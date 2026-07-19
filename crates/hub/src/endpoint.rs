//! JSON endpoint registry (Spec 1, design 1).
//!
//! `agents.json` is the single source of truth for registered ACP Agent
//! Endpoints and ACP Proxies. It mirrors the MCP `mcpServers` object-map
//! convention with `acpAgents` / `acpProxies`. SQLite `agent_cache` only ever
//! holds *negotiated* capabilities; on any disagreement the JSON wins.

use std::collections::BTreeMap;
use std::hash::{DefaultHasher, Hasher};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

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
/// ships with this SDK rev. The seam stays transport-tagged so future
/// `ConnectTo<Conductor>` adapters can be added without touching the conductor
/// surface; other transports are rejected with `UnsupportedProxyTransport`.
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

/// Secret-safe endpoint configuration returned by ordinary public read APIs.
///
/// Transport credentials are deliberately replaced before this DTO reaches
/// daemon serialization. Stdio argument count and environment/header names
/// remain available for operational inspection, but their values do not.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PublicEndpointConfig {
    pub transport: PublicTransport,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proxy_chain: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permission_policy: Option<PermissionPolicy>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_capabilities: Option<ClientCapabilityConfig>,
}

/// Secret-safe transport projection used by [`PublicEndpointConfig`].
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum PublicTransport {
    Stdio {
        command: String,
        args: Vec<String>,
        env: BTreeMap<String, String>,
    },
    Http {
        url: String,
        headers: BTreeMap<String, String>,
    },
    WebSocket {
        url: String,
        headers: BTreeMap<String, String>,
    },
}

/// Raw endpoint input accepted by [`public_endpoint_config`].
#[derive(Debug, Clone, Copy)]
pub enum EndpointConfigRef<'a> {
    Agent(&'a AgentEndpointConfig),
    Proxy(&'a ProxyEndpointConfig),
}

/// Project a raw registry endpoint into the only DTO used by ordinary reads.
pub fn public_endpoint_config(config: EndpointConfigRef<'_>) -> PublicEndpointConfig {
    match config {
        EndpointConfigRef::Agent(config) => PublicEndpointConfig {
            transport: public_agent_transport(&config.transport),
            proxy_chain: Some(config.proxy_chain.clone()),
            permission_policy: Some(config.permission_policy),
            client_capabilities: Some(config.client_capabilities.clone()),
        },
        EndpointConfigRef::Proxy(config) => PublicEndpointConfig {
            transport: match &config.transport {
                ProxyTransport::Stdio { command, args, env } => {
                    public_stdio_transport(command, args.len(), env)
                }
            },
            proxy_chain: None,
            permission_policy: None,
            client_capabilities: None,
        },
    }
}

fn public_agent_transport(transport: &AgentTransport) -> PublicTransport {
    match transport {
        AgentTransport::Stdio { command, args, env } => {
            public_stdio_transport(command, args.len(), env)
        }
        AgentTransport::Http { url, headers } => PublicTransport::Http {
            url: public_url(url),
            headers: redacted_values(headers),
        },
        AgentTransport::WebSocket { url, headers } => PublicTransport::WebSocket {
            url: public_url(url),
            headers: redacted_values(headers),
        },
    }
}

fn public_stdio_transport(
    _command: &str,
    argument_count: usize,
    env: &BTreeMap<String, String>,
) -> PublicTransport {
    PublicTransport::Stdio {
        command: "<redacted-command>".to_string(),
        args: vec!["<redacted>".to_string(); argument_count],
        env: redacted_values(env),
    }
}

fn redacted_values(values: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    values
        .keys()
        .map(|key| (key.clone(), "<redacted>".to_string()))
        .collect()
}

fn public_url(raw: &str) -> String {
    let Some((scheme, authority_and_rest)) = raw.split_once("://") else {
        return "<redacted-url>".to_string();
    };
    let scheme = scheme.to_ascii_lowercase();
    if !matches!(scheme.as_str(), "http" | "https" | "ws" | "wss") {
        return "<redacted-url>".to_string();
    }
    let raw_authority = authority_and_rest
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default();
    if raw_authority.is_empty()
        || raw_authority.contains('\\')
        || raw_authority
            .chars()
            .any(|ch| ch.is_ascii_control() || ch.is_ascii_whitespace())
    {
        return "<redacted-url>".to_string();
    }

    let Ok(url) = reqwest::Url::parse(raw) else {
        return "<redacted-url>".to_string();
    };
    if url.scheme() != scheme.as_str() {
        return "<redacted-url>".to_string();
    }
    let Some(host) = url.host() else {
        return "<redacted-url>".to_string();
    };
    let port = url
        .port()
        .map(|port| format!(":{port}"))
        .unwrap_or_default();
    format!("{}://{host}{port}/<redacted>", url.scheme())
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
    content_hash: u64,
}

const ID_PATTERN: &str = r"A-Za-z0-9_.-";

/// Maximum number of proxies in an agent's intentionally linear chain.
///
/// Sixteen keeps practical composition available while bounding conductor
/// component construction from registry-controlled input.
pub const MAX_PROXY_CHAIN_LEN: usize = 16;

fn validate_proxy_chain(agent_id: &str, proxy_chain: &[String]) -> Result<(), HubError> {
    if proxy_chain.len() > MAX_PROXY_CHAIN_LEN {
        return Err(HubError::InvalidRegistry(format!(
            "agent {agent_id:?} proxyChain has {} entries; maximum is {MAX_PROXY_CHAIN_LEN}",
            proxy_chain.len()
        )));
    }
    for (index, proxy_id) in proxy_chain.iter().enumerate() {
        if proxy_chain[..index].contains(proxy_id) {
            return Err(HubError::InvalidRegistry(format!(
                "agent {agent_id:?} proxyChain contains duplicate proxy {proxy_id:?}"
            )));
        }
    }
    Ok(())
}

impl Registry {
    /// Path to the registry file inside `home`.
    pub fn path(home: &Path) -> PathBuf {
        home.join("agents.json")
    }

    /// Load and validate the registry from disk. Missing file ⇒ empty registry.
    pub fn load(home: &Path) -> Result<Self, HubError> {
        harden_home(home)?;
        let path = Self::path(home);
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                harden_sensitive_file(&path)?;
                Self::parse(&text)
            }
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
        // proxyChain references are validated at send time (proxies may be
        // added after agents), but obviously-missing ones are reported early.
        for (id, agent) in &self.agents {
            validate_proxy_chain(id, &agent.proxy_chain)?;
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
    pub fn save(&self, home: &Path) -> Result<(), HubError> {
        harden_home(home)?;
        let path = Self::path(home);
        let text = serde_json::to_string_pretty(self)?;
        let tmp = home.join(format!(
            ".agents.{}.json.tmp",
            uuid::Uuid::new_v4().simple()
        ));
        let result = (|| -> Result<(), std::io::Error> {
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&tmp)?;
            harden_sensitive_file(&tmp)?;
            file.write_all(text.as_bytes())?;
            file.sync_all()?;
            drop(file);
            replace_file(&tmp, &path)?;
            harden_sensitive_file(&path)
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        result?;
        Ok(())
    }

    /// Current fingerprint of the registry file (mtime + length), or `None`
    /// if the file does not exist.
    pub fn fingerprint(home: &Path) -> Result<Option<FileFingerprint>, HubError> {
        let path = Self::path(home);
        match std::fs::read(&path) {
            Ok(bytes) => {
                let meta = std::fs::metadata(path)?;
                let mut hasher = DefaultHasher::new();
                hasher.write(&bytes);
                Ok(Some(FileFingerprint {
                    mtime: meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                    len: bytes.len() as u64,
                    content_hash: hasher.finish(),
                }))
            }
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
        validate_proxy_chain(agent_id, &agent.proxy_chain)?;
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
        let mut next = self.clone();
        next.agents.insert(id, config);
        next.validate()?;
        *self = next;
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
        let mut next = self.clone();
        next.proxies.insert(id, config);
        next.validate()?;
        *self = next;
        Ok(())
    }

    pub fn remove_proxy(&mut self, id: &str) -> Result<(), HubError> {
        if !self.proxies.contains_key(id) {
            return Err(HubError::not_found("proxy", id));
        }
        let referenced_by: Vec<&str> = self
            .agents
            .iter()
            .filter_map(|(agent_id, agent)| {
                agent
                    .proxy_chain
                    .iter()
                    .any(|proxy_id| proxy_id == id)
                    .then_some(agent_id.as_str())
            })
            .collect();
        if !referenced_by.is_empty() {
            return Err(HubError::InvalidRegistry(format!(
                "cannot remove proxy {id:?}; referenced by agent(s): {}",
                referenced_by.join(", ")
            )));
        }
        self.proxies.remove(id);
        Ok(())
    }
}

pub(crate) fn harden_home(home: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(home)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(home, std::fs::Permissions::from_mode(0o700))?;
    }
    #[cfg(windows)]
    set_windows_owner_acl(home, true)?;
    Ok(())
}

pub(crate) fn harden_sensitive_file(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(windows)]
    set_windows_owner_acl(path, false)?;
    Ok(())
}

#[cfg(windows)]
fn set_windows_owner_acl(path: &Path, directory: bool) -> std::io::Result<()> {
    use std::ffi::c_void;
    use std::os::windows::ffi::OsStrExt;
    use std::ptr;

    const SDDL_REVISION_1: u32 = 1;
    const DACL_SECURITY_INFORMATION: u32 = 0x0000_0004;
    const PROTECTED_DACL_SECURITY_INFORMATION: u32 = 0x8000_0000;

    #[link(name = "Advapi32")]
    unsafe extern "system" {
        fn ConvertStringSecurityDescriptorToSecurityDescriptorW(
            string_security_descriptor: *const u16,
            string_sd_revision: u32,
            security_descriptor: *mut *mut c_void,
            security_descriptor_size: *mut u32,
        ) -> i32;
        fn SetFileSecurityW(
            file_name: *const u16,
            security_information: u32,
            security_descriptor: *mut c_void,
        ) -> i32;
    }
    #[link(name = "Kernel32")]
    unsafe extern "system" {
        fn LocalFree(memory: *mut c_void) -> *mut c_void;
    }

    // `OW` is the Windows OWNER RIGHTS SID. System and Administrators retain
    // recovery access; the protected DACL removes inherited grants. Directory
    // ACEs inherit to new files and subdirectories.
    let sddl = if directory {
        "D:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;OICI;FA;;;OW)"
    } else {
        "D:P(A;;FA;;;SY)(A;;FA;;;BA)(A;;FA;;;OW)"
    };
    let sddl: Vec<u16> = std::ffi::OsStr::new(sddl)
        .encode_wide()
        .chain(Some(0))
        .collect();
    let path: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut descriptor = ptr::null_mut();
    // SAFETY: both inputs are NUL-terminated UTF-16 buffers; the descriptor is
    // owned by LocalAlloc and is released with LocalFree below.
    let converted = unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            SDDL_REVISION_1,
            &mut descriptor,
            ptr::null_mut(),
        )
    };
    if converted == 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: `descriptor` was produced by the conversion call and `path`
    // remains alive for the duration of this synchronous operation.
    let applied = unsafe {
        SetFileSecurityW(
            path.as_ptr(),
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            descriptor,
        )
    };
    let error = (applied == 0).then(std::io::Error::last_os_error);
    // SAFETY: `descriptor` is a LocalAlloc allocation returned above.
    unsafe {
        LocalFree(descriptor);
    }
    error.map_or(Ok(()), Err)
}

#[cfg(not(windows))]
fn replace_file(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::rename(from, to)
}

#[cfg(windows)]
fn replace_file(from: &Path, to: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;

    const MOVEFILE_REPLACE_EXISTING: u32 = 0x1;
    const MOVEFILE_WRITE_THROUGH: u32 = 0x8;

    #[link(name = "Kernel32")]
    unsafe extern "system" {
        fn MoveFileExW(
            existing_file_name: *const u16,
            new_file_name: *const u16,
            flags: u32,
        ) -> i32;
    }

    let from_wide: Vec<u16> = from.as_os_str().encode_wide().chain(Some(0)).collect();
    let to_wide: Vec<u16> = to.as_os_str().encode_wide().chain(Some(0)).collect();
    // SAFETY: both pointers reference NUL-terminated UTF-16 buffers that live
    // for the duration of the synchronous Win32 call.
    let replaced = unsafe {
        MoveFileExW(
            from_wide.as_ptr(),
            to_wide.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if replaced == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// Validate an agent/proxy id: non-empty, only `[A-Za-z0-9_.-]`.
fn is_valid_id(s: &str) -> bool {
    !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'.' || b == b'-')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_stdio_projection_hides_command_arguments_and_values() {
        let private_command = r"C:\Users\private\agent.exe";
        let config = AgentEndpointConfig {
            transport: AgentTransport::Stdio {
                command: private_command.to_string(),
                args: vec!["--token".to_string(), "private-value".to_string()],
                env: BTreeMap::from([
                    ("TOKEN".to_string(), "private-token".to_string()),
                    ("VISIBLE_NAME".to_string(), "private-name".to_string()),
                ]),
            },
            proxy_chain: Vec::new(),
            permission_policy: PermissionPolicy::Reject,
            client_capabilities: ClientCapabilityConfig::default(),
        };

        let public = public_endpoint_config(EndpointConfigRef::Agent(&config));
        let encoded = serde_json::to_string(&public).expect("public endpoint serializes");
        assert!(!encoded.contains(private_command));
        assert!(!encoded.contains("--token"));
        assert!(!encoded.contains("private-value"));
        assert!(!encoded.contains("private-token"));
        assert!(!encoded.contains("private-name"));
        assert!(encoded.contains("<redacted-command>"));
        assert!(encoded.contains("\"TOKEN\":\"<redacted>\""));
        assert!(encoded.contains("\"VISIBLE_NAME\":\"<redacted>\""));
    }
}
