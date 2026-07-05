//! JSON endpoint registry (Spec 1, design 1).
//!
//! `agents.json` is the single source of truth for registered ACP Agent
//! Endpoints and ACP Proxies. It mirrors the MCP `mcpServers` object-map
//! convention with `acpAgents` / `acpProxies`. SQLite `agent_cache` only ever
//! holds *negotiated* capabilities; on any disagreement the JSON wins.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use uuid::Uuid;

use crate::error::HubError;

/// Canonical home directory for Hub state (`$ACP_HUB_HOME`, else `$HOME/.acp-hub`,
/// else `$USERPROFILE/.acp-hub`).
pub fn home_dir() -> Result<PathBuf, HubError> {
    if let Some(dir) = std::env::var_os("ACP_HUB_HOME") {
        return Ok(PathBuf::from(dir));
    }
    let base = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .ok_or_else(|| HubError::other("cannot determine home directory; set ACP_HUB_HOME"))?;
    Ok(PathBuf::from(base).join(".acp-hub"))
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "lowercase", deny_unknown_fields)]
pub enum AgentTransport {
    /// Spawn a local process speaking ACP over newline-delimited JSON-RPC on stdio.
    Stdio {
        command: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        env: BTreeMap<String, String>,
    },
    /// Connect to a remote agent over HTTP (`POST /acp`, SSE response).
    Http {
        url: String,
        #[serde(default)]
        headers: BTreeMap<String, String>,
    },
    /// Connect to a remote agent over WebSocket (`ws://` / `wss://`).
    WebSocket {
        url: String,
        #[serde(default)]
        headers: BTreeMap<String, String>,
    },
}

/// Transport for an ACP Proxy component.
///
/// Stdio-only for this SDK revision: `AcpAgent` implements
/// `ConnectTo<Conductor>` for stdio processes, and no HTTP/WS proxy component
/// ships with this SDK rev. The registry enum intentionally models only
/// supported transports; serde rejects any other proxy transport tag while
/// parsing `agents.json`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "lowercase", deny_unknown_fields)]
pub enum ProxyTransport {
    Stdio {
        command: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        env: BTreeMap<String, String>,
    },
}

/// How the Hub answers `session/request_permission` callbacks.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PermissionPolicy {
    /// Reject every permission request (first reject option, else `Cancelled`).
    #[default]
    Reject,
    /// Auto-cancel any permission request.
    AutoCancel,
    /// Auto-approve any permission request (first allow option, else `Cancelled`).
    AutoAllow,
}

/// Client filesystem capability configuration advertised to the agent.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct FsConfig {
    /// Advertise `fs/read_text_file`.
    pub read_text_file: bool,
    /// Advertise `fs/write_text_file`.
    pub write_text_file: bool,
    /// Roots the Hub may read/write within (empty ⇒ session cwd).
    pub allowed_roots: Vec<PathBuf>,
}

/// Client capabilities advertised to the agent.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ClientCapabilityConfig {
    pub fs: FsConfig,
    pub terminal: bool,
}

/// A registered ACP Agent Endpoint.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentEndpointConfig {
    pub transport: AgentTransport,
    #[serde(default)]
    pub proxy_chain: Vec<String>,
    #[serde(default)]
    pub permission_policy: PermissionPolicy,
    #[serde(default)]
    pub client_capabilities: ClientCapabilityConfig,
}

/// A registered ACP Proxy Endpoint (stdio component).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProxyEndpointConfig {
    pub transport: ProxyTransport,
}

/// The full registry, mirroring MCP's `mcpServers` object-map convention.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Registry {
    #[serde(default, rename = "acpAgents")]
    pub agents: BTreeMap<String, AgentEndpointConfig>,
    #[serde(default, rename = "acpProxies")]
    pub proxies: BTreeMap<String, ProxyEndpointConfig>,
}

/// Observed filesystem state of `agents.json`, used to detect external edits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileFingerprint {
    mtime: SystemTime,
    len: u64,
}

const ID_PATTERN: &str = r"A-Za-z0-9_.-";

impl Registry {
    /// Path to the registry file inside `home`.
    pub fn path(home: &Path) -> PathBuf {
        home.join("agents.json")
    }

    /// Load and validate the registry from disk. Missing file ⇒ empty registry.
    pub fn load(home: &Path) -> Result<Self, HubError> {
        let path = Self::path(home);
        match std::fs::read_to_string(&path) {
            Ok(text) => Self::parse(&text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(HubError::Io(e)),
        }
    }

    /// Parse and validate registry JSON.
    pub fn parse(text: &str) -> Result<Self, HubError> {
        let reg: Registry = serde_json::from_str(text)
            .map_err(|e| HubError::InvalidRegistry(format!("agents.json is not valid: {e}")))?;
        reg.validate()?;
        Ok(reg)
    }

    /// Validate ids and references.
    pub fn validate(&self) -> Result<(), HubError> {
        for id in self.agents.keys() {
            if !is_valid_id(id) {
                return Err(HubError::InvalidRegistry(format!(
                    "invalid agent id {id:?} (must match [{}]+)",
                    ID_PATTERN
                )));
            }
        }
        for id in self.proxies.keys() {
            if !is_valid_id(id) {
                return Err(HubError::InvalidRegistry(format!(
                    "invalid proxy id {id:?} (must match [{}]+)",
                    ID_PATTERN
                )));
            }
        }
        // proxyChain references are validated eagerly: proxies must be
        // registered before agents that reference them.
        for (id, agent) in &self.agents {
            for p in &agent.proxy_chain {
                if !self.proxies.contains_key(p) {
                    return Err(HubError::InvalidRegistry(format!(
                        "agent {id:?} references unknown proxy {p:?} in proxyChain"
                    )));
                }
            }
        }
        Ok(())
    }

    /// Atomically write the registry to disk.
    ///
    /// Uses a unique temp file name (UUID-suffixed) rather than a fixed
    /// `.json.tmp`, so two concurrent saves never collide on the same temp
    /// path. The temp file is fully written and `sync_all`ed before the atomic
    /// rename. After the rename, the parent directory is best-effort synced
    /// where the standard library can open it as a file handle.
    pub fn save(&self, home: &Path) -> Result<(), HubError> {
        std::fs::create_dir_all(home)?;
        let path = Self::path(home);
        let text = serde_json::to_string_pretty(self)?;
        let tmp = path.with_extension(format!("json.{}.tmp", Uuid::new_v4()));
        {
            let mut file = File::create(&tmp)?;
            file.write_all(text.as_bytes())?;
            file.sync_all()?;
        }
        replace_registry_file(&tmp, &path)?;
        sync_parent_directory(&path);
        Ok(())
    }

    /// Current fingerprint of the registry file (mtime + length), or `None`
    /// if the file does not exist.
    pub fn fingerprint(home: &Path) -> Result<Option<FileFingerprint>, HubError> {
        match std::fs::metadata(Self::path(home)) {
            Ok(meta) => Ok(Some(FileFingerprint {
                mtime: meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                len: meta.len(),
            })),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(HubError::Io(e)),
        }
    }

    /// Resolve the proxy components for an agent's chain (ordered), validating
    /// every reference exists. Called at send time.
    pub fn proxy_chain(&self, agent_id: &str) -> Result<Vec<&ProxyEndpointConfig>, HubError> {
        let agent = self
            .agents
            .get(agent_id)
            .ok_or_else(|| HubError::not_found("agent", agent_id))?;
        let mut out = Vec::with_capacity(agent.proxy_chain.len());
        for p in &agent.proxy_chain {
            out.push(
                self.proxies
                    .get(p)
                    .ok_or_else(|| HubError::not_found("proxy", p))?,
            );
        }
        Ok(out)
    }

    // --- mutators used by hub/agent/* and hub/proxy/* RPCs -----------------

    /// Insert or replace an agent. Id is validated. Caller persists via
    /// [`Registry::save`].
    pub fn register_agent(
        &mut self,
        id: String,
        config: AgentEndpointConfig,
    ) -> Result<(), HubError> {
        if !is_valid_id(&id) {
            return Err(HubError::InvalidRegistry(format!(
                "invalid agent id {id:?}"
            )));
        }
        let previous = self.agents.insert(id.clone(), config);
        if let Err(err) = self.validate() {
            match previous {
                Some(config) => {
                    self.agents.insert(id, config);
                }
                None => {
                    self.agents.remove(&id);
                }
            }
            return Err(err);
        }
        Ok(())
    }

    pub fn remove_agent(&mut self, id: &str) -> Result<(), HubError> {
        self.agents
            .remove(id)
            .ok_or_else(|| HubError::not_found("agent", id))?;
        Ok(())
    }

    pub fn register_proxy(
        &mut self,
        id: String,
        config: ProxyEndpointConfig,
    ) -> Result<(), HubError> {
        if !is_valid_id(&id) {
            return Err(HubError::InvalidRegistry(format!(
                "invalid proxy id {id:?}"
            )));
        }
        self.proxies.insert(id, config);
        self.validate()
    }

    pub fn remove_proxy(&mut self, id: &str) -> Result<(), HubError> {
        // Refuse to remove a proxy still referenced by an agent's
        // proxy_chain; otherwise agents would point at a missing proxy.
        let dependents: Vec<&String> = self
            .agents
            .iter()
            .filter(|(_, cfg)| cfg.proxy_chain.iter().any(|p| p == id))
            .map(|(agent_id, _)| agent_id)
            .collect();
        if !dependents.is_empty() {
            let list = dependents
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            return Err(HubError::other(format!(
                "proxy '{id}' is referenced by agent(s): {list}"
            )));
        }
        self.proxies
            .remove(id)
            .ok_or_else(|| HubError::not_found("proxy", id))?;
        Ok(())
    }
}

fn replace_registry_file(tmp: &Path, path: &Path) -> Result<(), HubError> {
    match std::fs::rename(tmp, path) {
        Ok(()) => Ok(()),
        Err(err) => replace_registry_file_after_rename_error(tmp, path, err),
    }
}

#[cfg(windows)]
fn replace_registry_file_after_rename_error(
    tmp: &Path,
    path: &Path,
    original_error: std::io::Error,
) -> Result<(), HubError> {
    if !path.exists() {
        return Err(HubError::Io(original_error));
    }
    {
        let mut source = File::open(tmp)?;
        let mut destination = File::create(path)?;
        std::io::copy(&mut source, &mut destination)?;
        destination.sync_all()?;
    }
    let _ = std::fs::remove_file(tmp);
    Ok(())
}

#[cfg(not(windows))]
fn replace_registry_file_after_rename_error(
    _tmp: &Path,
    _path: &Path,
    original_error: std::io::Error,
) -> Result<(), HubError> {
    Err(HubError::Io(original_error))
}

/// Validate an agent/proxy id: non-empty, only `[A-Za-z0-9_.-]`.
fn is_valid_id(s: &str) -> bool {
    !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'.' || b == b'-')
}

fn sync_parent_directory(path: &Path) {
    if let Some(parent) = path.parent() {
        // Directory handles cannot be opened on every platform/filesystem via
        // safe std APIs (notably some Windows configurations). The registry is
        // still more durable because the temp file itself is fsynced before
        // rename; directory sync is an opportunistic extra when available.
        if let Ok(dir) = File::open(parent) {
            // Best-effort only; the temp file fsync above is the hard durability
            // guarantee this path can enforce portably.
            let _sync_result = dir.sync_all();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stdio_agent(proxy_chain: Vec<&str>) -> AgentEndpointConfig {
        AgentEndpointConfig {
            transport: AgentTransport::Stdio {
                command: "agent".into(),
                args: Vec::new(),
                env: BTreeMap::new(),
            },
            proxy_chain: proxy_chain.into_iter().map(String::from).collect(),
            permission_policy: PermissionPolicy::default(),
            client_capabilities: ClientCapabilityConfig::default(),
        }
    }

    fn stdio_proxy() -> ProxyEndpointConfig {
        ProxyEndpointConfig {
            transport: ProxyTransport::Stdio {
                command: "proxy".into(),
                args: Vec::new(),
                env: BTreeMap::new(),
            },
        }
    }

    #[test]
    fn register_agent_rejects_unknown_proxy_chain_without_mutating_registry() {
        let mut registry = Registry::default();

        let err = registry
            .register_agent("agent".into(), stdio_agent(vec!["missing-proxy"]))
            .unwrap_err();

        assert!(
            err.to_string()
                .contains("references unknown proxy \"missing-proxy\""),
            "unexpected error: {err}"
        );
        assert!(!registry.agents.contains_key("agent"));
    }

    #[test]
    fn parse_rejects_non_stdio_proxy_transport_tags() {
        let err = Registry::parse(
            r#"{
                "acpProxies": {
                    "proxy": {
                        "transport": {
                            "type": "http",
                            "url": "https://example.com/acp"
                        }
                    }
                }
            }"#,
        )
        .unwrap_err();

        assert!(
            err.to_string().contains("agents.json is not valid"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn save_writes_parseable_registry_without_temp_file_leftover() {
        let home = std::env::temp_dir().join(format!("acp-hub-registry-{}", Uuid::new_v4()));
        let result = (|| -> Result<(), HubError> {
            let mut registry = Registry::default();
            registry.register_proxy("proxy".into(), stdio_proxy())?;
            registry.register_agent("agent".into(), stdio_agent(vec!["proxy"]))?;

            registry.save(&home)?;

            assert_eq!(Registry::load(&home)?, registry);
            let leftover_tmp_files = std::fs::read_dir(&home)?
                .filter_map(Result::ok)
                .filter(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"))
                .count();
            assert_eq!(leftover_tmp_files, 0);
            Ok(())
        })();
        let _ = std::fs::remove_dir_all(&home);

        result.unwrap();
    }

    #[test]
    fn save_replaces_existing_registry_file() {
        let home = std::env::temp_dir().join(format!("acp-hub-registry-{}", Uuid::new_v4()));
        let result = (|| -> Result<(), HubError> {
            let mut first = Registry::default();
            first.register_agent("first".into(), stdio_agent(Vec::new()))?;
            first.save(&home)?;

            let mut second = Registry::default();
            second.register_agent("second".into(), stdio_agent(Vec::new()))?;
            second.save(&home)?;

            assert_eq!(Registry::load(&home)?, second);
            assert!(!Registry::load(&home)?.agents.contains_key("first"));
            Ok(())
        })();
        let _ = std::fs::remove_dir_all(&home);

        result.unwrap();
    }
}
