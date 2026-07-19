use super::*;

impl Store {
    pub fn open(home: &Path) -> Result<Self, HubError> {
        harden_home(home)?;
        let database = home.join("hub.db");
        let conn = Connection::open(&database)?;
        conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON;")?;
        harden_sensitive_file(&database)?;
        for suffix in ["hub.db-wal", "hub.db-shm"] {
            let sidecar = home.join(suffix);
            if sidecar.exists() {
                harden_sensitive_file(&sidecar)?;
            }
        }
        Self::migrate(&conn)?;
        let store = Self {
            conn: Mutex::new(conn),
            #[cfg(test)]
            fail_create_conversation_once: AtomicBool::new(false),
            #[cfg(test)]
            fail_static_snapshot_once: AtomicBool::new(false),
        };
        Ok(store)
    }

    pub fn open_memory() -> Result<Self, HubError> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch("PRAGMA foreign_keys = ON;")?;
        Self::migrate(&conn)?;
        let store = Self {
            conn: Mutex::new(conn),
            #[cfg(test)]
            fail_create_conversation_once: AtomicBool::new(false),
            #[cfg(test)]
            fail_static_snapshot_once: AtomicBool::new(false),
        };
        Ok(store)
    }

    fn migrate(conn: &Connection) -> Result<(), HubError> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_migrations(
                version INTEGER PRIMARY KEY, applied_at TEXT NOT NULL);",
        )?;
        let current: i64 = conn.query_row(
            "SELECT COALESCE(MAX(version),0) FROM schema_migrations",
            [],
            |r| r.get(0),
        )?;
        if current < 1 {
            let tx = conn.unchecked_transaction()?;
            tx.execute_batch(MIGRATION_1)?;
            tx.execute(
                "INSERT INTO schema_migrations(version, applied_at) VALUES (1, ?)",
                params![now_iso()],
            )?;
            tx.commit()?;
        }
        if current < 2 {
            let tx = conn.unchecked_transaction()?;
            tx.execute_batch(
                "CREATE TABLE IF NOT EXISTS load_replay_refreshes(
                    conv_id TEXT PRIMARY KEY,
                    load_id TEXT NOT NULL UNIQUE,
                    starting_seq INTEGER NOT NULL,
                    started_at TEXT NOT NULL,
                    FOREIGN KEY(conv_id) REFERENCES conversations(id) ON DELETE CASCADE
                );",
            )?;
            tx.execute(
                "INSERT INTO schema_migrations(version, applied_at) VALUES (2, ?)",
                params![now_iso()],
            )?;
            tx.commit()?;
        }
        if current < 3 {
            let tx = conn.unchecked_transaction()?;
            tx.execute_batch(
                "CREATE TABLE IF NOT EXISTS load_replay_projection_before_images(
                    load_id TEXT PRIMARY KEY
                        REFERENCES load_replay_refreshes(load_id) ON DELETE CASCADE,
                    conv_id TEXT NOT NULL UNIQUE
                        REFERENCES conversations(id) ON DELETE CASCADE,
                    conversation_title TEXT,
                    conversation_updated_at TEXT NOT NULL,
                    session_meta_json TEXT,
                    fts_title_present INTEGER NOT NULL CHECK(fts_title_present IN (0, 1)),
                    fts_title TEXT,
                    config_present INTEGER NOT NULL CHECK(config_present IN (0, 1)),
                    config_options_json TEXT,
                    config_modes_json TEXT,
                    config_updated_at TEXT,
                    plan_present INTEGER NOT NULL CHECK(plan_present IN (0, 1)),
                    plan_entries_json TEXT,
                    plan_updated_at TEXT,
                    commands_present INTEGER NOT NULL CHECK(commands_present IN (0, 1)),
                    commands_json TEXT,
                    commands_updated_at TEXT,
                    usage_present INTEGER NOT NULL CHECK(usage_present IN (0, 1)),
                    usage_used INTEGER,
                    usage_size INTEGER,
                    usage_cost_json TEXT,
                    usage_updated_at TEXT
                );",
            )?;
            tx.execute(
                "INSERT INTO schema_migrations(version, applied_at) VALUES (3, ?)",
                params![now_iso()],
            )?;
            tx.commit()?;
        }
        if current < 4 {
            let has_generation_nonce: bool = conn.query_row(
                "SELECT EXISTS(
                     SELECT 1
                     FROM pragma_table_info('load_replay_refreshes')
                     WHERE name = 'generation_nonce'
                 )",
                [],
                |row| row.get(0),
            )?;
            let tx = conn.unchecked_transaction()?;
            if !has_generation_nonce {
                tx.execute_batch(
                    "ALTER TABLE load_replay_refreshes
                         ADD COLUMN generation_nonce TEXT;",
                )?;
            }
            tx.execute(
                "UPDATE load_replay_refreshes
                 SET generation_nonce = lower(hex(randomblob(16)))
                 WHERE generation_nonce IS NULL",
                [],
            )?;
            tx.execute(
                "INSERT INTO schema_migrations(version, applied_at) VALUES (4, ?)",
                params![now_iso()],
            )?;
            tx.commit()?;
        }
        if current < 5 {
            let has_projection_generation =
                table_has_column(conn, "conversations", "projection_generation")?;
            let has_import_before_image = table_has_column(
                conn,
                "load_replay_projection_before_images",
                "conversation_was_present",
            )?;
            let tx = conn.unchecked_transaction()?;
            if !has_projection_generation {
                tx.execute_batch(
                    "ALTER TABLE conversations
                         ADD COLUMN projection_generation INTEGER NOT NULL DEFAULT 0;",
                )?;
            }
            if !has_import_before_image {
                tx.execute_batch(
                    "ALTER TABLE load_replay_projection_before_images
                         ADD COLUMN conversation_was_present INTEGER NOT NULL DEFAULT 1
                         CHECK(conversation_was_present IN (0, 1));
                     ALTER TABLE load_replay_projection_before_images
                         ADD COLUMN conversation_status TEXT;
                     ALTER TABLE load_replay_projection_before_images
                         ADD COLUMN conversation_cwd TEXT;
                     ALTER TABLE load_replay_projection_before_images
                         ADD COLUMN conversation_directories_json TEXT;",
                )?;
            }
            tx.execute(
                "INSERT INTO schema_migrations(version, applied_at) VALUES (5, ?)",
                params![now_iso()],
            )?;
            tx.commit()?;
        }
        if current < 6 {
            let tx = conn.unchecked_transaction()?;
            tx.execute_batch(
                "CREATE TABLE IF NOT EXISTS hub_metadata(
                     key TEXT PRIMARY KEY,
                     value TEXT NOT NULL
                 );
                 INSERT OR IGNORE INTO hub_metadata(key, value)
                 VALUES ('message_cursor_hmac_key', lower(hex(randomblob(32))));",
            )?;
            tx.execute(
                "INSERT INTO schema_migrations(version, applied_at) VALUES (6, ?)",
                params![now_iso()],
            )?;
            tx.commit()?;
        }
        Ok(())
    }

    // --- agent_cache -------------------------------------------------------

    pub fn upsert_agent_cache(
        &self,
        id: &str,
        agent_info_json: &str,
        capabilities_json: &str,
    ) -> Result<(), HubError> {
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO agent_cache(id, agent_info_json, capabilities_json, inspected_at)
             VALUES (?, ?, ?, ?)
             ON CONFLICT(id) DO UPDATE SET
               agent_info_json = excluded.agent_info_json,
               capabilities_json = excluded.capabilities_json,
               inspected_at = excluded.inspected_at",
            params![id, agent_info_json, capabilities_json, now_iso()],
        )?;
        Ok(())
    }

    pub fn agent_cache(&self, id: &str) -> Result<Option<(String, String)>, HubError> {
        let conn = self.conn.lock();
        Ok(conn
            .query_row(
                "SELECT agent_info_json, capabilities_json FROM agent_cache WHERE id = ?",
                params![id],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
            )
            .optional()?)
    }

    pub fn delete_agent_cache(&self, id: &str) -> Result<(), HubError> {
        self.conn
            .lock()
            .execute("DELETE FROM agent_cache WHERE id = ?", params![id])?;
        Ok(())
    }

    // --- conversations -----------------------------------------------------

    #[cfg(test)]
    pub(crate) fn fail_next_create_conversation_for_test(&self) {
        self.fail_create_conversation_once
            .store(true, std::sync::atomic::Ordering::Release);
    }

    pub fn create_conversation(&self, c: &NewConversation) -> Result<(), HubError> {
        #[cfg(test)]
        if self
            .fail_create_conversation_once
            .swap(false, std::sync::atomic::Ordering::AcqRel)
        {
            return Err(HubError::other("injected conversation creation failure"));
        }
        let mut conn = self.conn.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let dirs = serde_json::to_string(&c.additional_directories)?;
        let ts = now_iso();
        tx.execute(
            "INSERT INTO conversations(
                 id, agent_id, agent_session_id, title, status,
                 cwd, additional_directories_json, session_meta_json,
                 created_at, updated_at)
             VALUES (?, ?, ?, ?, 'idle', ?, ?, NULL, ?, ?)",
            params![
                c.id,
                c.agent_id,
                c.agent_session_id,
                c.title,
                c.cwd,
                dirs,
                ts,
                ts
            ],
        )?;
        let fts_title = c.title.as_deref().unwrap_or("");
        tx.execute(
            "INSERT INTO conversations_fts(conv_id, title) VALUES (?, ?)",
            params![c.id, fts_title],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn conversation(&self, conv_id: &str) -> Result<Option<ConversationRow>, HubError> {
        let conn = self.conn.lock();
        Ok(conn
            .query_row(CONV_SELECT[0], params![conv_id], map_conversation)
            .optional()?)
    }

    pub fn conversation_by_agent_session(
        &self,
        agent_id: &str,
        agent_session_id: &str,
    ) -> Result<Option<ConversationRow>, HubError> {
        let conn = self.conn.lock();
        Ok(conn
            .query_row(
                CONV_SELECT[1],
                params![agent_id, agent_session_id],
                map_conversation,
            )
            .optional()?)
    }

    pub fn list_conversations(
        &self,
        agent_id: Option<&str>,
    ) -> Result<Vec<ConversationRow>, HubError> {
        let conn = self.conn.lock();
        let sql = if agent_id.is_some() {
            CONV_SELECT[2]
        } else {
            CONV_SELECT[3]
        };
        let mut stmt = conn.prepare(sql)?;
        let rows = if let Some(a) = agent_id {
            stmt.query_map(params![a], map_conversation)?
        } else {
            stmt.query_map([], map_conversation)?
        };
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    pub fn set_conv_status(&self, conv_id: &str, status: ConvStatus) -> Result<(), HubError> {
        let conn = self.conn.lock();
        conn.execute(
            "UPDATE conversations SET status = ?, updated_at = ? WHERE id = ?",
            params![status.as_str(), now_iso(), conv_id],
        )?;
        Ok(())
    }

    pub fn touch_conversation(&self, conv_id: &str) -> Result<(), HubError> {
        let conn = self.conn.lock();
        conn.execute(
            "UPDATE conversations SET updated_at = ? WHERE id = ?",
            params![now_iso(), conv_id],
        )?;
        Ok(())
    }

    /// Apply a partial `session_info_update`: title/updatedAt/_meta only.
    pub fn apply_session_info(
        &self,
        conv_id: &str,
        title: Option<&str>,
        updated_at: Option<&str>,
        meta_patch: Option<&serde_json::Map<String, serde_json::Value>>,
    ) -> Result<(), HubError> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        if let Some(t) = title {
            tx.execute(
                "UPDATE conversations SET title = ? WHERE id = ?",
                params![t, conv_id],
            )?;
            tx.execute(
                "DELETE FROM conversations_fts WHERE conv_id = ?",
                params![conv_id],
            )?;
            tx.execute(
                "INSERT INTO conversations_fts(conv_id, title) VALUES (?, ?)",
                params![conv_id, t],
            )?;
        }
        if updated_at.is_some() {
            tx.execute(
                "UPDATE conversations SET updated_at = ? WHERE id = ?",
                params![now_iso(), conv_id],
            )?;
        }
        if let Some(patch) = meta_patch {
            let existing: Option<String> = tx
                .query_row(
                    "SELECT session_meta_json FROM conversations WHERE id = ?",
                    params![conv_id],
                    |r| r.get::<_, Option<String>>(0),
                )
                .optional()?
                .flatten();
            let mut merged: serde_json::Map<String, serde_json::Value> = existing
                .map(|s| serde_json::from_str(&s))
                .transpose()?
                .unwrap_or_default();
            for (k, v) in patch {
                if v.is_null() {
                    merged.remove(k);
                } else {
                    merged.insert(k.clone(), v.clone());
                }
            }
            let serialized = if merged.is_empty() {
                None
            } else {
                Some(serde_json::to_string(&serde_json::Value::Object(merged))?)
            };
            tx.execute(
                "UPDATE conversations SET session_meta_json = ? WHERE id = ?",
                params![serialized, conv_id],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Replace the complete `additionalDirectories` list (never merges omitted).
    pub fn set_additional_directories(
        &self,
        conv_id: &str,
        dirs: &[String],
    ) -> Result<(), HubError> {
        let conn = self.conn.lock();
        let serialized = serde_json::to_string(dirs)?;
        conn.execute(
            "UPDATE conversations SET additional_directories_json = ?, updated_at = ? WHERE id = ?",
            params![serialized, now_iso(), conv_id],
        )?;
        Ok(())
    }

    pub fn delete_conversation(&self, conv_id: &str) -> Result<(), HubError> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let active: bool = tx.query_row(
            "SELECT EXISTS(
                 SELECT 1 FROM runs
                 WHERE conv_id = ? AND status IN ('running','cancelling')
             )",
            params![conv_id],
            |r| r.get(0),
        )?;
        if active {
            return Err(HubError::Conflict(conv_id.to_string()));
        }
        tx.execute(
            "DELETE FROM messages_fts WHERE conv_id = ?",
            params![conv_id],
        )?;
        tx.execute(
            "DELETE FROM conversations_fts WHERE conv_id = ?",
            params![conv_id],
        )?;
        tx.execute("DELETE FROM conversations WHERE id = ?", params![conv_id])?;
        tx.commit()?;
        Ok(())
    }

    /// Upsert a conversation row discovered via agent `session/list`.
    /// Creates a new row if the (agent_id, agent_session_id) pair doesn't
    /// exist; otherwise updates title/cwd/directories/meta. Does NOT touch
    /// messages — use `stage_load_replay` to import message history.
    pub fn upsert_agent_session(
        &self,
        agent_id: &str,
        agent_session_id: &str,
        title: Option<&str>,
        cwd: Option<&str>,
        additional_directories: &[String],
    ) -> Result<String, HubError> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let existing_id: Option<String> = tx
            .query_row(
                "SELECT id FROM conversations WHERE agent_id = ? AND agent_session_id = ?",
                params![agent_id, agent_session_id],
                |r| r.get(0),
            )
            .optional()?;
        let dirs = serde_json::to_string(additional_directories)?;
        let ts = now_iso();
        if let Some(id) = existing_id {
            // Update metadata.
            if let Some(t) = title {
                tx.execute(
                    "UPDATE conversations SET title = ?, updated_at = ? WHERE id = ?",
                    params![t, ts, id],
                )?;
                tx.execute(
                    "DELETE FROM conversations_fts WHERE conv_id = ?",
                    params![id],
                )?;
                tx.execute(
                    "INSERT INTO conversations_fts(conv_id, title) VALUES (?, ?)",
                    params![id, t],
                )?;
            }
            if let Some(c) = cwd {
                tx.execute(
                    "UPDATE conversations SET cwd = ? WHERE id = ?",
                    params![c, id],
                )?;
            }
            tx.execute(
                "UPDATE conversations SET additional_directories_json = ? WHERE id = ?",
                params![dirs, id],
            )?;
            tx.commit()?;
            Ok(id)
        } else {
            // Create new conversation row from agent-side discovery.
            let conv_id = format!("conv-{}", uuid::Uuid::new_v4().simple());
            tx.execute(
                "INSERT INTO conversations(
                     id, agent_id, agent_session_id, title, status,
                     cwd, additional_directories_json, session_meta_json,
                     created_at, updated_at)
                 VALUES (?, ?, ?, ?, 'idle', ?, ?, NULL, ?, ?)",
                params![
                    conv_id,
                    agent_id,
                    agent_session_id,
                    title,
                    cwd,
                    dirs,
                    ts,
                    ts
                ],
            )?;
            tx.execute(
                "INSERT INTO conversations_fts(conv_id, title) VALUES (?, ?)",
                params![conv_id, title.unwrap_or("")],
            )?;
            tx.commit()?;
            Ok(conv_id)
        }
    }

    // --- runs --------------------------------------------------------------

    pub fn create_run(&self, run_id: &str, conv_id: &str) -> Result<(), HubError> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let active: bool = tx.query_row(
            "SELECT EXISTS(
                 SELECT 1 FROM runs
                 WHERE conv_id = ? AND status IN ('running','cancelling')
             )",
            params![conv_id],
            |r| r.get(0),
        )?;
        if active {
            return Err(HubError::Conflict(conv_id.to_string()));
        }
        tx.execute(
            "INSERT INTO runs(id, conv_id, status, started_at) VALUES (?, ?, 'running', ?)",
            params![run_id, conv_id, now_iso()],
        )?;
        tx.execute(
            "UPDATE conversations SET status = 'running', updated_at = ? WHERE id = ?",
            params![now_iso(), conv_id],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Compare-and-set finalize: only updates if status is running/cancelling.
    pub fn finalize_run_cas(
        &self,
        run_id: &str,
        conv_id: &str,
        status: RunStatus,
        stop_reason: Option<&str>,
    ) -> Result<bool, HubError> {
        if status == RunStatus::Running {
            return Err(HubError::other(
                "finalize_run cannot transition a run to running",
            ));
        }
        let mut conn = self.conn.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let actual_conv: Option<String> = tx
            .query_row(
                "SELECT conv_id FROM runs WHERE id = ?",
                params![run_id],
                |r| r.get(0),
            )
            .optional()?;
        let Some(actual_conv) = actual_conv else {
            return Ok(false);
        };
        if actual_conv != conv_id {
            return Err(HubError::other(format!(
                "run {run_id} belongs to conversation {actual_conv}, not {conv_id}"
            )));
        }
        let ended_at = (status != RunStatus::Cancelling).then(now_iso);
        let updated = tx.execute(
            "UPDATE runs SET status = ?, stop_reason = ?, ended_at = ?
             WHERE id = ? AND conv_id = ? AND status IN ('running','cancelling')",
            params![status.as_str(), stop_reason, ended_at, run_id, conv_id],
        )?;
        if updated > 0 {
            let conv_status = match status {
                RunStatus::Cancelling => ConvStatus::Cancelling,
                RunStatus::Running => ConvStatus::Running,
                _ => ConvStatus::Idle,
            };
            tx.execute(
                "UPDATE conversations SET status = ?, updated_at = ? WHERE id = ?",
                params![conv_status.as_str(), now_iso(), conv_id],
            )?;
        }
        tx.commit()?;
        Ok(updated > 0)
    }

    /// Resolve run/conversation state left behind by an unclean daemon exit.
    ///
    /// No ACP command survives a daemon process, so persisted non-terminal
    /// runs cannot truthfully remain `running` after startup.
    pub fn recover_interrupted_runs(&self) -> Result<usize, HubError> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let ts = now_iso();
        let recovered = tx.execute(
            "UPDATE runs
             SET status = 'failed',
                 stop_reason = COALESCE(stop_reason, 'daemon_restarted'),
                 ended_at = ?
             WHERE status IN ('running','cancelling')",
            params![ts],
        )?;
        tx.execute(
            "UPDATE conversations
             SET status = 'failed', updated_at = ?
             WHERE status IN ('running','cancelling')",
            params![now_iso()],
        )?;
        tx.commit()?;
        Ok(recovered)
    }

    pub fn run_status(&self, run_id: &str) -> Result<Option<RunStatus>, HubError> {
        let conn = self.conn.lock();
        let row = conn
            .query_row(
                "SELECT status FROM runs WHERE id = ?",
                params![run_id],
                |r| r.get::<_, String>(0),
            )
            .optional()?;
        row.map(|status| {
            RunStatus::parse(&status).ok_or_else(|| {
                HubError::other(format!(
                    "corrupt persisted run status {status:?} for run {run_id}"
                ))
            })
        })
        .transpose()
    }

    // --- messages ----------------------------------------------------------

    /// Append a message, allocating `seq` atomically inside `BEGIN IMMEDIATE`.
    pub fn append_message(&self, m: &NewMessage) -> Result<i64, HubError> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let seq: i64 = tx.query_row(
            "SELECT COALESCE(MAX(seq), 0) + 1 FROM messages WHERE conv_id = ?",
            params![m.conv_id],
            |r| r.get(0),
        )?;
        let content = serde_json::to_string(&m.content_json)?;
        tx.execute(
            "INSERT INTO messages(
                 id, conv_id, run_id, source, current_projection, message_key,
                 superseded_by_load_id, role, kind, content_json, body_text,
                 seq, created_at)
             VALUES (?, ?, ?, ?, 1, NULL, NULL, ?, ?, ?, ?, ?, ?)",
            params![
                m.id,
                m.conv_id,
                m.run_id,
                m.source.as_str(),
                m.role,
                m.kind,
                content,
                m.body_text,
                seq,
                now_iso(),
            ],
        )?;
        tx.execute(
            "INSERT INTO messages_fts(message_id, conv_id, body) VALUES (?, ?, ?)",
            params![m.id, m.conv_id, m.body_text],
        )?;
        tx.commit()?;
        Ok(seq)
    }
}
