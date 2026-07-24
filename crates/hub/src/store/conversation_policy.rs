//! Pure conversation UX policy (Phase-1 contract).
//!
//! No I/O: origin / interaction / phase / busy / last_outcome recompute and
//! synthetic STATUS. Store and Hub call these; CLI only displays results.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// How the Hub conversation row entered the store.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConvOrigin {
    HubCreated,
    Bound,
    ImportedList,
}

impl ConvOrigin {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::HubCreated => "hub_created",
            Self::Bound => "bound",
            Self::ImportedList => "imported_list",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "hub_created" => Some(Self::HubCreated),
            "bound" => Some(Self::Bound),
            "imported_list" => Some(Self::ImportedList),
            _ => None,
        }
    }
}

/// Write gate truth (must match send/param/mode gate).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Interaction {
    Writable,
    ReadOnly,
}

impl Interaction {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Writable => "writable",
            Self::ReadOnly => "read_only",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "writable" => Some(Self::Writable),
            "read_only" => Some(Self::ReadOnly),
            _ => None,
        }
    }

    pub fn short(self) -> &'static str {
        match self {
            Self::Writable => "W",
            Self::ReadOnly => "R",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConvPhase {
    Open,
    Closed,
    Deleted,
}

impl ConvPhase {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Closed => "closed",
            Self::Deleted => "deleted",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "open" => Some(Self::Open),
            "closed" => Some(Self::Closed),
            "deleted" => Some(Self::Deleted),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConvBusy {
    None,
    Running,
    Cancelling,
}

impl ConvBusy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Running => "running",
            Self::Cancelling => "cancelling",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "none" => Some(Self::None),
            "running" => Some(Self::Running),
            "cancelling" => Some(Self::Cancelling),
            _ => None,
        }
    }

    pub fn is_busy(self) -> bool {
        !matches!(self, Self::None)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LastOutcome {
    None,
    Completed,
    Failed,
    Cancelled,
}

impl LastOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "none" => Some(Self::None),
            "completed" => Some(Self::Completed),
            "failed" => Some(Self::Failed),
            "cancelled" => Some(Self::Cancelled),
            _ => None,
        }
    }
}

/// Session space from SessionInfo meta (Phase-1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionSpace {
    Acp,
    Cli,
    Ide,
    Unknown,
}

impl SessionSpace {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Acp => "acp",
            Self::Cli => "cli",
            Self::Ide => "ide",
            Self::Unknown => "unknown",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "acp" => Self::Acp,
            "cli" => Self::Cli,
            "ide" => Self::Ide,
            _ => Self::Unknown,
        }
    }
}

/// Synthetic list STATUS (Phase-1 §1.2). First match wins.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyntheticStatus {
    Running,
    Cancelling,
    Closed,
    Failed,
    Cancelled,
    Completed,
    Idle,
    Deleted,
}

impl SyntheticStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Cancelling => "cancelling",
            Self::Closed => "closed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Completed => "completed",
            Self::Idle => "idle",
            Self::Deleted => "deleted",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "running" => Some(Self::Running),
            "cancelling" => Some(Self::Cancelling),
            "closed" => Some(Self::Closed),
            "failed" => Some(Self::Failed),
            "cancelled" => Some(Self::Cancelled),
            "completed" => Some(Self::Completed),
            "idle" => Some(Self::Idle),
            "deleted" => Some(Self::Deleted),
            _ => None,
        }
    }
}

/// PHASE1-CONTRACT §1.2
pub fn synthetic_status(
    phase: ConvPhase,
    busy: ConvBusy,
    last_outcome: LastOutcome,
) -> SyntheticStatus {
    if phase == ConvPhase::Deleted {
        return SyntheticStatus::Deleted;
    }
    match busy {
        ConvBusy::Running => return SyntheticStatus::Running,
        ConvBusy::Cancelling => return SyntheticStatus::Cancelling,
        ConvBusy::None => {}
    }
    if phase == ConvPhase::Closed {
        return SyntheticStatus::Closed;
    }
    match last_outcome {
        LastOutcome::Failed => SyntheticStatus::Failed,
        LastOutcome::Cancelled => SyntheticStatus::Cancelled,
        LastOutcome::Completed => SyntheticStatus::Completed,
        LastOutcome::None => SyntheticStatus::Idle,
    }
}

/// PHASE1-CONTRACT §1.4 — legacy status → hybrid fields.
pub fn backfill_from_legacy_status(status: &str) -> (ConvPhase, ConvBusy, LastOutcome) {
    match status {
        "running" => (ConvPhase::Open, ConvBusy::Running, LastOutcome::None),
        "cancelling" => (ConvPhase::Open, ConvBusy::Cancelling, LastOutcome::None),
        "completed" => (ConvPhase::Open, ConvBusy::None, LastOutcome::Completed),
        "failed" => (ConvPhase::Open, ConvBusy::None, LastOutcome::Failed),
        "cancelled" => (ConvPhase::Open, ConvBusy::None, LastOutcome::Cancelled),
        "deleted" => (ConvPhase::Deleted, ConvBusy::None, LastOutcome::None),
        // idle and unknown → open/none/none
        _ => (ConvPhase::Open, ConvBusy::None, LastOutcome::None),
    }
}

/// PHASE1-CONTRACT §2.1 — first hit wins.
pub fn parse_session_meta(meta: Option<&Value>) -> (Option<Interaction>, SessionSpace) {
    let Some(meta) = meta.and_then(|v| v.as_object()) else {
        return (None, SessionSpace::Unknown);
    };

    let explicit_ix = meta
        .get("acp_hub")
        .and_then(|v| v.get("interaction"))
        .and_then(|v| v.as_str())
        .and_then(Interaction::parse);

    let space = meta
        .get("acp_hub")
        .and_then(|v| v.get("space"))
        .and_then(|v| v.as_str())
        .map(SessionSpace::parse)
        .or_else(|| {
            meta.get("cursor-adapter")
                .and_then(|v| v.get("space"))
                .and_then(|v| v.as_str())
                .map(SessionSpace::parse)
        })
        .unwrap_or(SessionSpace::Unknown);

    (explicit_ix, space)
}

/// PHASE1-CONTRACT §2.3
pub fn recompute_interaction(origin: ConvOrigin, meta: Option<&Value>) -> Interaction {
    if origin == ConvOrigin::ImportedList {
        return Interaction::ReadOnly; // Option A
    }
    if origin == ConvOrigin::HubCreated {
        return Interaction::Writable;
    }
    // bound
    let (explicit_ix, space) = parse_session_meta(meta);
    if explicit_ix == Some(Interaction::ReadOnly) {
        return Interaction::ReadOnly;
    }
    match space {
        SessionSpace::Ide => Interaction::ReadOnly,
        SessionSpace::Acp | SessionSpace::Cli => Interaction::Writable,
        SessionSpace::Unknown => Interaction::ReadOnly,
    }
}

/// Title/cwd merge (PHASE1-CONTRACT §3.2).
pub fn merge_discover_title(
    origin: ConvOrigin,
    local: Option<&str>,
    remote: Option<&str>,
) -> Option<String> {
    merge_discover_field(origin, local, remote)
}

pub fn merge_discover_cwd(
    origin: ConvOrigin,
    local: Option<&str>,
    remote: Option<&str>,
) -> Option<String> {
    merge_discover_field(origin, local, remote)
}

fn merge_discover_field(
    origin: ConvOrigin,
    local: Option<&str>,
    remote: Option<&str>,
) -> Option<String> {
    let local_empty = local.map(|s| s.is_empty()).unwrap_or(true);
    let remote_empty = remote.map(|s| s.is_empty()).unwrap_or(true);
    if local_empty {
        return remote.filter(|s| !s.is_empty()).map(str::to_string);
    }
    if remote_empty {
        return local.map(str::to_string);
    }
    match origin {
        ConvOrigin::ImportedList => remote.map(str::to_string),
        ConvOrigin::HubCreated | ConvOrigin::Bound => local.map(str::to_string),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn synthetic_status_priority() {
        assert_eq!(
            synthetic_status(ConvPhase::Deleted, ConvBusy::Running, LastOutcome::Failed),
            SyntheticStatus::Deleted
        );
        assert_eq!(
            synthetic_status(ConvPhase::Open, ConvBusy::Running, LastOutcome::Failed),
            SyntheticStatus::Running
        );
        assert_eq!(
            synthetic_status(ConvPhase::Open, ConvBusy::Cancelling, LastOutcome::None),
            SyntheticStatus::Cancelling
        );
        assert_eq!(
            synthetic_status(ConvPhase::Closed, ConvBusy::None, LastOutcome::Completed),
            SyntheticStatus::Closed
        );
        assert_eq!(
            synthetic_status(ConvPhase::Open, ConvBusy::None, LastOutcome::Failed),
            SyntheticStatus::Failed
        );
        assert_eq!(
            synthetic_status(ConvPhase::Open, ConvBusy::None, LastOutcome::None),
            SyntheticStatus::Idle
        );
    }

    #[test]
    fn option_a_imported_always_read_only() {
        let meta = json!({"cursor-adapter": {"space": "acp"}});
        assert_eq!(
            recompute_interaction(ConvOrigin::ImportedList, Some(&meta)),
            Interaction::ReadOnly
        );
    }

    #[test]
    fn hub_created_always_writable() {
        let meta = json!({"cursor-adapter": {"space": "ide"}});
        assert_eq!(
            recompute_interaction(ConvOrigin::HubCreated, Some(&meta)),
            Interaction::Writable
        );
    }

    #[test]
    fn bound_ide_read_only_acp_writable() {
        let ide = json!({"cursor-adapter": {"space": "ide"}});
        let acp = json!({"cursor-adapter": {"space": "acp"}});
        assert_eq!(
            recompute_interaction(ConvOrigin::Bound, Some(&ide)),
            Interaction::ReadOnly
        );
        assert_eq!(
            recompute_interaction(ConvOrigin::Bound, Some(&acp)),
            Interaction::Writable
        );
        assert_eq!(
            recompute_interaction(ConvOrigin::Bound, None),
            Interaction::ReadOnly
        );
    }

    #[test]
    fn meta_prefers_acp_hub_space() {
        let meta = json!({
            "acp_hub": { "space": "cli", "interaction": "read_only" },
            "cursor-adapter": { "space": "acp" }
        });
        let (ix, space) = parse_session_meta(Some(&meta));
        assert_eq!(ix, Some(Interaction::ReadOnly));
        assert_eq!(space, SessionSpace::Cli);
        assert_eq!(
            recompute_interaction(ConvOrigin::Bound, Some(&meta)),
            Interaction::ReadOnly
        );
    }

    #[test]
    fn title_merge_matrix() {
        assert_eq!(
            merge_discover_title(ConvOrigin::HubCreated, Some("local"), Some("remote")),
            Some("local".into())
        );
        assert_eq!(
            merge_discover_title(ConvOrigin::ImportedList, Some("local"), Some("remote")),
            Some("remote".into())
        );
        assert_eq!(
            merge_discover_title(ConvOrigin::Bound, None, Some("remote")),
            Some("remote".into())
        );
    }

    #[test]
    fn legacy_backfill() {
        assert_eq!(
            backfill_from_legacy_status("failed"),
            (ConvPhase::Open, ConvBusy::None, LastOutcome::Failed)
        );
        assert_eq!(
            backfill_from_legacy_status("running"),
            (ConvPhase::Open, ConvBusy::Running, LastOutcome::None)
        );
    }
}
