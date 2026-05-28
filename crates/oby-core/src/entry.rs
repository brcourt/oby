use serde::{Deserialize, Serialize};
use std::time::SystemTime;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DisplayEntry {
    pub agent_key: String,
    pub tool_use_id: String,
    pub tool: String,
    #[serde(with = "ts_millis")]
    pub timestamp: SystemTime,
    pub headline: String,
    pub body: EntryBody,
    pub status: EntryStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EntryBody {
    None,
    Text {
        text: String,
    },
    /// Live byte stream; the wrapper binds it to the matching oby-tee connection by tool_use_id.
    LiveStream {
        tool_use_id: String,
    },
    /// Pre-rendered structured diff (for Edit/Write — placeholder in v0.1).
    Diff {
        hunks: Vec<DiffHunk>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiffHunk {
    pub old_start: u32,
    pub old_lines: Vec<String>,
    pub new_start: u32,
    pub new_lines: Vec<String>,
}

#[derive(Debug, Copy, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EntryStatus {
    Pending,
    Ok,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DisplayEntryUpdate {
    pub tool_use_id: String,
    pub status: EntryStatus,
    #[serde(default)]
    pub append_body: Option<EntryBody>,
}

/// Serialize SystemTime as integer milliseconds since epoch for JSON wire stability.
mod ts_millis {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    pub fn serialize<S: Serializer>(t: &SystemTime, s: S) -> Result<S::Ok, S::Error> {
        let ms = t
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        (ms as u64).serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<SystemTime, D::Error> {
        let ms = u64::deserialize(d)?;
        Ok(UNIX_EPOCH + Duration::from_millis(ms))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entry_roundtrips_through_json() {
        let entry = DisplayEntry {
            agent_key: "main".into(),
            tool_use_id: "toolu_01".into(),
            tool: "bash".into(),
            timestamp: SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1716840000),
            headline: "echo MAIN_PROBE".into(),
            body: EntryBody::LiveStream {
                tool_use_id: "toolu_01".into(),
            },
            status: EntryStatus::Pending,
        };
        let json = serde_json::to_string(&entry).unwrap();
        let parsed: DisplayEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.agent_key, "main");
        assert_eq!(parsed.status, EntryStatus::Pending);
        match parsed.body {
            EntryBody::LiveStream { tool_use_id } => assert_eq!(tool_use_id, "toolu_01"),
            _ => panic!("expected LiveStream"),
        }
    }

    #[test]
    fn update_roundtrips() {
        let upd = DisplayEntryUpdate {
            tool_use_id: "toolu_01".into(),
            status: EntryStatus::Ok,
            append_body: Some(EntryBody::Text {
                text: "exit 0".into(),
            }),
        };
        let json = serde_json::to_string(&upd).unwrap();
        let parsed: DisplayEntryUpdate = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.status, EntryStatus::Ok);
    }
}
