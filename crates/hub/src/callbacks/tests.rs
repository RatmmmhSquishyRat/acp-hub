use super::*;
use crate::endpoint::{AgentTransport, ClientCapabilityConfig};
use crate::store::NewConversation;
use agent_client_protocol::schema::v1::{ContentBlock, ContentChunk, TextContent};
use std::collections::BTreeMap;

fn context() -> (Arc<HubCtx>, PathBuf) {
    let home = std::env::temp_dir().join(format!("acp-hub-callbacks-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&home).expect("create test home");
    let store = Store::open(&home).expect("open test store");
    (HubCtx::new(store), home)
}

fn binding(agent_id: &str, conv_id: &str, cwd: &Path) -> SessionBinding {
    SessionBinding {
        conv_id: conv_id.into(),
        agent_id: agent_id.into(),
        permission_policy: PermissionPolicy::Reject,
        fs: FsConfig {
            read_text_file: true,
            write_text_file: true,
            allowed_roots: vec![cwd.into()],
        },
        cwd: cwd.into(),
    }
}

fn config(read: bool, terminal: bool) -> AgentEndpointConfig {
    AgentEndpointConfig {
        transport: AgentTransport::Stdio {
            command: "unused".into(),
            args: Vec::new(),
            env: BTreeMap::new(),
        },
        proxy_chain: Vec::new(),
        permission_policy: PermissionPolicy::Reject,
        client_capabilities: ClientCapabilityConfig {
            fs: FsConfig {
                read_text_file: read,
                write_text_file: read,
                allowed_roots: Vec::new(),
            },
            terminal,
        },
    }
}

include!("tests/permission_filesystem.rs");
include!("tests/terminal.rs");
include!("tests/capture.rs");
