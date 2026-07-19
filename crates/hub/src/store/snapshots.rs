use super::*;

impl Store {
    #[cfg(test)]
    pub(crate) fn fail_next_static_snapshot_for_test(&self) {
        self.fail_static_snapshot_once
            .store(true, std::sync::atomic::Ordering::Release);
    }

    // --- snapshots ---------------------------------------------------------

    pub fn replace_static_snapshots(
        &self,
        conv_id: &str,
        config_options: Option<&serde_json::Value>,
        modes: Option<&serde_json::Value>,
    ) -> Result<(), HubError> {
        #[cfg(test)]
        if self
            .fail_static_snapshot_once
            .swap(false, std::sync::atomic::Ordering::AcqRel)
        {
            return Err(HubError::other("injected static snapshot failure"));
        }
        let mut conn = self.conn.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "DELETE FROM config_snapshots WHERE conv_id = ?",
            params![conv_id],
        )?;
        tx.execute(
            "DELETE FROM plan_snapshots WHERE conv_id = ?",
            params![conv_id],
        )?;
        tx.execute(
            "DELETE FROM available_command_snapshots WHERE conv_id = ?",
            params![conv_id],
        )?;
        tx.execute(
            "DELETE FROM usage_snapshots WHERE conv_id = ?",
            params![conv_id],
        )?;
        if config_options.is_some() || modes.is_some() {
            let config_json =
                serde_json::to_string(config_options.unwrap_or(&serde_json::Value::Null))?;
            let modes_json = modes.map(serde_json::to_string).transpose()?;
            tx.execute(
                "INSERT INTO config_snapshots(
                     conv_id, config_options_json, modes_json, updated_at
                 ) VALUES (?, ?, ?, ?)",
                params![conv_id, config_json, modes_json, now_iso()],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn set_config_snapshot(
        &self,
        conv_id: &str,
        config_options: &serde_json::Value,
        modes: Option<&serde_json::Value>,
    ) -> Result<(), HubError> {
        let conn = self.conn.lock();
        let opts = serde_json::to_string(config_options)?;
        let modes = match modes {
            Some(v) => Some(serde_json::to_string(v)?),
            None => None,
        };
        conn.execute(
            "INSERT INTO config_snapshots(conv_id, config_options_json, modes_json, updated_at)
             VALUES (?, ?, ?, ?)
             ON CONFLICT(conv_id) DO UPDATE SET
               config_options_json = excluded.config_options_json,
               modes_json = COALESCE(
                   excluded.modes_json,
                   config_snapshots.modes_json
               ),
               updated_at = excluded.updated_at",
            params![conv_id, opts, modes, now_iso()],
        )?;
        Ok(())
    }

    pub fn set_plan_snapshot(
        &self,
        conv_id: &str,
        entries: &serde_json::Value,
    ) -> Result<(), HubError> {
        replace_json_snapshot(
            &self.conn.lock(),
            "plan_snapshots",
            "entries_json",
            conv_id,
            entries,
        )
    }

    pub fn set_available_commands_snapshot(
        &self,
        conv_id: &str,
        commands: &serde_json::Value,
    ) -> Result<(), HubError> {
        replace_json_snapshot(
            &self.conn.lock(),
            "available_command_snapshots",
            "commands_json",
            conv_id,
            commands,
        )
    }

    pub fn upsert_usage_snapshot(
        &self,
        conv_id: &str,
        used: i64,
        size: i64,
        cost: Option<&serde_json::Value>,
    ) -> Result<(), HubError> {
        let conn = self.conn.lock();
        let cost = match cost {
            Some(v) => Some(serde_json::to_string(v)?),
            None => None,
        };
        conn.execute(
            "INSERT INTO usage_snapshots(conv_id, used, size, cost_json, updated_at)
             VALUES (?, ?, ?, ?, ?)
             ON CONFLICT(conv_id) DO UPDATE SET
               used = excluded.used, size = excluded.size,
               cost_json = excluded.cost_json, updated_at = excluded.updated_at",
            params![conv_id, used, size, cost, now_iso()],
        )?;
        Ok(())
    }

    pub fn config_snapshot(&self, conv_id: &str) -> Result<Option<serde_json::Value>, HubError> {
        Ok(snapshot_json(
            &self.conn.lock(),
            "config_snapshots",
            "config_options_json",
            conv_id,
        )?
        .filter(|value| !value.is_null()))
    }

    pub fn modes_snapshot(&self, conv_id: &str) -> Result<Option<serde_json::Value>, HubError> {
        snapshot_json(&self.conn.lock(), "config_snapshots", "modes_json", conv_id)
    }

    pub fn plan_snapshot(&self, conv_id: &str) -> Result<Option<serde_json::Value>, HubError> {
        snapshot_json(&self.conn.lock(), "plan_snapshots", "entries_json", conv_id)
    }

    pub fn commands_snapshot(&self, conv_id: &str) -> Result<Option<serde_json::Value>, HubError> {
        snapshot_json(
            &self.conn.lock(),
            "available_command_snapshots",
            "commands_json",
            conv_id,
        )
    }

    pub fn usage_snapshot(&self, conv_id: &str) -> Result<Option<serde_json::Value>, HubError> {
        let conn = self.conn.lock();
        let row = conn
            .query_row(
                "SELECT used, size, cost_json FROM usage_snapshots WHERE conv_id = ?",
                params![conv_id],
                |r| {
                    Ok(serde_json::json!({
                        "used": r.get::<_, i64>(0)?,
                        "size": r.get::<_, i64>(1)?,
                        "cost": r.get::<_, Option<String>>(2)?
                            .map(|s| parse_sql_json::<serde_json::Value>(2, &s))
                            .transpose()?,
                    }))
                },
            )
            .optional()?;
        Ok(row)
    }

    // --- search ------------------------------------------------------------

    pub fn search(
        &self,
        query: &str,
        agent_id: Option<&str>,
        conv_id: Option<&str>,
        limit: usize,
        offset: usize,
    ) -> Result<SearchPage, HubError> {
        if limit == 0 {
            return Ok(SearchPage {
                items: Vec::new(),
                next_offset: None,
            });
        }
        let limit = limit.min(500);
        let offset_for_next = offset;
        let Ok(sql_offset) = i64::try_from(offset) else {
            return Ok(SearchPage {
                items: Vec::new(),
                next_offset: None,
            });
        };
        let conn = self.conn.lock();
        let fts = sanitize_fts(query);
        let mut sql = String::from(
            "WITH hits AS (
                 SELECT 'message' AS kind,
                        bm25(messages_fts) AS rank,
                        conversations.agent_id AS agent_id,
                        messages_fts.conv_id AS conv_id,
                        conversations.title AS conv_title,
                        messages_fts.message_id AS message_id,
                        m.run_id AS run_id,
                        m.seq AS seq,
                        m.role AS role,
                        m.source ||
                            CASE WHEN m.current_projection = 1 THEN '' ELSE ':audit' END
                            AS source,
                        m.created_at AS created_at,
                        snippet(messages_fts, 2, '[', ']', '…', 18) AS snippet
                 FROM messages_fts
                 JOIN messages m ON m.id = messages_fts.message_id
                 JOIN conversations ON conversations.id = messages_fts.conv_id
                 WHERE messages_fts MATCH ?
                   AND conversations.status != 'deleted'",
        );
        let mut pv: Vec<rusqlite::types::Value> = vec![rusqlite::types::Value::Text(fts.clone())];
        if let Some(a) = agent_id {
            sql.push_str(" AND conversations.agent_id = ?");
            pv.push(rusqlite::types::Value::Text(a.to_string()));
        }
        if let Some(c) = conv_id {
            sql.push_str(" AND messages_fts.conv_id = ?");
            pv.push(rusqlite::types::Value::Text(c.to_string()));
        }
        sql.push_str(
            " UNION ALL
                 SELECT 'conversation' AS kind,
                        bm25(conversations_fts) AS rank,
                        conversations.agent_id AS agent_id,
                        conversations_fts.conv_id AS conv_id,
                        conversations.title AS conv_title,
                        NULL AS message_id,
                        NULL AS run_id,
                        NULL AS seq,
                        NULL AS role,
                        NULL AS source,
                        conversations.updated_at AS created_at,
                        snippet(conversations_fts, 1, '[', ']', '…', 18) AS snippet
                 FROM conversations_fts
                 JOIN conversations ON conversations.id = conversations_fts.conv_id
                 WHERE conversations_fts MATCH ?
                   AND conversations.status != 'deleted'",
        );
        pv.push(rusqlite::types::Value::Text(fts));
        if let Some(a) = agent_id {
            sql.push_str(" AND conversations.agent_id = ?");
            pv.push(rusqlite::types::Value::Text(a.to_string()));
        }
        if let Some(c) = conv_id {
            sql.push_str(" AND conversations_fts.conv_id = ?");
            pv.push(rusqlite::types::Value::Text(c.to_string()));
        }
        sql.push_str(
            " )
             SELECT kind, rank, agent_id, conv_id, conv_title, message_id,
                    run_id, seq, role, source, created_at, snippet
             FROM hits
             ORDER BY rank ASC, kind ASC, conv_id ASC, COALESCE(message_id, '')
             LIMIT ? OFFSET ?",
        );
        pv.push(rusqlite::types::Value::Integer((limit + 1) as i64));
        pv.push(rusqlite::types::Value::Integer(sql_offset));
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(pv.iter()), |r| {
            Ok(SearchHit {
                kind: r.get(0)?,
                rank: r.get(1)?,
                agent_id: r.get(2)?,
                conv_id: r.get(3)?,
                conv_title: r.get(4)?,
                message_id: r.get(5)?,
                run_id: r.get(6)?,
                seq: r.get(7)?,
                role: r.get(8)?,
                source: r.get(9)?,
                created_at: r.get(10)?,
                snippet: r.get(11)?,
            })
        })?;
        let mut items = Vec::new();
        for r in rows {
            items.push(r?);
        }
        let has_more = items.len() > limit;
        items.truncate(limit);
        let next_offset = has_more.then(|| offset_for_next.saturating_add(limit));
        Ok(SearchPage { items, next_offset })
    }
}
