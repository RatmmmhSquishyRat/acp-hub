use super::*;

impl Store {
    pub fn messages(
        &self,
        conv_id: &str,
        include_audit: bool,
    ) -> Result<Vec<MessageRow>, HubError> {
        let conn = self.conn.lock();
        let filter = if include_audit {
            ""
        } else {
            " AND current_projection = 1"
        };
        let sql = format!(
            "SELECT id, conv_id, run_id, source, current_projection, message_key,
                    role, kind, content_json, body_text, seq, created_at
             FROM messages WHERE conv_id = ?{filter} ORDER BY seq ASC"
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params![conv_id], map_message)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    pub fn messages_page(
        &self,
        conv_id: &str,
        include_audit: bool,
        run_id: Option<&str>,
        after_seq: Option<i64>,
        limit: usize,
        offset: usize,
    ) -> Result<MessagePage, HubError> {
        self.messages_page_query(MessagePageQuery {
            conv_id,
            include_audit,
            run_id,
            after_seq,
            cursor: None,
            limit,
            offset,
        })
    }

    /// Return one bounded message page.
    ///
    /// A continuation cursor is authenticated and binds the conversation,
    /// projection generation, sort key, audit visibility, run filter, and
    /// initial sequence filter. Reusing it with another query is invalid;
    /// changing the replay projection makes it explicitly stale.
    pub fn messages_page_query(
        &self,
        query: MessagePageQuery<'_>,
    ) -> Result<MessagePage, HubError> {
        let MessagePageQuery {
            conv_id,
            include_audit,
            run_id,
            after_seq,
            cursor,
            limit,
            offset,
        } = query;
        if limit == 0 {
            return Err(HubError::other("message page limit must be positive"));
        }
        if cursor.is_some() && offset != 0 {
            return Err(HubError::invalid_cursor(
                "offset cannot be combined with a continuation cursor",
            ));
        }
        let limit = limit.min(MAX_MESSAGE_PAGE_ROWS);
        let conn = self.conn.lock();
        let generation: i64 = conn
            .query_row(
                "SELECT projection_generation FROM conversations WHERE id = ?",
                params![conv_id],
                |row| row.get(0),
            )
            .optional()?
            .ok_or_else(|| HubError::not_found("conversation", conv_id))?;
        let cursor_payload = cursor
            .map(|cursor| decode_message_cursor(&conn, cursor))
            .transpose()?;
        if let Some(payload) = &cursor_payload {
            if payload.conversation != conv_id
                || payload.include_audit != include_audit
                || payload.run_id.as_deref() != run_id
                || payload.start_after_seq != after_seq
            {
                return Err(HubError::invalid_cursor(
                    "cursor does not belong to this message query",
                ));
            }
            if payload.generation != generation {
                return Err(HubError::StaleCursor {
                    conv_id: conv_id.to_string(),
                    expected_generation: payload.generation,
                    current_generation: generation,
                });
            }
            if after_seq.is_some_and(|start| payload.last_key <= start) {
                return Err(HubError::invalid_cursor(
                    "cursor sort key is outside the message query",
                ));
            }
        }
        let page_after_seq = cursor_payload
            .as_ref()
            .map(|payload| payload.last_key)
            .or(after_seq);
        let filter = if include_audit {
            ""
        } else {
            " AND current_projection = 1"
        };
        let total: i64 = conn.query_row(
            &format!(
                "SELECT COUNT(*) FROM messages
                 WHERE conv_id = ?{filter}
                   AND (? IS NULL OR run_id = ?)
                   AND (? IS NULL OR seq > ?)"
            ),
            params![conv_id, run_id, run_id, after_seq, after_seq],
            |row| row.get(0),
        )?;
        let total = usize::try_from(total).unwrap_or(usize::MAX);
        let Ok(sql_offset) = i64::try_from(offset) else {
            return Ok(MessagePage {
                items: Vec::new(),
                next_cursor: None,
                next_offset: None,
                total,
            });
        };
        let sql = format!(
            "WITH page_candidates AS (
                 SELECT id, conv_id, run_id, source, current_projection, message_key,
                        role, kind, content_json, body_text, seq, created_at,
                        length(CAST(content_json AS BLOB))
                            + length(CAST(body_text AS BLOB)) + 512 AS row_bytes
                 FROM messages
                 WHERE conv_id = ?{filter}
                   AND (? IS NULL OR run_id = ?)
                   AND (? IS NULL OR seq > ?)
                 ORDER BY seq ASC LIMIT ? OFFSET ?
             ),
             budgeted AS (
                 SELECT *,
                        SUM(row_bytes) OVER (ORDER BY seq ASC) AS cumulative_bytes
                 FROM page_candidates
             )
             SELECT id, conv_id, run_id, source, current_projection, message_key,
                    role, kind, content_json, body_text, seq, created_at
             FROM budgeted
             WHERE cumulative_bytes <= ?
             ORDER BY seq ASC"
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(
            params![
                conv_id,
                run_id,
                run_id,
                page_after_seq,
                page_after_seq,
                limit as i64,
                sql_offset,
                MAX_MESSAGE_PAGE_BYTES
            ],
            map_message,
        )?;
        let mut items = Vec::new();
        for row in rows {
            items.push(row?);
        }
        let remaining_before_page: i64 = conn.query_row(
            &format!(
                "SELECT COUNT(*) FROM messages
                 WHERE conv_id = ?{filter}
                   AND (? IS NULL OR run_id = ?)
                   AND (? IS NULL OR seq > ?)"
            ),
            params![conv_id, run_id, run_id, page_after_seq, page_after_seq],
            |row| row.get(0),
        )?;
        let remaining_before_page = usize::try_from(remaining_before_page).unwrap_or(usize::MAX);
        if items.is_empty() && offset < remaining_before_page {
            return Err(HubError::other(format!(
                "message at offset {offset} exceeds the {MAX_MESSAGE_PAGE_BYTES}-byte page budget"
            )));
        }
        let has_more = if let Some(last) = items.last() {
            conn.query_row(
                &format!(
                    "SELECT EXISTS(
                         SELECT 1 FROM messages
                         WHERE conv_id = ?{filter}
                           AND (? IS NULL OR run_id = ?)
                           AND seq > ?
                     )"
                ),
                params![conv_id, run_id, run_id, last.seq],
                |row| row.get::<_, bool>(0),
            )?
        } else {
            false
        };
        let next_cursor = if has_more {
            let last_key = items
                .last()
                .map(|message| message.seq)
                .ok_or_else(|| HubError::other("message cursor did not advance"))?;
            Some(encode_message_cursor(
                &conn,
                &MessageCursorPayload {
                    version: MESSAGE_CURSOR_VERSION,
                    conversation: conv_id.to_string(),
                    generation,
                    last_key,
                    include_audit,
                    run_id: run_id.map(str::to_string),
                    start_after_seq: after_seq,
                    filter: MESSAGE_CURSOR_FILTER.to_string(),
                },
            )?)
        } else {
            None
        };
        let consumed = offset.saturating_add(items.len());
        let next_offset = cursor
            .is_none()
            .then(|| (consumed < total).then_some(consumed))
            .flatten();
        Ok(MessagePage {
            items,
            next_cursor,
            next_offset,
            total,
        })
    }

    pub fn max_message_seq(&self, conv_id: &str) -> Result<i64, HubError> {
        let conn = self.conn.lock();
        conn.query_row(
            "SELECT COALESCE(MAX(seq), 0) FROM messages WHERE conv_id = ?",
            params![conv_id],
            |row| row.get(0),
        )
        .map_err(HubError::from)
    }
}
