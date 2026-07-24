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
            #[cfg(test)]
            fail_append_message_once: AtomicBool::new(false),
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
            #[cfg(test)]
            fail_append_message_once: AtomicBool::new(false),
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
        if current < 7 {
            // Phase-1 hybrid conversation fields + closed status + soft-delete keep row.
            // Rebuild conversations so status CHECK includes 'closed'.
            // foreign_keys OFF so DROP does not cascade-delete messages/runs.
            conn.pragma_update(None, "foreign_keys", false)?;
            let tx = conn.unchecked_transaction()?;
            tx.execute_batch(
                r#"
CREATE TABLE conversations_v7(
    id TEXT PRIMARY KEY,
    agent_id TEXT NOT NULL,
    agent_session_id TEXT NOT NULL,
    title TEXT,
    status TEXT NOT NULL CHECK(status IN (
        'idle','running','cancelling','cancelled','failed','completed','closed','deleted'
    )),
    cwd TEXT,
    additional_directories_json TEXT NOT NULL DEFAULT '[]',
    session_meta_json TEXT,
    projection_generation INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    origin TEXT NOT NULL CHECK(origin IN ('hub_created','bound','imported_list')),
    interaction TEXT NOT NULL CHECK(interaction IN ('writable','read_only')),
    phase TEXT NOT NULL CHECK(phase IN ('open','closed','deleted')),
    busy TEXT NOT NULL CHECK(busy IN ('none','running','cancelling')),
    last_outcome TEXT NOT NULL CHECK(last_outcome IN ('none','completed','failed','cancelled')),
    UNIQUE(agent_id, agent_session_id)
);
INSERT INTO conversations_v7(
    id, agent_id, agent_session_id, title, status, cwd,
    additional_directories_json, session_meta_json, projection_generation,
    created_at, updated_at, origin, interaction, phase, busy, last_outcome
)
SELECT
    c.id, c.agent_id, c.agent_session_id, c.title, c.status, c.cwd,
    c.additional_directories_json, c.session_meta_json, c.projection_generation,
    c.created_at, c.updated_at,
    CASE
        WHEN EXISTS (
            SELECT 1 FROM messages m
            WHERE m.conv_id = c.id
              AND m.source = 'local_turn'
              AND m.current_projection = 1
        ) THEN 'hub_created'
        ELSE 'imported_list'
    END,
    CASE
        WHEN EXISTS (
            SELECT 1 FROM messages m
            WHERE m.conv_id = c.id
              AND m.source = 'local_turn'
              AND m.current_projection = 1
        ) THEN 'writable'
        ELSE 'read_only'
    END,
    CASE c.status
        WHEN 'deleted' THEN 'deleted'
        ELSE 'open'
    END,
    CASE c.status
        WHEN 'running' THEN 'running'
        WHEN 'cancelling' THEN 'cancelling'
        ELSE 'none'
    END,
    CASE c.status
        WHEN 'completed' THEN 'completed'
        WHEN 'failed' THEN 'failed'
        WHEN 'cancelled' THEN 'cancelled'
        ELSE 'none'
    END
FROM conversations c;
DROP TABLE conversations;
ALTER TABLE conversations_v7 RENAME TO conversations;
CREATE INDEX IF NOT EXISTS idx_conversations_updated
    ON conversations(updated_at DESC, id ASC);
"#,
            )?;
            tx.execute(
                "INSERT INTO schema_migrations(version, applied_at) VALUES (7, ?)",
                params![now_iso()],
            )?;
            tx.commit()?;
            conn.pragma_update(None, "foreign_keys", true)?;
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

    #[cfg(test)]
    pub(crate) fn fail_next_append_message_for_test(&self) {
        self.fail_append_message_once
            .store(true, std::sync::atomic::Ordering::Release);
    }

    pub fn create_conversation(&self, c: &NewConversation) -> Result<(), HubError> {
        self.create_conversation_with_options(c, &NewConversationOptions::default())
    }

    pub fn create_conversation_with_options(
        &self,
        c: &NewConversation,
        opts: &NewConversationOptions,
    ) -> Result<(), HubError> {
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
        let phase = conversation_policy::ConvPhase::Open;
        let busy = conversation_policy::ConvBusy::None;
        let last_outcome = conversation_policy::LastOutcome::None;
        let interaction =
            conversation_policy::recompute_interaction(opts.origin, opts.session_meta.as_ref());
        let status = super::mirror_status(phase, busy, last_outcome);
        let meta_json = opts
            .session_meta
            .as_ref()
            .map(serde_json::to_string)
            .transpose()?;
        tx.execute(
            "INSERT INTO conversations(
                 id, agent_id, agent_session_id, title, status,
                 cwd, additional_directories_json, session_meta_json,
                 created_at, updated_at,
                 origin, interaction, phase, busy, last_outcome)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            params![
                c.id,
                c.agent_id,
                c.agent_session_id,
                c.title,
                status.as_str(),
                c.cwd,
                dirs,
                meta_json,
                ts,
                ts,
                opts.origin.as_str(),
                interaction.as_str(),
                phase.as_str(),
                busy.as_str(),
                last_outcome.as_str(),
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
        let mut filter = ListConversationsFilter::workbench_default();
        // Legacy callers: full open list (not workbench-only) for agent filter compatibility.
        filter.workbench = false;
        filter.include_imported = true;
        filter.limit = 10_000;
        filter.agent_id = agent_id.map(str::to_string);
        Ok(self.list_conversations_filtered(&filter)?.items)
    }

    /// PHASE1-CONTRACT §6.1 list with workbench / filters / envelope counts.
    pub fn list_conversations_filtered(
        &self,
        filter: &ListConversationsFilter,
    ) -> Result<ConversationListPage, HubError> {
        let conn = self.conn.lock();
        let limit = if filter.limit == 0 { 100 } else { filter.limit };
        let offset = filter.offset;

        let mut where_parts: Vec<String> = Vec::new();
        let mut binds: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

        // Status filter may include closed; default excludes deleted always.
        let status_filter = filter.status.as_deref();
        if status_filter == Some("closed") {
            where_parts.push("phase = 'closed'".into());
        } else if status_filter == Some("deleted") {
            // Phase1: deleted never listed
            return Ok(ConversationListPage {
                items: vec![],
                limit,
                offset,
                truncated: false,
            });
        } else {
            where_parts.push("phase = 'open'".into());
            if let Some(s) = status_filter {
                where_parts.push("status = ?".into());
                binds.push(Box::new(s.to_string()));
            }
        }

        // Workbench: default on; --all / include_imported clears; --status clears unless force.
        let use_workbench = if filter.force_workbench {
            true
        } else if filter.include_imported || status_filter.is_some() {
            false
        } else {
            filter.workbench
        };

        if use_workbench {
            where_parts.push(
                "(origin IN ('hub_created','bound') OR EXISTS (
                    SELECT 1 FROM messages m
                    WHERE m.conv_id = conversations.id
                      AND m.source = 'local_turn'
                      AND m.current_projection = 1
                ))"
                .into(),
            );
        }

        if let Some(agent) = &filter.agent_id {
            where_parts.push("agent_id = ?".into());
            binds.push(Box::new(agent.clone()));
        }
        if let Some(ix) = &filter.interaction {
            where_parts.push("interaction = ?".into());
            binds.push(Box::new(ix.clone()));
        }

        let where_sql = if where_parts.is_empty() {
            "1=1".to_string()
        } else {
            where_parts.join(" AND ")
        };

        let count_sql = format!("SELECT COUNT(*) FROM conversations WHERE {where_sql}");
        let total: usize = {
            let mut stmt = conn.prepare(&count_sql)?;
            let params_refs: Vec<&dyn rusqlite::types::ToSql> =
                binds.iter().map(|b| b.as_ref()).collect();
            stmt.query_row(params_refs.as_slice(), |r| r.get::<_, i64>(0))? as usize
        };

        let sql = format!(
            "SELECT id, agent_id, agent_session_id, title, status, cwd,
                    additional_directories_json, session_meta_json, created_at, updated_at,
                    origin, interaction, phase, busy, last_outcome
             FROM conversations WHERE {where_sql}
             ORDER BY updated_at DESC, id ASC
             LIMIT ? OFFSET ?"
        );
        let mut stmt = conn.prepare(&sql)?;
        let mut params_refs: Vec<&dyn rusqlite::types::ToSql> =
            binds.iter().map(|b| b.as_ref()).collect();
        let limit_i = limit as i64;
        let offset_i = offset as i64;
        params_refs.push(&limit_i);
        params_refs.push(&offset_i);
        let rows = stmt.query_map(params_refs.as_slice(), map_conversation)?;
        let mut items = Vec::new();
        for r in rows {
            items.push(r?);
        }
        let truncated = total > offset.saturating_add(items.len());
        Ok(ConversationListPage {
            items,
            limit,
            offset,
            truncated,
        })
    }

    /// Set hybrid fields and mirror synthetic status.
    pub fn set_conversation_lifecycle(
        &self,
        conv_id: &str,
        phase: conversation_policy::ConvPhase,
        busy: conversation_policy::ConvBusy,
        last_outcome: conversation_policy::LastOutcome,
    ) -> Result<(), HubError> {
        let conn = self.conn.lock();
        let status = super::mirror_status(phase, busy, last_outcome);
        conn.execute(
            "UPDATE conversations SET phase = ?, busy = ?, last_outcome = ?, status = ?, updated_at = ?
             WHERE id = ?",
            params![
                phase.as_str(),
                busy.as_str(),
                last_outcome.as_str(),
                status.as_str(),
                now_iso(),
                conv_id
            ],
        )?;
        Ok(())
    }

    pub fn set_conv_status(&self, conv_id: &str, status: ConvStatus) -> Result<(), HubError> {
        // Preserve last_outcome when closing without busy (close path sets phase only).
        if status == ConvStatus::Closed {
            let conn = self.conn.lock();
            let existing: Option<String> = conn
                .query_row(
                    "SELECT last_outcome FROM conversations WHERE id = ?",
                    params![conv_id],
                    |r| r.get(0),
                )
                .optional()?;
            let keep = existing
                .as_deref()
                .and_then(conversation_policy::LastOutcome::parse)
                .unwrap_or(conversation_policy::LastOutcome::None);
            let mirror = super::mirror_status(
                conversation_policy::ConvPhase::Closed,
                conversation_policy::ConvBusy::None,
                keep,
            );
            conn.execute(
                "UPDATE conversations SET phase = 'closed', busy = 'none', status = ?, updated_at = ?
                 WHERE id = ?",
                params![mirror.as_str(), now_iso(), conv_id],
            )?;
            return Ok(());
        }
        let (phase, busy, last_outcome) = match status {
            ConvStatus::Running => (
                conversation_policy::ConvPhase::Open,
                conversation_policy::ConvBusy::Running,
                conversation_policy::LastOutcome::None,
            ),
            ConvStatus::Cancelling => (
                conversation_policy::ConvPhase::Open,
                conversation_policy::ConvBusy::Cancelling,
                conversation_policy::LastOutcome::None,
            ),
            ConvStatus::Completed => (
                conversation_policy::ConvPhase::Open,
                conversation_policy::ConvBusy::None,
                conversation_policy::LastOutcome::Completed,
            ),
            ConvStatus::Failed => (
                conversation_policy::ConvPhase::Open,
                conversation_policy::ConvBusy::None,
                conversation_policy::LastOutcome::Failed,
            ),
            ConvStatus::Cancelled => (
                conversation_policy::ConvPhase::Open,
                conversation_policy::ConvBusy::None,
                conversation_policy::LastOutcome::Cancelled,
            ),
            ConvStatus::Deleted => (
                conversation_policy::ConvPhase::Deleted,
                conversation_policy::ConvBusy::None,
                conversation_policy::LastOutcome::None,
            ),
            ConvStatus::Idle | ConvStatus::Closed => (
                conversation_policy::ConvPhase::Open,
                conversation_policy::ConvBusy::None,
                conversation_policy::LastOutcome::None,
            ),
        };
        self.set_conversation_lifecycle(conv_id, phase, busy, last_outcome)
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

    /// Soft-delete (PHASE1-CONTRACT §4.5): phase=deleted, keep UNIQUE row.
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
        let busy_raw: Option<String> = tx
            .query_row(
                "SELECT busy FROM conversations WHERE id = ?",
                params![conv_id],
                |r| r.get(0),
            )
            .optional()?;
        if busy_raw
            .as_deref()
            .and_then(conversation_policy::ConvBusy::parse)
            .is_some_and(|b| b.is_busy())
        {
            return Err(HubError::Conflict(conv_id.to_string()));
        }
        tx.execute(
            "UPDATE conversations
             SET phase = 'deleted', busy = 'none', status = 'deleted', updated_at = ?
             WHERE id = ?",
            params![now_iso(), conv_id],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Hard-delete for create-rollback only (not operator-facing).
    pub fn hard_delete_conversation(&self, conv_id: &str) -> Result<(), HubError> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
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

    /// Metadata-only discover upsert — never session/load.
    /// Never downgrades hub_created/bound → imported_list.
    pub fn upsert_agent_session_discover(
        &self,
        agent_id: &str,
        agent_session_id: &str,
        title: Option<&str>,
        cwd: Option<&str>,
        additional_directories: &[String],
        session_meta: Option<&serde_json::Value>,
    ) -> Result<DiscoverUpsertResult, HubError> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        struct ExistingDiscoverRow {
            id: String,
            origin_raw: String,
            phase_raw: String,
            local_title: Option<String>,
            local_cwd: Option<String>,
        }
        let existing: Option<ExistingDiscoverRow> = tx
            .query_row(
                "SELECT id, origin, phase, title, cwd FROM conversations
                 WHERE agent_id = ? AND agent_session_id = ?",
                params![agent_id, agent_session_id],
                |r| {
                    Ok(ExistingDiscoverRow {
                        id: r.get(0)?,
                        origin_raw: r.get(1)?,
                        phase_raw: r.get(2)?,
                        local_title: r.get(3)?,
                        local_cwd: r.get(4)?,
                    })
                },
            )
            .optional()?;
        let dirs = serde_json::to_string(additional_directories)?;
        let ts = now_iso();
        let meta_json = session_meta.map(serde_json::to_string).transpose()?;

        if let Some(ExistingDiscoverRow {
            id,
            origin_raw,
            phase_raw,
            local_title,
            local_cwd,
        }) = existing
        {
            let phase = conversation_policy::ConvPhase::parse(&phase_raw)
                .unwrap_or(conversation_policy::ConvPhase::Open);
            let origin = conversation_policy::ConvOrigin::parse(&origin_raw)
                .unwrap_or(conversation_policy::ConvOrigin::ImportedList);
            let in_hub_before = phase != conversation_policy::ConvPhase::Deleted;

            if phase == conversation_policy::ConvPhase::Deleted {
                // Revive as imported_list museum row
                let interaction = conversation_policy::Interaction::ReadOnly;
                let status = super::mirror_status(
                    conversation_policy::ConvPhase::Open,
                    conversation_policy::ConvBusy::None,
                    conversation_policy::LastOutcome::None,
                );
                let new_title = title;
                tx.execute(
                    "UPDATE conversations SET
                        origin = 'imported_list', interaction = 'read_only',
                        phase = 'open', busy = 'none', last_outcome = 'none',
                        status = ?, title = COALESCE(?, title), cwd = COALESCE(?, cwd),
                        additional_directories_json = ?,
                        session_meta_json = COALESCE(?, session_meta_json),
                        updated_at = ?
                     WHERE id = ?",
                    params![status.as_str(), new_title, cwd, dirs, meta_json, ts, id],
                )?;
                if let Some(t) = new_title {
                    tx.execute(
                        "DELETE FROM conversations_fts WHERE conv_id = ?",
                        params![id],
                    )?;
                    tx.execute(
                        "INSERT INTO conversations_fts(conv_id, title) VALUES (?, ?)",
                        params![id, t],
                    )?;
                }
                tx.commit()?;
                return Ok(DiscoverUpsertResult {
                    conv_id: id,
                    in_hub_before: false,
                    origin: conversation_policy::ConvOrigin::ImportedList,
                    interaction,
                });
            }

            // Never downgrade hub_created/bound
            let keep_origin = origin;
            let merged_title = conversation_policy::merge_discover_title(
                keep_origin,
                local_title.as_deref(),
                title,
            );
            let merged_cwd =
                conversation_policy::merge_discover_cwd(keep_origin, local_cwd.as_deref(), cwd);
            // Refresh meta merge: remote keys replace
            let existing_meta: Option<String> = tx
                .query_row(
                    "SELECT session_meta_json FROM conversations WHERE id = ?",
                    params![id],
                    |r| r.get(0),
                )
                .optional()?
                .flatten();
            let merged_meta = merge_session_meta_json(existing_meta.as_deref(), session_meta)?;
            let interaction = if keep_origin == conversation_policy::ConvOrigin::ImportedList {
                conversation_policy::Interaction::ReadOnly
            } else {
                conversation_policy::recompute_interaction(
                    keep_origin,
                    merged_meta
                        .as_ref()
                        .and_then(|s| serde_json::from_str(s).ok())
                        .as_ref(),
                )
            };
            tx.execute(
                "UPDATE conversations SET
                    title = ?, cwd = ?, additional_directories_json = ?,
                    session_meta_json = ?, interaction = ?, updated_at = ?
                 WHERE id = ?",
                params![
                    merged_title,
                    merged_cwd,
                    dirs,
                    merged_meta,
                    interaction.as_str(),
                    ts,
                    id
                ],
            )?;
            if let Some(ref t) = merged_title {
                tx.execute(
                    "DELETE FROM conversations_fts WHERE conv_id = ?",
                    params![id],
                )?;
                tx.execute(
                    "INSERT INTO conversations_fts(conv_id, title) VALUES (?, ?)",
                    params![id, t],
                )?;
            }
            tx.commit()?;
            Ok(DiscoverUpsertResult {
                conv_id: id,
                in_hub_before,
                origin: keep_origin,
                interaction,
            })
        } else {
            let conv_id = format!("conv-{}", uuid::Uuid::new_v4().simple());
            let origin = conversation_policy::ConvOrigin::ImportedList;
            let interaction = conversation_policy::Interaction::ReadOnly;
            let status = super::mirror_status(
                conversation_policy::ConvPhase::Open,
                conversation_policy::ConvBusy::None,
                conversation_policy::LastOutcome::None,
            );
            tx.execute(
                "INSERT INTO conversations(
                     id, agent_id, agent_session_id, title, status,
                     cwd, additional_directories_json, session_meta_json,
                     created_at, updated_at,
                     origin, interaction, phase, busy, last_outcome)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'open', 'none', 'none')",
                params![
                    conv_id,
                    agent_id,
                    agent_session_id,
                    title,
                    status.as_str(),
                    cwd,
                    dirs,
                    meta_json,
                    ts,
                    ts,
                    origin.as_str(),
                    interaction.as_str(),
                ],
            )?;
            tx.execute(
                "INSERT INTO conversations_fts(conv_id, title) VALUES (?, ?)",
                params![conv_id, title.unwrap_or("")],
            )?;
            tx.commit()?;
            Ok(DiscoverUpsertResult {
                conv_id,
                in_hub_before: false,
                origin,
                interaction,
            })
        }
    }

    /// Legacy wrapper: discover without meta.
    pub fn upsert_agent_session(
        &self,
        agent_id: &str,
        agent_session_id: &str,
        title: Option<&str>,
        cwd: Option<&str>,
        additional_directories: &[String],
    ) -> Result<String, HubError> {
        Ok(self
            .upsert_agent_session_discover(
                agent_id,
                agent_session_id,
                title,
                cwd,
                additional_directories,
                None,
            )?
            .conv_id)
    }

    /// Promote imported_list → bound (or set origin on bind create).
    pub fn promote_conversation_bind(
        &self,
        conv_id: &str,
        session_meta: Option<&serde_json::Value>,
    ) -> Result<(), HubError> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (origin_raw, existing_meta): (String, Option<String>) = tx.query_row(
            "SELECT origin, session_meta_json FROM conversations WHERE id = ?",
            params![conv_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let origin = conversation_policy::ConvOrigin::parse(&origin_raw)
            .unwrap_or(conversation_policy::ConvOrigin::ImportedList);
        let new_origin = match origin {
            conversation_policy::ConvOrigin::HubCreated => {
                conversation_policy::ConvOrigin::HubCreated
            }
            conversation_policy::ConvOrigin::Bound
            | conversation_policy::ConvOrigin::ImportedList => {
                conversation_policy::ConvOrigin::Bound
            }
        };
        let merged_meta = merge_session_meta_json(existing_meta.as_deref(), session_meta)?;
        let meta_value: Option<serde_json::Value> = merged_meta
            .as_ref()
            .map(|s| serde_json::from_str(s))
            .transpose()?;
        let interaction =
            conversation_policy::recompute_interaction(new_origin, meta_value.as_ref());
        tx.execute(
            "UPDATE conversations SET origin = ?, interaction = ?, phase = 'open',
                session_meta_json = COALESCE(?, session_meta_json), updated_at = ?
             WHERE id = ?",
            params![
                new_origin.as_str(),
                interaction.as_str(),
                merged_meta,
                now_iso(),
                conv_id
            ],
        )?;
        // Refresh status mirror for open/none/* — keep last_outcome/busy
        let (busy_raw, outcome_raw): (String, String) = tx.query_row(
            "SELECT busy, last_outcome FROM conversations WHERE id = ?",
            params![conv_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let busy = conversation_policy::ConvBusy::parse(&busy_raw)
            .unwrap_or(conversation_policy::ConvBusy::None);
        let last_outcome = conversation_policy::LastOutcome::parse(&outcome_raw)
            .unwrap_or(conversation_policy::LastOutcome::None);
        let status = super::mirror_status(conversation_policy::ConvPhase::Open, busy, last_outcome);
        tx.execute(
            "UPDATE conversations SET status = ? WHERE id = ?",
            params![status.as_str(), conv_id],
        )?;
        tx.commit()?;
        Ok(())
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
        // send accepted: busy=running; last_outcome unchanged
        tx.execute(
            "UPDATE conversations SET busy = 'running', phase = 'open', status = 'running',
                updated_at = ? WHERE id = ?",
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
            // Hybrid: busy + last_outcome + status mirror (PHASE1 §1.3)
            let (busy, last_outcome, conv_status) = match status {
                RunStatus::Cancelling => (
                    conversation_policy::ConvBusy::Cancelling,
                    None,
                    ConvStatus::Cancelling,
                ),
                RunStatus::Running => (
                    conversation_policy::ConvBusy::Running,
                    None,
                    ConvStatus::Running,
                ),
                RunStatus::Completed => (
                    conversation_policy::ConvBusy::None,
                    Some(conversation_policy::LastOutcome::Completed),
                    ConvStatus::Completed,
                ),
                RunStatus::Cancelled => (
                    conversation_policy::ConvBusy::None,
                    Some(conversation_policy::LastOutcome::Cancelled),
                    ConvStatus::Cancelled,
                ),
                RunStatus::Failed => (
                    conversation_policy::ConvBusy::None,
                    Some(conversation_policy::LastOutcome::Failed),
                    ConvStatus::Failed,
                ),
            };
            if let Some(outcome) = last_outcome {
                tx.execute(
                    "UPDATE conversations SET busy = ?, last_outcome = ?, status = ?,
                        phase = 'open', updated_at = ? WHERE id = ?",
                    params![
                        busy.as_str(),
                        outcome.as_str(),
                        conv_status.as_str(),
                        now_iso(),
                        conv_id
                    ],
                )?;
            } else {
                tx.execute(
                    "UPDATE conversations SET busy = ?, status = ?, phase = 'open', updated_at = ?
                     WHERE id = ?",
                    params![busy.as_str(), conv_status.as_str(), now_iso(), conv_id],
                )?;
            }
        }
        tx.commit()?;
        Ok(updated > 0)
    }

    /// Transition one exact active run from running to cancelling.
    ///
    /// Unlike finalization, this transition is intentionally strict: a run
    /// that already reached cancelling or any terminal status must not cause
    /// another ACP cancellation notification.
    pub fn request_run_cancel_cas(&self, run_id: &str, conv_id: &str) -> Result<bool, HubError> {
        self.transition_run_cancel_state(run_id, conv_id, "running", "cancelling")
    }

    /// Restore a cancellation request whose ACP notification could not be sent.
    pub fn rollback_run_cancel_request_cas(
        &self,
        run_id: &str,
        conv_id: &str,
    ) -> Result<bool, HubError> {
        self.transition_run_cancel_state(run_id, conv_id, "cancelling", "running")
    }

    fn transition_run_cancel_state(
        &self,
        run_id: &str,
        conv_id: &str,
        from: &str,
        to: &str,
    ) -> Result<bool, HubError> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let actual_conv: Option<String> = tx
            .query_row(
                "SELECT conv_id FROM runs WHERE id = ?",
                params![run_id],
                |row| row.get(0),
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
        let updated = tx.execute(
            "UPDATE runs SET status = ?, stop_reason = NULL, ended_at = NULL
             WHERE id = ? AND conv_id = ? AND status = ?",
            params![to, run_id, conv_id, from],
        )?;
        if updated > 0 {
            let conversation_updated = tx.execute(
                "UPDATE conversations SET busy = ?, status = ?, phase = 'open', updated_at = ?
                 WHERE id = ? AND busy = ?",
                params![to, to, now_iso(), conv_id, from],
            )?;
            if conversation_updated != 1 {
                return Err(HubError::other(format!(
                    "conversation {conv_id} lost {from} state while transitioning run {run_id}"
                )));
            }
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
             SET busy = 'none', last_outcome = 'failed', status = 'failed',
                 phase = 'open', updated_at = ?
             WHERE busy IN ('running','cancelling') OR status IN ('running','cancelling')",
            params![now_iso()],
        )?;
        tx.commit()?;
        Ok(recovered)
    }

    /// Close while busy: phase=closed, busy=none, last_outcome=failed (PHASE1 §4.4).
    pub fn close_conversation_local(&self, conv_id: &str, was_busy: bool) -> Result<(), HubError> {
        let conn = self.conn.lock();
        if was_busy {
            let status = super::mirror_status(
                conversation_policy::ConvPhase::Closed,
                conversation_policy::ConvBusy::None,
                conversation_policy::LastOutcome::Failed,
            );
            conn.execute(
                "UPDATE conversations SET phase = 'closed', busy = 'none',
                    last_outcome = 'failed', status = ?, updated_at = ?
                 WHERE id = ?",
                params![status.as_str(), now_iso(), conv_id],
            )?;
        } else {
            let outcome: String = conn
                .query_row(
                    "SELECT last_outcome FROM conversations WHERE id = ?",
                    params![conv_id],
                    |r| r.get(0),
                )
                .optional()?
                .unwrap_or_else(|| "none".into());
            let last = conversation_policy::LastOutcome::parse(&outcome)
                .unwrap_or(conversation_policy::LastOutcome::None);
            let status = super::mirror_status(
                conversation_policy::ConvPhase::Closed,
                conversation_policy::ConvBusy::None,
                last,
            );
            conn.execute(
                "UPDATE conversations SET phase = 'closed', busy = 'none', status = ?, updated_at = ?
                 WHERE id = ?",
                params![status.as_str(), now_iso(), conv_id],
            )?;
        }
        Ok(())
    }

    pub fn active_run_id(&self, conv_id: &str) -> Result<Option<String>, HubError> {
        let conn = self.conn.lock();
        Ok(conn
            .query_row(
                "SELECT id FROM runs
                 WHERE conv_id = ? AND status IN ('running','cancelling')
                 ORDER BY started_at DESC LIMIT 1",
                params![conv_id],
                |r| r.get(0),
            )
            .optional()?)
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
        #[cfg(test)]
        if self
            .fail_append_message_once
            .swap(false, std::sync::atomic::Ordering::AcqRel)
        {
            return Err(HubError::other("injected append_message failure"));
        }
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
