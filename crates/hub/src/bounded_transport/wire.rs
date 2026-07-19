use super::*;

pub(super) fn sanitized_reqwest_error(error: reqwest::Error) -> String {
    error.without_url().to_string()
}

pub(super) async fn bounded_response_bytes(response: reqwest::Response) -> Result<Vec<u8>, String> {
    if response
        .content_length()
        .is_some_and(|length| length > MAX_ACP_FRAME_BYTES as u64)
    {
        return Err(format!("HTTP ACP body exceeds {MAX_ACP_FRAME_BYTES} bytes"));
    }
    let mut stream = response.bytes_stream();
    let mut body = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(sanitized_reqwest_error)?;
        if body.len().saturating_add(chunk.len()) > MAX_ACP_FRAME_BYTES {
            return Err(format!("HTTP ACP body exceeds {MAX_ACP_FRAME_BYTES} bytes"));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

pub(super) fn serialize_bounded(message: &RawJsonRpcMessage) -> Result<Vec<u8>, String> {
    let bytes = serde_json::to_vec(message).map_err(|error| error.to_string())?;
    if bytes.len() > MAX_ACP_FRAME_BYTES {
        return Err(format!(
            "outbound ACP frame exceeds {MAX_ACP_FRAME_BYTES} bytes"
        ));
    }
    Ok(bytes)
}

#[derive(Default)]
pub(super) struct SseDecoder {
    line: Vec<u8>,
    data: Vec<u8>,
}

impl SseDecoder {
    fn buffered_len(&self) -> usize {
        self.line.len().saturating_add(self.data.len())
    }

    pub(super) fn push(&mut self, chunk: &[u8]) -> Result<Vec<Vec<u8>>, String> {
        let mut events = Vec::new();
        for byte in chunk {
            if *byte == b'\n' {
                let mut line = std::mem::take(&mut self.line);
                if line.last() == Some(&b'\r') {
                    line.pop();
                }
                self.consume_line(&line, &mut events)?;
            } else {
                if self.line.len() >= MAX_ACP_FRAME_BYTES {
                    return Err(format!("SSE line exceeds {MAX_ACP_FRAME_BYTES} bytes"));
                }
                self.line.push(*byte);
            }
        }
        Ok(events)
    }

    fn consume_line(&mut self, line: &[u8], events: &mut Vec<Vec<u8>>) -> Result<(), String> {
        if line.is_empty() {
            if !self.data.is_empty() {
                if self.data.last() == Some(&b'\n') {
                    self.data.pop();
                }
                events.push(std::mem::take(&mut self.data));
            }
            return Ok(());
        }
        let Some(value) = line.strip_prefix(b"data:") else {
            return Ok(());
        };
        let value = value.strip_prefix(b" ").unwrap_or(value);
        if self
            .data
            .len()
            .saturating_add(value.len())
            .saturating_add(1)
            > MAX_ACP_FRAME_BYTES
        {
            return Err(format!("SSE event exceeds {MAX_ACP_FRAME_BYTES} bytes"));
        }
        self.data.extend_from_slice(value);
        self.data.push(b'\n');
        Ok(())
    }
}

struct PartialReservation {
    flow: InboundFlowControl,
    bytes: usize,
}

impl PartialReservation {
    fn new(flow: InboundFlowControl) -> Self {
        Self { flow, bytes: 0 }
    }

    fn add(&mut self, bytes: usize) -> Result<(), String> {
        self.flow.reserve_partial(bytes)?;
        self.bytes = self.bytes.saturating_add(bytes);
        Ok(())
    }

    fn retain(&mut self, bytes: usize) {
        if bytes < self.bytes {
            self.flow.release_partial(self.bytes - bytes);
        }
        self.bytes = bytes;
    }

    fn transfer_message(
        &mut self,
        message: &RawJsonRpcMessage,
        bytes: usize,
    ) -> Result<(), String> {
        self.bytes = self.bytes.saturating_sub(bytes);
        self.flow.track_from_partial(message, bytes)
    }
}

impl Drop for PartialReservation {
    fn drop(&mut self) {
        self.flow.release_partial(self.bytes);
    }
}

pub(super) async fn read_sse(
    connection: HttpConnection,
    session_id: Option<String>,
    mut sender: Sender<SseMessage>,
) -> Result<(), String> {
    let connection_id = connection
        .connection_id()
        .ok_or_else(|| "SSE attempted before initialize".to_string())?;
    let mut request = connection
        .http
        .get(connection.endpoint.clone())
        .header("Accept", "text/event-stream")
        .header(HEADER_CONNECTION_ID, connection_id);
    if let Some(session_id) = &session_id {
        request = request.header(HEADER_SESSION_ID, session_id);
    }
    let response = request.send().await.map_err(sanitized_reqwest_error)?;
    if !response.status().is_success() {
        return Err(format!("HTTP {}", response.status()));
    }
    let mut stream = response.bytes_stream();
    let mut decoder = SseDecoder::default();
    let mut partial = PartialReservation::new(connection.flow.clone());
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(sanitized_reqwest_error)?;
        partial.add(chunk.len())?;
        let payloads = decoder.push(&chunk)?;
        let retained = decoder.buffered_len().saturating_add(
            payloads
                .iter()
                .map(Vec::len)
                .fold(0_usize, usize::saturating_add),
        );
        partial.retain(retained);
        for payload in payloads {
            if payload.is_empty() {
                continue;
            }
            let message = serde_json::from_slice(&payload)
                .map_err(|error| format!("malformed SSE JSON-RPC payload: {error}"))?;
            partial.transfer_message(&message, payload.len())?;
            sender
                .send(SseMessage { message })
                .await
                .map_err(|_| "upstream channel closed".to_string())?;
        }
    }
    Ok(())
}

pub(super) fn sanitized_websocket_error(context: &'static str, error: &WsError) -> AcpError {
    let cause = match error {
        WsError::ConnectionClosed => "connection closed".to_string(),
        WsError::AlreadyClosed => "connection already closed".to_string(),
        WsError::Io(error) => format!("I/O failure ({:?})", error.kind()),
        WsError::Tls(_) => "TLS failure".to_string(),
        WsError::Capacity(_) => "capacity limit exceeded".to_string(),
        WsError::Protocol(_) => "protocol violation".to_string(),
        WsError::WriteBufferFull(_) => "write buffer full".to_string(),
        WsError::Utf8(_) => "UTF-8 failure".to_string(),
        WsError::AttackAttempt => "attack attempt detected".to_string(),
        WsError::Url(_) => "invalid URL".to_string(),
        WsError::Http(response) => {
            format!("HTTP handshake rejected ({})", response.status())
        }
        WsError::HttpFormat(_) => "invalid HTTP handshake".to_string(),
    };
    AcpError::internal_error().data(format!("WebSocket {context} failed: {cause}"))
}
