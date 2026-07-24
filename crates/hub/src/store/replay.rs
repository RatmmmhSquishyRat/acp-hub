use super::*;

impl Store {
    /// Begin a streamed `session/load` refresh.
    ///
    /// The previous Layer 1 projection remains current while the remote load is
    /// in flight. New replay rows are appended after `starting_seq`; commit
    /// atomically supersedes the prior Layer 1, while rollback removes the new
    /// rows. This ordering ensures a daemon crash cannot hide the last complete
    /// replay snapshot.
    pub fn begin_load_replay(
        &self,
        conv_id: &str,
        load_id: &str,
    ) -> Result<ReplayRefresh, HubError> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let refresh = Self::begin_load_replay_tx(&tx, conv_id, load_id, true)?;
        tx.commit()?;
        Ok(refresh)
    }

    pub(super) fn begin_load_replay_tx(
        tx: &Transaction<'_>,
        conv_id: &str,
        load_id: &str,
        conversation_was_present: bool,
    ) -> Result<ReplayRefresh, HubError> {
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM conversations WHERE id = ?)",
            params![conv_id],
            |r| r.get(0),
        )?;
        if !exists {
            return Err(HubError::not_found("conversation", conv_id));
        }
        let starting_seq = tx.query_row(
            "SELECT COALESCE(MAX(seq), 0) FROM messages WHERE conv_id = ?",
            params![conv_id],
            |r| r.get(0),
        )?;
        let generation_nonce = uuid::Uuid::new_v4().simple().to_string();
        let started_at = now_iso();
        tx.execute(
            "INSERT INTO load_replay_refreshes(
                 conv_id, load_id, starting_seq, started_at, generation_nonce
             ) VALUES (?, ?, ?, ?, ?)",
            params![
                conv_id,
                load_id,
                starting_seq,
                started_at.as_str(),
                generation_nonce.as_str()
            ],
        )?;
        tx.execute(
            "INSERT INTO load_replay_projection_before_images(
                 load_id,
                 conv_id,
                 conversation_title,
                 conversation_updated_at,
                 session_meta_json,
                 fts_title_present,
                 fts_title,
                 config_present,
                 config_options_json,
                 config_modes_json,
                 config_updated_at,
                 plan_present,
                 plan_entries_json,
                 plan_updated_at,
                 commands_present,
                 commands_json,
                 commands_updated_at,
                 usage_present,
                 usage_used,
                 usage_size,
                 usage_cost_json,
                 usage_updated_at,
                 conversation_was_present,
                 conversation_status,
                 conversation_cwd,
                 conversation_directories_json
             )
             SELECT ?,
                    c.id,
                    c.title,
                    c.updated_at,
                    c.session_meta_json,
                    EXISTS(
                        SELECT 1 FROM conversations_fts f WHERE f.conv_id = c.id
                    ),
                    (
                        SELECT f.title FROM conversations_fts f
                        WHERE f.conv_id = c.id LIMIT 1
                    ),
                    EXISTS(
                        SELECT 1 FROM config_snapshots s WHERE s.conv_id = c.id
                    ),
                    (
                        SELECT s.config_options_json FROM config_snapshots s
                        WHERE s.conv_id = c.id
                    ),
                    (
                        SELECT s.modes_json FROM config_snapshots s
                        WHERE s.conv_id = c.id
                    ),
                    (
                        SELECT s.updated_at FROM config_snapshots s
                        WHERE s.conv_id = c.id
                    ),
                    EXISTS(
                        SELECT 1 FROM plan_snapshots s WHERE s.conv_id = c.id
                    ),
                    (
                        SELECT s.entries_json FROM plan_snapshots s
                        WHERE s.conv_id = c.id
                    ),
                    (
                        SELECT s.updated_at FROM plan_snapshots s
                        WHERE s.conv_id = c.id
                    ),
                    EXISTS(
                        SELECT 1 FROM available_command_snapshots s
                        WHERE s.conv_id = c.id
                    ),
                    (
                        SELECT s.commands_json FROM available_command_snapshots s
                        WHERE s.conv_id = c.id
                    ),
                    (
                        SELECT s.updated_at FROM available_command_snapshots s
                        WHERE s.conv_id = c.id
                    ),
                    EXISTS(
                        SELECT 1 FROM usage_snapshots s WHERE s.conv_id = c.id
                    ),
                    (
                        SELECT s.used FROM usage_snapshots s
                        WHERE s.conv_id = c.id
                    ),
                    (
                        SELECT s.size FROM usage_snapshots s
                        WHERE s.conv_id = c.id
                    ),
                    (
                        SELECT s.cost_json FROM usage_snapshots s
                        WHERE s.conv_id = c.id
                    ),
                    (
                        SELECT s.updated_at FROM usage_snapshots s
                        WHERE s.conv_id = c.id
                    ),
                    ?,
                    c.status,
                    c.cwd,
                    c.additional_directories_json
             FROM conversations c
             WHERE c.id = ?",
            params![load_id, conversation_was_present, conv_id],
        )?;
        Ok(ReplayRefresh {
            conv_id: conv_id.to_string(),
            load_id: load_id.to_string(),
            starting_seq,
            generation_nonce,
        })
    }

    /// Begin an agent-discovery import and its replay refresh in one durable
    /// transaction. Existing conversation metadata/FTS is captured before the
    /// update; a newly discovered row is marked provisional so rollback or
    /// startup recovery removes it completely.
    pub fn begin_agent_session_import(
        &self,
        import: AgentSessionImport<'_>,
    ) -> Result<(String, ReplayRefresh), HubError> {
        let AgentSessionImport {
            provisional_conv_id,
            agent_id,
            agent_session_id,
            title,
            cwd,
            additional_directories,
            load_id,
        } = import;
        let mut conn = self.conn.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let existing_id: Option<String> = tx
            .query_row(
                "SELECT id FROM conversations
                 WHERE agent_id = ? AND agent_session_id = ?",
                params![agent_id, agent_session_id],
                |row| row.get(0),
            )
            .optional()?;
        let dirs = serde_json::to_string(additional_directories)?;
        let ts = now_iso();
        let (conv_id, existed) = match existing_id {
            Some(conv_id) => (conv_id, true),
            None => {
                tx.execute(
                    "INSERT INTO conversations(
                         id, agent_id, agent_session_id, title, status,
                         cwd, additional_directories_json, session_meta_json,
                         created_at, updated_at,
                         origin, interaction, phase, busy, last_outcome)
                     VALUES (?, ?, ?, ?, 'idle', ?, ?, NULL, ?, ?,
                             'imported_list', 'read_only', 'open', 'none', 'none')",
                    params![
                        provisional_conv_id,
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
                    params![provisional_conv_id, title.unwrap_or("")],
                )?;
                (provisional_conv_id.to_string(), false)
            }
        };
        let refresh = Self::begin_load_replay_tx(&tx, &conv_id, load_id, existed)?;
        if existed {
            tx.execute(
                "UPDATE conversations
                 SET title = ?, cwd = ?, additional_directories_json = ?, updated_at = ?
                 WHERE id = ?",
                params![title, cwd, dirs, ts, conv_id],
            )?;
            tx.execute(
                "DELETE FROM conversations_fts WHERE conv_id = ?",
                params![conv_id],
            )?;
            tx.execute(
                "INSERT INTO conversations_fts(conv_id, title) VALUES (?, ?)",
                params![conv_id, title.unwrap_or("")],
            )?;
        }
        tx.commit()?;
        Ok((conv_id, refresh))
    }

    fn validate_load_replay_refresh(
        tx: &Transaction<'_>,
        refresh: &ReplayRefresh,
    ) -> Result<(), HubError> {
        let exact_marker_exists: bool = tx.query_row(
            "SELECT EXISTS(
                 SELECT 1 FROM load_replay_refreshes
                 WHERE conv_id = ?
                   AND load_id = ?
                   AND starting_seq = ?
                   AND generation_nonce = ?
             )",
            params![
                refresh.conv_id.as_str(),
                refresh.load_id.as_str(),
                refresh.starting_seq,
                refresh.generation_nonce.as_str()
            ],
            |row| row.get(0),
        )?;
        if !exact_marker_exists {
            return Err(HubError::Conflict(refresh.conv_id.clone()));
        }
        Ok(())
    }

    /// Commit a streamed replay refresh.
    ///
    /// Notification rows are already durable. Superseding the previous Layer 1
    /// happens only here, after the remote load and snapshot persistence have
    /// succeeded.
    pub fn commit_load_replay<R>(&self, refresh: R) -> Result<(), HubError>
    where
        R: Borrow<ReplayRefresh>,
    {
        self.commit_load_replay_inner(refresh.borrow(), None)
    }

    /// Commit a remote load/new refresh and atomically replace its complete
    /// static snapshot set. Config options and modes have independent
    /// presence; plan/commands/usage rows that were not updated during this
    /// refresh cease to be current.
    pub fn commit_load_replay_with_static<R>(
        &self,
        refresh: R,
        config_options: Option<&serde_json::Value>,
        modes: Option<&serde_json::Value>,
    ) -> Result<(), HubError>
    where
        R: Borrow<ReplayRefresh>,
    {
        self.commit_load_replay_inner(refresh.borrow(), Some((config_options, modes)))
    }

    fn commit_load_replay_inner(
        &self,
        refresh: &ReplayRefresh,
        static_snapshots: Option<(Option<&serde_json::Value>, Option<&serde_json::Value>)>,
    ) -> Result<(), HubError> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        Self::validate_load_replay_refresh(&tx, refresh)?;
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM conversations WHERE id = ?)",
            params![refresh.conv_id.as_str()],
            |row| row.get(0),
        )?;
        if !exists {
            return Err(HubError::not_found("conversation", refresh.conv_id.clone()));
        }
        if let Some((config_options, modes)) = static_snapshots {
            for (table, before_column) in [
                ("plan_snapshots", "plan_updated_at"),
                ("available_command_snapshots", "commands_updated_at"),
                ("usage_snapshots", "usage_updated_at"),
            ] {
                tx.execute(
                    &format!(
                        "DELETE FROM {table}
                         WHERE conv_id = ?
                           AND updated_at = (
                               SELECT {before_column}
                               FROM load_replay_projection_before_images
                               WHERE conv_id = ? AND load_id = ?
                           )"
                    ),
                    params![
                        refresh.conv_id.as_str(),
                        refresh.conv_id.as_str(),
                        refresh.load_id.as_str()
                    ],
                )?;
            }
            tx.execute(
                "DELETE FROM config_snapshots WHERE conv_id = ?",
                params![refresh.conv_id.as_str()],
            )?;
            if config_options.is_some() || modes.is_some() {
                let config_json =
                    serde_json::to_string(config_options.unwrap_or(&serde_json::Value::Null))?;
                let modes_json = modes.map(serde_json::to_string).transpose()?;
                tx.execute(
                    "INSERT INTO config_snapshots(
                         conv_id, config_options_json, modes_json, updated_at
                     ) VALUES (?, ?, ?, ?)",
                    params![refresh.conv_id.as_str(), config_json, modes_json, now_iso()],
                )?;
            }
        }
        tx.execute(
            "UPDATE messages
             SET current_projection = 0, superseded_by_load_id = ?
             WHERE conv_id = ?
               AND source = 'load_replay'
               AND current_projection = 1
               AND seq <= ?",
            params![
                refresh.load_id.as_str(),
                refresh.conv_id.as_str(),
                refresh.starting_seq
            ],
        )?;
        tx.execute(
            "UPDATE conversations
             SET projection_generation = projection_generation + 1
             WHERE id = ?",
            params![refresh.conv_id.as_str()],
        )?;
        tx.execute(
            "DELETE FROM load_replay_refreshes
             WHERE conv_id = ? AND load_id = ?",
            params![refresh.conv_id.as_str(), refresh.load_id.as_str()],
        )?;
        tx.commit()?;
        Ok(())
    }

    fn restore_load_replay_projection(
        tx: &Transaction<'_>,
        conv_id: &str,
        load_id: &str,
    ) -> Result<(), HubError> {
        let before_image_exists: bool = tx.query_row(
            "SELECT EXISTS(
                 SELECT 1 FROM load_replay_projection_before_images
                 WHERE conv_id = ? AND load_id = ?
             )",
            params![conv_id, load_id],
            |row| row.get(0),
        )?;
        if !before_image_exists {
            return Ok(());
        }
        let conversation_was_present: bool = tx.query_row(
            "SELECT conversation_was_present
             FROM load_replay_projection_before_images
             WHERE conv_id = ? AND load_id = ?",
            params![conv_id, load_id],
            |row| row.get(0),
        )?;
        if !conversation_was_present {
            tx.execute(
                "DELETE FROM messages_fts WHERE conv_id = ?",
                params![conv_id],
            )?;
            tx.execute(
                "DELETE FROM conversations_fts WHERE conv_id = ?",
                params![conv_id],
            )?;
            tx.execute("DELETE FROM conversations WHERE id = ?", params![conv_id])?;
            return Ok(());
        }

        tx.execute(
            "UPDATE conversations
             SET title = (
                     SELECT conversation_title
                     FROM load_replay_projection_before_images
                     WHERE conv_id = ? AND load_id = ?
                 ),
                 updated_at = (
                     SELECT conversation_updated_at
                     FROM load_replay_projection_before_images
                     WHERE conv_id = ? AND load_id = ?
                 ),
                 session_meta_json = (
                     SELECT session_meta_json
                     FROM load_replay_projection_before_images
                     WHERE conv_id = ? AND load_id = ?
                 ),
                 status = (
                     SELECT conversation_status
                     FROM load_replay_projection_before_images
                     WHERE conv_id = ? AND load_id = ?
                 ),
                 cwd = (
                     SELECT conversation_cwd
                     FROM load_replay_projection_before_images
                     WHERE conv_id = ? AND load_id = ?
                 ),
                 additional_directories_json = (
                     SELECT conversation_directories_json
                     FROM load_replay_projection_before_images
                     WHERE conv_id = ? AND load_id = ?
                 )
             WHERE id = ?",
            params![
                conv_id, load_id, conv_id, load_id, conv_id, load_id, conv_id, load_id, conv_id,
                load_id, conv_id, load_id, conv_id
            ],
        )?;
        tx.execute(
            "DELETE FROM conversations_fts WHERE conv_id = ?",
            params![conv_id],
        )?;
        tx.execute(
            "INSERT INTO conversations_fts(conv_id, title)
             SELECT conv_id, fts_title
             FROM load_replay_projection_before_images
             WHERE conv_id = ? AND load_id = ? AND fts_title_present = 1",
            params![conv_id, load_id],
        )?;

        tx.execute(
            "DELETE FROM config_snapshots WHERE conv_id = ?",
            params![conv_id],
        )?;
        tx.execute(
            "INSERT INTO config_snapshots(
                 conv_id, config_options_json, modes_json, updated_at
             )
             SELECT conv_id, config_options_json, config_modes_json, config_updated_at
             FROM load_replay_projection_before_images
             WHERE conv_id = ? AND load_id = ? AND config_present = 1",
            params![conv_id, load_id],
        )?;

        tx.execute(
            "DELETE FROM plan_snapshots WHERE conv_id = ?",
            params![conv_id],
        )?;
        tx.execute(
            "INSERT INTO plan_snapshots(conv_id, entries_json, updated_at)
             SELECT conv_id, plan_entries_json, plan_updated_at
             FROM load_replay_projection_before_images
             WHERE conv_id = ? AND load_id = ? AND plan_present = 1",
            params![conv_id, load_id],
        )?;

        tx.execute(
            "DELETE FROM available_command_snapshots WHERE conv_id = ?",
            params![conv_id],
        )?;
        tx.execute(
            "INSERT INTO available_command_snapshots(conv_id, commands_json, updated_at)
             SELECT conv_id, commands_json, commands_updated_at
             FROM load_replay_projection_before_images
             WHERE conv_id = ? AND load_id = ? AND commands_present = 1",
            params![conv_id, load_id],
        )?;

        tx.execute(
            "DELETE FROM usage_snapshots WHERE conv_id = ?",
            params![conv_id],
        )?;
        tx.execute(
            "INSERT INTO usage_snapshots(
                 conv_id, used, size, cost_json, updated_at
             )
             SELECT conv_id, usage_used, usage_size, usage_cost_json, usage_updated_at
             FROM load_replay_projection_before_images
             WHERE conv_id = ? AND load_id = ? AND usage_present = 1",
            params![conv_id, load_id],
        )?;
        Ok(())
    }

    /// Roll back a failed streamed replay refresh.
    ///
    /// Newly captured Layer 1 rows are removed from both the base table and
    /// FTS, then the exact previous Layer 1 projection is restored. Layer 2 is
    /// never changed.
    pub fn rollback_load_replay<R>(&self, refresh: R) -> Result<(), HubError>
    where
        R: Borrow<ReplayRefresh>,
    {
        let refresh = refresh.borrow();
        let mut conn = self.conn.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        Self::validate_load_replay_refresh(&tx, refresh)?;
        tx.execute(
            "DELETE FROM messages_fts
             WHERE message_id IN (
                 SELECT id FROM messages
                 WHERE conv_id = ?
                   AND source = 'load_replay'
                   AND seq > ?
             )",
            params![refresh.conv_id.as_str(), refresh.starting_seq],
        )?;
        let deleted_messages = tx.execute(
            "DELETE FROM messages
             WHERE conv_id = ?
               AND source = 'load_replay'
               AND seq > ?",
            params![refresh.conv_id.as_str(), refresh.starting_seq],
        )?;
        Self::restore_load_replay_projection(
            &tx,
            refresh.conv_id.as_str(),
            refresh.load_id.as_str(),
        )?;
        if deleted_messages > 0 {
            tx.execute(
                "UPDATE conversations
                 SET projection_generation = projection_generation + 1
                 WHERE id = ?",
                params![refresh.conv_id.as_str()],
            )?;
        }
        tx.execute(
            "DELETE FROM load_replay_refreshes
             WHERE conv_id = ? AND load_id = ?",
            params![refresh.conv_id.as_str(), refresh.load_id.as_str()],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Restore partial Layer 1 rows and projection snapshots left by a daemon
    /// crash during `session/load`.
    ///
    /// The last complete Layer 1 and the projection before-image both remain
    /// durable until commit.
    pub fn recover_interrupted_load_replays(&self) -> Result<usize, HubError> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let refreshes = {
            let mut stmt = tx.prepare(
                "SELECT conv_id, load_id, starting_seq
                 FROM load_replay_refreshes",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            })?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        for (conv_id, load_id, starting_seq) in &refreshes {
            let has_before_image: bool = tx.query_row(
                "SELECT EXISTS(
                     SELECT 1 FROM load_replay_projection_before_images
                     WHERE conv_id = ? AND load_id = ?
                 )",
                params![conv_id, load_id],
                |row| row.get(0),
            )?;
            tx.execute(
                "DELETE FROM messages_fts
                 WHERE message_id IN (
                     SELECT id FROM messages
                     WHERE conv_id = ?
                       AND source = 'load_replay'
                       AND seq > ?
                 )",
                params![conv_id, starting_seq],
            )?;
            let deleted_messages = tx.execute(
                "DELETE FROM messages
                 WHERE conv_id = ?
                   AND source = 'load_replay'
                   AND seq > ?",
                params![conv_id, starting_seq],
            )?;
            if !has_before_image {
                tx.execute(
                    "UPDATE messages
                     SET current_projection = 1, superseded_by_load_id = NULL
                     WHERE conv_id = ?
                       AND source = 'load_replay'
                       AND current_projection = 0
                       AND seq <= ?
                       AND superseded_by_load_id = ?",
                    params![conv_id, starting_seq, load_id],
                )?;
            }
            Self::restore_load_replay_projection(&tx, conv_id, load_id)?;
            if deleted_messages > 0 {
                tx.execute(
                    "UPDATE conversations
                     SET projection_generation = projection_generation + 1
                     WHERE id = ?",
                    params![conv_id],
                )?;
            }
        }
        tx.execute("DELETE FROM load_replay_refreshes", [])?;
        tx.commit()?;
        Ok(refreshes.len())
    }

    /// Non-destructive `session/load` replay: insert new `load_replay` rows as
    /// the current Layer 1 projection and supersede only prior Layer 1 rows.
    /// Hub-captured Layer 2 rows remain current and independently visible.
    pub fn stage_load_replay(
        &self,
        conv_id: &str,
        load_id: &str,
        messages: &[ReplayedMessage],
    ) -> Result<(), HubError> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "UPDATE messages SET current_projection = 0, superseded_by_load_id = ?
             WHERE conv_id = ?
               AND source = 'load_replay'
               AND current_projection = 1",
            params![load_id, conv_id],
        )?;
        let mut seq: i64 = tx.query_row(
            "SELECT COALESCE(MAX(seq), 0) FROM messages WHERE conv_id = ?",
            params![conv_id],
            |r| r.get(0),
        )?;
        for m in messages {
            seq += 1;
            let content = serde_json::to_string(&m.content_json)?;
            tx.execute(
                "INSERT INTO messages(
                     id, conv_id, run_id, source, current_projection, message_key,
                     superseded_by_load_id, role, kind, content_json, body_text,
                     seq, created_at)
                 VALUES (?, ?, NULL, 'load_replay', 1, ?, NULL, ?, ?, ?, ?, ?, ?)",
                params![
                    m.id,
                    conv_id,
                    m.message_key,
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
                params![m.id, conv_id, m.body_text],
            )?;
        }
        tx.execute(
            "UPDATE conversations
             SET projection_generation = projection_generation + 1,
                 updated_at = ?
             WHERE id = ?",
            params![now_iso(), conv_id],
        )?;
        tx.commit()?;
        Ok(())
    }
}
