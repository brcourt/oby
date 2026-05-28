use crate::{DisplayEntry, DisplayEntryUpdate};
use serde::{Deserialize, Serialize};

/// Sent from oby-hook to the wrapper's control socket. JSON-line framed.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ControlMessage {
    /// A new entry to add to the agent's timeline.
    Entry { v: u8, entry: DisplayEntry },
    /// Update to an existing entry (correlated by tool_use_id).
    Update { v: u8, update: DisplayEntryUpdate },
}

impl ControlMessage {
    pub fn entry(entry: DisplayEntry) -> Self {
        Self::Entry { v: 1, entry }
    }
    pub fn update(update: DisplayEntryUpdate) -> Self {
        Self::Update { v: 1, update }
    }
}

/// Sent from oby-tee on connection open, before any bytes. One JSON line, then raw bytes until EOF.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeaderLine {
    pub v: u8,
    pub tool_use_id: String,
    /// Sub-stream name: "stdout", "stderr", "stderr-discarded", etc. Capturer-defined.
    pub stream: String,
    /// Unix timestamp (seconds since epoch), prefixed with "@". v0.1 stopgap — no external chrono dep.
    pub started_at: String,
}

impl HeaderLine {
    pub fn new(tool_use_id: impl Into<String>, stream: impl Into<String>) -> Self {
        use std::time::{SystemTime, UNIX_EPOCH};
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        // Minimal timestamp marker — no external chrono dep. v0.1 just stores secs since epoch.
        let started_at = format!("@{}", now.as_secs());
        Self {
            v: 1,
            tool_use_id: tool_use_id.into(),
            stream: stream.into(),
            started_at,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EntryBody, EntryStatus};
    use std::time::SystemTime;

    #[test]
    fn control_entry_roundtrips() {
        let msg = ControlMessage::entry(DisplayEntry {
            agent_key: "main".into(),
            tool_use_id: "t1".into(),
            tool: "bash".into(),
            timestamp: SystemTime::UNIX_EPOCH,
            headline: "ls".into(),
            body: EntryBody::None,
            status: EntryStatus::Pending,
        });
        let s = serde_json::to_string(&msg).unwrap();
        assert!(s.contains("\"kind\":\"entry\""));
        let parsed: ControlMessage = serde_json::from_str(&s).unwrap();
        match parsed {
            ControlMessage::Entry { v, entry } => {
                assert_eq!(v, 1);
                assert_eq!(entry.tool_use_id, "t1");
            }
            _ => panic!("expected Entry"),
        }
    }

    #[test]
    fn header_line_roundtrips() {
        let h = HeaderLine::new("t1", "stderr-discarded");
        let s = serde_json::to_string(&h).unwrap();
        let parsed: HeaderLine = serde_json::from_str(&s).unwrap();
        assert_eq!(parsed.tool_use_id, "t1");
        assert_eq!(parsed.stream, "stderr-discarded");
    }
}
