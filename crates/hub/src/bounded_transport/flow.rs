use super::*;
use sha2::{Digest, Sha256};

type SharedFlowBudget = Arc<StdMutex<FlowBudget>>;

#[derive(Debug)]
pub(super) struct PhysicalFrame {
    /// Monotonic identity within one physical transport leg.
    pub(super) token: u64,
    /// Canonical semantic identity expected to survive a transparent proxy.
    pub(super) identity: String,
    pub(super) bytes: usize,
}

#[derive(Debug)]
pub(super) struct FlowBudget {
    pub(super) frames: usize,
    pub(super) bytes: usize,
    pub(super) partial_bytes: usize,
    pub(super) requests: HashMap<RequestId, PhysicalFrame>,
    pub(super) notifications: VecDeque<PhysicalFrame>,
    pub(super) responses: VecDeque<PhysicalFrame>,
    next_token: u64,
    pub(super) max_frames: usize,
    pub(super) max_bytes: usize,
}

impl Default for FlowBudget {
    fn default() -> Self {
        Self {
            frames: 0,
            bytes: 0,
            partial_bytes: 0,
            requests: HashMap::new(),
            notifications: VecDeque::new(),
            responses: VecDeque::new(),
            next_token: 1,
            max_frames: MAX_OUTSTANDING_INBOUND_FRAMES,
            max_bytes: MAX_OUTSTANDING_INBOUND_BYTES,
        }
    }
}

impl FlowBudget {
    pub(super) fn track(
        &mut self,
        message: &RawJsonRpcMessage,
        bytes: usize,
    ) -> Result<u64, String> {
        let next_frames = self.frames.saturating_add(1);
        let next_bytes = self.bytes.saturating_add(bytes);
        if next_frames > self.max_frames
            || next_bytes.saturating_add(self.partial_bytes) > self.max_bytes
        {
            return Err(format!(
                "inbound ACP flow exceeds {} outstanding frames or {} outstanding bytes",
                self.max_frames, self.max_bytes
            ));
        }
        let token = self.next_token;
        self.next_token = self
            .next_token
            .checked_add(1)
            .ok_or_else(|| "physical ACP flow reservation token space exhausted".to_string())?;
        let identity = physical_message_identity(message);
        let frame = PhysicalFrame {
            token,
            identity,
            bytes,
        };
        match message {
            RawJsonRpcMessage::Request(request) => {
                if self.requests.len() >= MAX_OUTSTANDING_INBOUND_REQUESTS {
                    return Err(format!(
                        "inbound ACP flow exceeds {MAX_OUTSTANDING_INBOUND_REQUESTS} \
                         outstanding requests"
                    ));
                }
                if self.requests.contains_key(&request.id) {
                    return Err("duplicate outstanding inbound ACP request id".to_string());
                }
                self.requests.insert(request.id.clone(), frame);
            }
            RawJsonRpcMessage::Notification(_) => {
                self.notifications.push_back(frame);
            }
            RawJsonRpcMessage::Response(_) => {
                self.responses.push_back(frame);
            }
        }
        self.frames = next_frames;
        self.bytes = next_bytes;
        Ok(token)
    }

    pub(super) fn acknowledge_notification(
        &mut self,
        method: &str,
        params: &serde_json::Value,
    ) -> Result<PhysicalFrame, String> {
        let identity = notification_identity(method, params);
        // Canonically identical frames can have different wire sizes because
        // of whitespace or object-key order. Releasing the smallest match is
        // conservative: ambiguity may temporarily overcount, never undercount.
        let position = self
            .notifications
            .iter()
            .enumerate()
            .filter(|(_, pending)| pending.identity == identity)
            .min_by_key(|(_, pending)| (pending.bytes, pending.token))
            .map(|(position, _)| position)
            .ok_or_else(|| format!("no physical ACP reservation matches {identity}"))?;
        let frame = self
            .notifications
            .remove(position)
            .expect("matched notification reservation must exist");
        self.release(frame.bytes);
        Ok(frame)
    }

    pub(super) fn acknowledge_response(
        &mut self,
        result: &Result<serde_json::Value, AcpError>,
    ) -> Result<PhysicalFrame, String> {
        let identity = response_identity(result);
        // Apply the same conservative rule as notifications when a proxy
        // reserializes canonically identical response payloads.
        let position = self
            .responses
            .iter()
            .enumerate()
            .filter(|(_, pending)| pending.identity == identity)
            .min_by_key(|(_, pending)| (pending.bytes, pending.token))
            .map(|(position, _)| position)
            .ok_or_else(|| format!("no physical ACP reservation matches {identity}"))?;
        let frame = self
            .responses
            .remove(position)
            .expect("matched response reservation must exist");
        self.release(frame.bytes);
        Ok(frame)
    }

    pub(super) fn complete_request(&mut self, id: &RequestId) -> Result<PhysicalFrame, String> {
        let frame = self.requests.remove(id).ok_or_else(|| {
            format!(
                "no physical ACP reservation matches outbound response {}",
                request_id_text(id)
            )
        })?;
        self.release(frame.bytes);
        Ok(frame)
    }

    pub(super) fn release(&mut self, bytes: usize) {
        self.frames = self.frames.saturating_sub(1);
        self.bytes = self.bytes.saturating_sub(bytes);
    }

    pub(super) fn reserve_partial(&mut self, bytes: usize) -> Result<(), String> {
        let next = self.partial_bytes.saturating_add(bytes);
        if self.bytes.saturating_add(next) > self.max_bytes {
            return Err(format!(
                "inbound ACP partial framing exceeds {} bytes",
                self.max_bytes
            ));
        }
        self.partial_bytes = next;
        Ok(())
    }

    pub(super) fn release_partial(&mut self, bytes: usize) {
        self.partial_bytes = self.partial_bytes.saturating_sub(bytes);
    }
}

fn request_id_text(id: &RequestId) -> String {
    serde_json::to_string(id).unwrap_or_else(|_| "<invalid-id>".to_string())
}

fn request_identity(id: &RequestId, method: &str) -> String {
    format!("request:{}:{method}", request_id_text(id))
}

pub(super) fn notification_identity(method: &str, params: &serde_json::Value) -> String {
    let mut canonical_params = Vec::new();
    write_canonical_json(params, &mut canonical_params);
    let mut hasher = Sha256::new();
    hasher.update(b"notification");
    hasher.update([0]);
    hasher.update(method.as_bytes());
    hasher.update([0]);
    hasher.update(canonical_params);
    let digest = hasher.finalize();
    let digest = digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    format!("notification:{method}:sha256:{digest}")
}

fn write_canonical_json(value: &serde_json::Value, output: &mut Vec<u8>) {
    match value {
        serde_json::Value::Array(items) => {
            output.push(b'[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    output.push(b',');
                }
                write_canonical_json(item, output);
            }
            output.push(b']');
        }
        serde_json::Value::Object(object) => {
            output.push(b'{');
            let mut keys = object.keys().collect::<Vec<_>>();
            keys.sort_unstable();
            for (index, key) in keys.into_iter().enumerate() {
                if index > 0 {
                    output.push(b',');
                }
                serde_json::to_writer(&mut *output, key)
                    .expect("serializing a JSON object key into bytes cannot fail");
                output.push(b':');
                write_canonical_json(&object[key], output);
            }
            output.push(b'}');
        }
        scalar => serde_json::to_writer(output, scalar)
            .expect("serializing a JSON scalar into bytes cannot fail"),
    }
}

fn response_identity(result: &Result<serde_json::Value, AcpError>) -> String {
    // Conductor proxies may remap JSON-RPC ids per physical leg. The response
    // payload is the cross-leg identity; changing it violates the transparent
    // one-to-one contract and therefore produces an explicit ACK mismatch.
    let (kind, value) = match result {
        Ok(value) => ("result", value.clone()),
        Err(error) => (
            "error",
            serde_json::to_value(error)
                .expect("serializing an ACP JSON-RPC error identity cannot fail"),
        ),
    };
    let mut canonical_value = Vec::new();
    write_canonical_json(&value, &mut canonical_value);
    let mut hasher = Sha256::new();
    hasher.update(kind.as_bytes());
    hasher.update([0]);
    hasher.update(canonical_value);
    let digest = hasher.finalize();
    let digest = digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    format!("response:{kind}:sha256:{digest}")
}

fn physical_message_identity(message: &RawJsonRpcMessage) -> String {
    match message {
        RawJsonRpcMessage::Request(request) => {
            request_identity(&request.id, request.method.as_ref())
        }
        RawJsonRpcMessage::Notification(notification) => {
            let params = notification
                .params
                .clone()
                .map_or(serde_json::Value::Null, |params| params.into_value());
            notification_identity(notification.method.as_ref(), &params)
        }
        RawJsonRpcMessage::Response(response) => match response {
            RpcResponse::Result { result, .. } => response_identity(&Ok(result.clone())),
            RpcResponse::Error { error, .. } => response_identity(&Err(error.clone())),
        },
    }
}

#[cfg(any(test, feature = "test-flow-ledger"))]
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TestFlowLedgerEvent {
    pub sequence: u64,
    pub flow_id: u64,
    pub action: String,
    pub identity: Option<String>,
    pub reservation_token: Option<u64>,
    pub frames: usize,
    pub bytes: usize,
    pub partial_bytes: usize,
}

#[cfg(any(test, feature = "test-flow-ledger"))]
#[derive(Debug)]
struct TestFlowLedgerState {
    enabled: bool,
    pause_acknowledgements: bool,
    max_frames: usize,
    max_bytes: usize,
    events: Vec<TestFlowLedgerEvent>,
}

#[cfg(any(test, feature = "test-flow-ledger"))]
static TEST_FLOW_LEDGER: LazyLock<StdMutex<TestFlowLedgerState>> = LazyLock::new(|| {
    StdMutex::new(TestFlowLedgerState {
        enabled: false,
        pause_acknowledgements: false,
        max_frames: MAX_OUTSTANDING_INBOUND_FRAMES,
        max_bytes: MAX_OUTSTANDING_INBOUND_BYTES,
        events: Vec::new(),
    })
});
#[cfg(any(test, feature = "test-flow-ledger"))]
static TEST_FLOW_ID: AtomicU64 = AtomicU64::new(1);
#[cfg(any(test, feature = "test-flow-ledger"))]
static TEST_FLOW_SEQUENCE: AtomicU64 = AtomicU64::new(1);

/// Enable and clear the feature-gated physical-leg flow ledger.
///
/// This is only available to the integration-test build. Production limits
/// remain the fixed constants above.
#[cfg(any(test, feature = "test-flow-ledger"))]
#[cfg_attr(test, allow(dead_code))]
pub fn reset_test_flow_ledger(max_frames: usize, max_bytes: usize) {
    let mut state = TEST_FLOW_LEDGER.lock().expect("test flow ledger poisoned");
    state.enabled = true;
    state.pause_acknowledgements = false;
    state.max_frames = max_frames;
    state.max_bytes = max_bytes;
    state.events.clear();
    TEST_FLOW_ID.store(1, Ordering::SeqCst);
    TEST_FLOW_SEQUENCE.store(1, Ordering::SeqCst);
}

#[cfg(any(test, feature = "test-flow-ledger"))]
#[cfg_attr(test, allow(dead_code))]
pub fn pause_test_flow_acknowledgements(paused: bool) {
    TEST_FLOW_LEDGER
        .lock()
        .expect("test flow ledger poisoned")
        .pause_acknowledgements = paused;
}

#[cfg(any(test, feature = "test-flow-ledger"))]
#[cfg_attr(test, allow(dead_code))]
pub fn test_flow_ledger_snapshot() -> Vec<TestFlowLedgerEvent> {
    TEST_FLOW_LEDGER
        .lock()
        .expect("test flow ledger poisoned")
        .events
        .clone()
}

#[cfg(any(test, feature = "test-flow-ledger"))]
fn test_flow_settings() -> Option<(usize, usize)> {
    let state = TEST_FLOW_LEDGER.lock().ok()?;
    state.enabled.then_some((state.max_frames, state.max_bytes))
}

#[cfg(any(test, feature = "test-flow-ledger"))]
fn test_flow_acknowledgements_paused() -> bool {
    TEST_FLOW_LEDGER
        .lock()
        .is_ok_and(|state| state.enabled && state.pause_acknowledgements)
}

#[cfg(any(test, feature = "test-flow-ledger"))]
fn record_test_flow_event(
    flow_id: u64,
    action: &str,
    identity: Option<String>,
    reservation_token: Option<u64>,
    budget: &FlowBudget,
) {
    if let Ok(mut state) = TEST_FLOW_LEDGER.lock()
        && state.enabled
    {
        state.events.push(TestFlowLedgerEvent {
            sequence: TEST_FLOW_SEQUENCE.fetch_add(1, Ordering::SeqCst),
            flow_id,
            action: action.to_string(),
            identity,
            reservation_token,
            frames: budget.frames,
            bytes: budget.bytes,
            partial_bytes: budget.partial_bytes,
        });
    }
}

#[derive(Clone, Debug)]
pub(crate) struct InboundFlowControl {
    pub(super) inner: SharedFlowBudget,
    #[cfg(any(test, feature = "test-flow-ledger"))]
    test_flow_id: u64,
}

impl InboundFlowControl {
    pub(crate) fn new() -> Self {
        let budget = FlowBudget::default();
        #[cfg(any(test, feature = "test-flow-ledger"))]
        let budget = {
            let mut budget = budget;
            if let Some((max_frames, max_bytes)) = test_flow_settings() {
                budget.max_frames = max_frames;
                budget.max_bytes = max_bytes;
            }
            budget
        };
        Self {
            inner: Arc::new(StdMutex::new(budget)),
            #[cfg(any(test, feature = "test-flow-ledger"))]
            test_flow_id: TEST_FLOW_ID.fetch_add(1, Ordering::SeqCst),
        }
    }

    pub(super) fn track(&self, message: &RawJsonRpcMessage, bytes: usize) -> Result<(), String> {
        #[cfg(any(test, feature = "test-flow-ledger"))]
        let identity = physical_message_identity(message);
        let mut budget = self
            .inner
            .lock()
            .map_err(|_| "ACP flow-budget mutex poisoned".to_string())?;
        let result = budget.track(message, bytes);
        #[cfg(any(test, feature = "test-flow-ledger"))]
        record_test_flow_event(
            self.test_flow_id,
            if result.is_ok() { "reserve" } else { "reject" },
            Some(identity),
            result.as_ref().ok().copied(),
            &budget,
        );
        result.map(|_| ())
    }

    pub(crate) fn acknowledge_notification(
        &self,
        method: &str,
        params: &serde_json::Value,
    ) -> Result<(), AcpError> {
        let mut budget = self
            .inner
            .lock()
            .map_err(|_| AcpError::internal_error().data("ACP flow-budget mutex poisoned"))?;
        #[cfg(any(test, feature = "test-flow-ledger"))]
        if test_flow_acknowledgements_paused() {
            record_test_flow_event(
                self.test_flow_id,
                "ack_deferred",
                Some(notification_identity(method, params)),
                None,
                &budget,
            );
            return Ok(());
        }
        let _frame = match budget.acknowledge_notification(method, params) {
            Ok(frame) => frame,
            Err(error) => {
                #[cfg(any(test, feature = "test-flow-ledger"))]
                record_test_flow_event(
                    self.test_flow_id,
                    "ack_mismatch",
                    Some(notification_identity(method, params)),
                    None,
                    &budget,
                );
                return Err(AcpError::invalid_request().data(error));
            }
        };
        #[cfg(any(test, feature = "test-flow-ledger"))]
        record_test_flow_event(
            self.test_flow_id,
            "ack",
            Some(_frame.identity),
            Some(_frame.token),
            &budget,
        );
        Ok(())
    }

    pub(crate) fn acknowledge_response(
        &self,
        result: &Result<serde_json::Value, AcpError>,
    ) -> Result<(), AcpError> {
        let mut budget = self
            .inner
            .lock()
            .map_err(|_| AcpError::internal_error().data("ACP flow-budget mutex poisoned"))?;
        #[cfg(any(test, feature = "test-flow-ledger"))]
        if test_flow_acknowledgements_paused() {
            record_test_flow_event(
                self.test_flow_id,
                "ack_deferred",
                Some(response_identity(result)),
                None,
                &budget,
            );
            return Ok(());
        }
        let _frame = match budget.acknowledge_response(result) {
            Ok(frame) => frame,
            Err(error) => {
                #[cfg(any(test, feature = "test-flow-ledger"))]
                record_test_flow_event(
                    self.test_flow_id,
                    "ack_mismatch",
                    Some(response_identity(result)),
                    None,
                    &budget,
                );
                return Err(AcpError::invalid_request().data(error));
            }
        };
        #[cfg(any(test, feature = "test-flow-ledger"))]
        record_test_flow_event(
            self.test_flow_id,
            "ack",
            Some(_frame.identity),
            Some(_frame.token),
            &budget,
        );
        Ok(())
    }

    pub(super) fn complete_outbound_response(
        &self,
        message: &RawJsonRpcMessage,
    ) -> Result<(), String> {
        let RawJsonRpcMessage::Response(response) = message else {
            return Ok(());
        };
        let id = match response {
            RpcResponse::Result { id, .. } | RpcResponse::Error { id, .. } => id,
        };
        let mut budget = self
            .inner
            .lock()
            .map_err(|_| "ACP flow-budget mutex poisoned".to_string())?;
        // The SDK represents an out-of-band notification-handler error as an
        // error response with id null. It is not a reply and therefore has no
        // inbound request reservation to release. A real request whose id is
        // null still takes the normal exact-match path below.
        if matches!(
            response,
            RpcResponse::Error {
                id: RequestId::Null,
                ..
            }
        ) && !budget.requests.contains_key(id)
        {
            return Ok(());
        }
        let _frame = budget.complete_request(id)?;
        #[cfg(any(test, feature = "test-flow-ledger"))]
        record_test_flow_event(
            self.test_flow_id,
            "ack",
            Some(_frame.identity),
            Some(_frame.token),
            &budget,
        );
        Ok(())
    }

    pub(super) fn complete_request_id(&self, id: &RequestId) -> Result<(), String> {
        let mut budget = self
            .inner
            .lock()
            .map_err(|_| "ACP flow-budget mutex poisoned".to_string())?;
        let _frame = budget.complete_request(id)?;
        #[cfg(any(test, feature = "test-flow-ledger"))]
        record_test_flow_event(
            self.test_flow_id,
            "ack",
            Some(_frame.identity),
            Some(_frame.token),
            &budget,
        );
        Ok(())
    }

    pub(super) fn reserve_partial(&self, bytes: usize) -> Result<(), String> {
        self.inner
            .lock()
            .map_err(|_| "ACP flow-budget mutex poisoned".to_string())?
            .reserve_partial(bytes)
    }

    pub(super) fn release_partial(&self, bytes: usize) {
        if let Ok(mut flow) = self.inner.lock() {
            flow.release_partial(bytes);
        }
    }

    pub(super) fn track_from_partial(
        &self,
        message: &RawJsonRpcMessage,
        bytes: usize,
    ) -> Result<(), String> {
        let mut flow = self
            .inner
            .lock()
            .map_err(|_| "ACP flow-budget mutex poisoned".to_string())?;
        flow.release_partial(bytes);
        flow.track(message, bytes).map(|_| ())
    }
}

pub(super) fn charge_flow(
    flow: &InboundFlowControl,
    message: &RawJsonRpcMessage,
    bytes: usize,
) -> Result<(), String> {
    flow.track(message, bytes)
}

pub(super) fn complete_outbound_response(
    flow: &InboundFlowControl,
    message: &RawJsonRpcMessage,
) -> Result<(), String> {
    flow.complete_outbound_response(message)
}

#[derive(Debug)]
pub(crate) struct BoundedStdioAgent {
    inner: AcpAgent,
    flow: InboundFlowControl,
}

impl BoundedStdioAgent {
    pub(crate) fn with_flow(inner: AcpAgent, flow: InboundFlowControl) -> Self {
        Self { inner, flow }
    }
}

struct ChildGuard(Child);

impl ChildGuard {
    async fn wait(&mut self) -> io::Result<std::process::ExitStatus> {
        self.0.status().await
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        drop(self.0.kill());
    }
}

impl<Counterpart: Role> ConnectTo<Counterpart> for BoundedStdioAgent {
    async fn connect_to(
        self,
        client: impl ConnectTo<Counterpart::Counterpart>,
    ) -> Result<(), AcpError> {
        let (child_stdin, child_stdout, child_stderr, child) = self.inner.spawn_process()?;

        let flow = self.flow;
        let incoming = bounded_line_stream(child_stdout, flow.clone());
        let outgoing = futures::sink::unfold(
            (child_stdin, flow),
            async move |(mut writer, flow), line: String| {
                if line.len().saturating_add(1) > MAX_ACP_FRAME_BYTES {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("outbound ACP frame exceeds {MAX_ACP_FRAME_BYTES} bytes"),
                    ));
                }
                let message: RawJsonRpcMessage = serde_json::from_str(&line).map_err(|error| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("malformed outbound ACP JSON-RPC frame: {error}"),
                    )
                })?;
                writer.write_all(line.as_bytes()).await?;
                writer.write_all(b"\n").await?;
                complete_outbound_response(&flow, &message).map_err(io::Error::other)?;
                Ok::<_, io::Error>((writer, flow))
            },
        );

        let protocol = ConnectTo::<Counterpart>::connect_to(Lines::new(outgoing, incoming), client);
        let monitor = async move {
            let mut guard = ChildGuard(child);
            let status = guard.wait().await.map_err(|error| {
                AcpError::internal_error().data(format!("failed to wait for ACP process: {error}"))
            })?;
            if status.success() {
                Ok(())
            } else {
                Err(AcpError::internal_error().data(format!("ACP process exited with {status}")))
            }
        };
        let stderr = drain_stderr(child_stderr);

        let protocol = pin!(protocol);
        let monitor = pin!(monitor);
        let main_race = async {
            match futures::future::select(protocol, monitor).await {
                futures::future::Either::Left((result, _))
                | futures::future::Either::Right((result, _)) => result,
            }
        };
        let main_race = pin!(main_race);
        let stderr = pin!(stderr);
        match futures::future::select(main_race, stderr).await {
            futures::future::Either::Left((result, _)) => result,
            futures::future::Either::Right((result, main)) => {
                result.map_err(|error| {
                    AcpError::internal_error().data(format!("failed to drain ACP stderr: {error}"))
                })?;
                main.await
            }
        }
    }
}

pub(super) fn bounded_line_stream<R>(
    reader: R,
    flow: InboundFlowControl,
) -> Pin<Box<dyn Stream<Item = io::Result<String>> + Send + 'static>>
where
    R: AsyncRead + Send + Unpin + 'static,
{
    let reader = futures::io::BufReader::new(reader);
    Box::pin(futures::stream::try_unfold(
        (reader, flow),
        |(mut reader, flow)| async move {
            match read_bounded_line(&mut reader).await? {
                Some(line) => {
                    let message: RawJsonRpcMessage =
                        serde_json::from_str(&line).map_err(|error| {
                            io::Error::new(
                                io::ErrorKind::InvalidData,
                                format!("malformed inbound ACP JSON-RPC frame: {error}"),
                            )
                        })?;
                    charge_flow(&flow, &message, line.len().saturating_add(1))
                        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
                    Ok(Some((line, (reader, flow))))
                }
                None => Ok(None),
            }
        },
    ))
}

pub(super) async fn read_bounded_line<R>(reader: &mut R) -> io::Result<Option<String>>
where
    R: AsyncBufRead + Unpin,
{
    let mut bytes = Vec::new();
    loop {
        let buffer = reader.fill_buf().await?;
        if buffer.is_empty() {
            if bytes.is_empty() {
                return Ok(None);
            }
            break;
        }
        if let Some(newline) = buffer.iter().position(|byte| *byte == b'\n') {
            if bytes.len().saturating_add(newline) > MAX_ACP_FRAME_BYTES {
                return Err(frame_too_large());
            }
            bytes.extend_from_slice(&buffer[..newline]);
            reader.consume_unpin(newline + 1);
            break;
        }
        if bytes.len().saturating_add(buffer.len()) > MAX_ACP_FRAME_BYTES {
            return Err(frame_too_large());
        }
        let consumed = buffer.len();
        bytes.extend_from_slice(buffer);
        reader.consume_unpin(consumed);
    }
    if bytes.last() == Some(&b'\r') {
        bytes.pop();
    }
    String::from_utf8(bytes)
        .map(Some)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

pub(super) fn frame_too_large() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("inbound ACP frame exceeds {MAX_ACP_FRAME_BYTES} bytes"),
    )
}

async fn drain_stderr<R>(mut reader: R) -> io::Result<()>
where
    R: AsyncRead + Unpin,
{
    let mut buffer = [0_u8; 8192];
    loop {
        let read = reader.read(&mut buffer).await?;
        if read == 0 {
            return Ok(());
        }
    }
}
