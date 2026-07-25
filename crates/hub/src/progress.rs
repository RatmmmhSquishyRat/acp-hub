//! Blocking-command progress + timings (Operator UX Phase 3 / SYSTEM §F.4).
//! Pure stage accounting — no I/O. CLI/daemon attach writers.

use std::time::Instant;

use serde::Serialize;

/// F.4 stage names (only emit stages that actually ran).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgressStage {
    DaemonConnect,
    AgentSpawn,
    Initialize,
    SessionOp,
    Prompt,
    End,
}

impl ProgressStage {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::DaemonConnect => "daemon_connect",
            Self::AgentSpawn => "agent_spawn",
            Self::Initialize => "initialize",
            Self::SessionOp => "session_op",
            Self::Prompt => "prompt",
            Self::End => "end",
        }
    }

    fn timings_key(self) -> Option<&'static str> {
        match self {
            Self::DaemonConnect => Some("daemonMs"),
            Self::AgentSpawn => Some("agentSpawnMs"),
            Self::Initialize => Some("initializeMs"),
            Self::SessionOp => Some("sessionMs"),
            Self::Prompt => Some("promptMs"),
            Self::End => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProgressEvent {
    #[serde(rename = "type")]
    pub type_name: &'static str,
    pub stage: String,
    pub at_ms: u64,
}

#[derive(Debug, Clone, Serialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Timings {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub daemon_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_spawn_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub initialize_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_ms: Option<u64>,
    pub total_ms: u64,
}

/// Records stage enter times; durations are measured between consecutive marks.
#[derive(Debug)]
pub struct ProgressTracker {
    started: Instant,
    last_mark: Instant,
    current: Option<ProgressStage>,
    timings: Timings,
    events: Vec<ProgressEvent>,
}

impl Default for ProgressTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl ProgressTracker {
    pub fn new() -> Self {
        let now = Instant::now();
        Self {
            started: now,
            last_mark: now,
            current: None,
            timings: Timings::default(),
            events: Vec::new(),
        }
    }

    /// Enter a stage (closes previous stage duration if any).
    pub fn stage(&mut self, stage: ProgressStage) -> ProgressEvent {
        self.close_current();
        self.current = Some(stage);
        self.last_mark = Instant::now();
        let event = ProgressEvent {
            type_name: "progress",
            stage: stage.as_str().to_string(),
            at_ms: self.elapsed_ms(),
        };
        self.events.push(event.clone());
        event
    }

    pub fn finish(&mut self) -> (ProgressEvent, Timings) {
        self.close_current();
        self.current = Some(ProgressStage::End);
        let event = ProgressEvent {
            type_name: "progress",
            stage: ProgressStage::End.as_str().to_string(),
            at_ms: self.elapsed_ms(),
        };
        self.events.push(event.clone());
        self.timings.total_ms = self.elapsed_ms();
        self.current = None;
        (event, self.timings.clone())
    }

    pub fn events(&self) -> &[ProgressEvent] {
        &self.events
    }

    pub fn timings(&self) -> &Timings {
        &self.timings
    }

    pub fn human_stage_line(stage: &str) -> String {
        format!("[acp-hub] stage={stage}")
    }

    fn elapsed_ms(&self) -> u64 {
        self.started.elapsed().as_millis() as u64
    }

    fn close_current(&mut self) {
        if let Some(stage) = self.current.take() {
            let ms = self.last_mark.elapsed().as_millis() as u64;
            match stage.timings_key() {
                Some("daemonMs") => self.timings.daemon_ms = Some(ms),
                Some("agentSpawnMs") => self.timings.agent_spawn_ms = Some(ms),
                Some("initializeMs") => self.timings.initialize_ms = Some(ms),
                Some("sessionMs") => self.timings.session_ms = Some(ms),
                Some("promptMs") => self.timings.prompt_ms = Some(ms),
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_path_emits_session_and_total() {
        let mut t = ProgressTracker::new();
        let e1 = t.stage(ProgressStage::DaemonConnect);
        assert_eq!(e1.stage, "daemon_connect");
        assert_eq!(e1.type_name, "progress");
        let _ = t.stage(ProgressStage::SessionOp);
        let (end, timings) = t.finish();
        assert_eq!(end.stage, "end");
        assert!(timings.total_ms < u64::MAX);
        assert!(timings.daemon_ms.is_some());
        assert!(timings.session_ms.is_some());
        assert!(timings.prompt_ms.is_none()); // skipped stages omit keys
    }

    #[test]
    fn send_path_records_prompt_ms() {
        let mut t = ProgressTracker::new();
        t.stage(ProgressStage::DaemonConnect);
        t.stage(ProgressStage::Prompt);
        let (_, timings) = t.finish();
        assert!(timings.prompt_ms.is_some());
        assert!(timings.session_ms.is_none());
    }
}
