use oby_core::{DisplayEntry, DisplayEntryUpdate, EntryBody};
use std::collections::{HashMap, VecDeque};

const DEFAULT_CAPACITY: usize = 500;

#[derive(Default)]
pub struct AllAgentBuffers {
    /// agent_key → AgentRing
    inner: HashMap<String, AgentRing>,
    /// Updates that arrived before their matching entry. Each hook invocation
    /// opens its own connection to control.sock and the listener handles them
    /// concurrently, so a PostToolUse Update can race ahead of its PreToolUse
    /// Entry on a fast tool (e.g. a small Read). Stash by tool_use_id and apply
    /// in push_entry when the entry shows up.
    orphan_updates: HashMap<String, DisplayEntryUpdate>,
}

pub struct AgentRing {
    pub agent_key: String,
    pub agent_type: Option<String>,
    pub entries: VecDeque<EntryRecord>,
    pub capacity: usize,
}

/// An entry + any live bytes attached to its LiveStream.
pub struct EntryRecord {
    pub entry: DisplayEntry,
    /// For LiveStream entries, raw bytes accumulated from oby-tee, with stream-name tags.
    pub live: Vec<LiveChunk>,
}

pub struct LiveChunk {
    pub stream: String, // "stdout" | "stderr" | "stderr-discarded" | ...
    pub bytes: Vec<u8>,
}

impl AllAgentBuffers {
    pub fn push_entry(&mut self, entry: DisplayEntry) {
        let key = entry.agent_key.clone();
        let tool_use_id = entry.tool_use_id.clone();
        // Drain any orphan update *before* taking the mutable borrow on a ring.
        let orphan = self.orphan_updates.remove(&tool_use_id);

        let ring = self.inner.entry(key.clone()).or_insert_with(|| AgentRing {
            agent_key: key,
            agent_type: None,
            entries: VecDeque::with_capacity(DEFAULT_CAPACITY),
            capacity: DEFAULT_CAPACITY,
        });
        if ring.entries.len() >= ring.capacity {
            ring.entries.pop_front();
        }
        ring.entries.push_back(EntryRecord {
            entry,
            live: Vec::new(),
        });
        if let Some(upd) = orphan {
            // Safe: we just pushed; back_mut() is the entry we want.
            let rec = ring.entries.back_mut().expect("just pushed");
            merge_update_into(rec, upd);
        }
    }

    /// Returns true iff the update was orphaned (no matching entry, stashed
    /// for a later push_entry to drain). Callers use this to count orphans
    /// in the metrics bar.
    pub fn apply_update(&mut self, upd: DisplayEntryUpdate) -> bool {
        for ring in self.inner.values_mut() {
            if let Some(rec) = ring
                .entries
                .iter_mut()
                .rfind(|r| r.entry.tool_use_id == upd.tool_use_id)
            {
                merge_update_into(rec, upd);
                return false;
            }
        }
        // No matching entry yet — stash and let push_entry apply it later.
        self.orphan_updates.insert(upd.tool_use_id.clone(), upd);
        true
    }

    pub fn append_live(
        &mut self,
        agent_key: &str,
        tool_use_id: &str,
        stream: &str,
        bytes: Vec<u8>,
    ) {
        let Some(ring) = self.inner.get_mut(agent_key) else {
            return;
        };
        if let Some(rec) = ring
            .entries
            .iter_mut()
            .rfind(|r| r.entry.tool_use_id == tool_use_id)
        {
            rec.live.push(LiveChunk {
                stream: stream.to_string(),
                bytes,
            });
        }
    }

    /// Iterate agents in a stable order: "main" first, then the rest alphabetically.
    /// HashMap iteration order would otherwise shuffle the feed picker between renders.
    pub fn agents(&self) -> impl Iterator<Item = &AgentRing> {
        let mut sorted: Vec<&AgentRing> = self.inner.values().collect();
        sorted.sort_by(|a, b| {
            let ak = a.agent_key.as_str();
            let bk = b.agent_key.as_str();
            match (ak == "main", bk == "main") {
                (true, true) => std::cmp::Ordering::Equal,
                (true, false) => std::cmp::Ordering::Less,
                (false, true) => std::cmp::Ordering::Greater,
                _ => ak.cmp(bk),
            }
        });
        sorted.into_iter()
    }

    pub fn get(&self, agent_key: &str) -> Option<&AgentRing> {
        self.inner.get(agent_key)
    }
}

fn merge_update_into(rec: &mut EntryRecord, upd: DisplayEntryUpdate) {
    rec.entry.status = upd.status;
    if let Some(body) = upd.append_body {
        // For Text, replace the body; for None, leave; for LiveStream, ignore.
        if !matches!(rec.entry.body, EntryBody::LiveStream { .. }) {
            rec.entry.body = body;
        } else if let EntryBody::Text { text } = body {
            // For live-stream entries, attach final text as an extra chunk.
            rec.live.push(LiveChunk {
                stream: "post-summary".into(),
                bytes: text.into_bytes(),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oby_core::EntryStatus;
    use std::time::SystemTime;

    fn entry(agent: &str, tuid: &str, body: EntryBody) -> DisplayEntry {
        DisplayEntry {
            agent_key: agent.into(),
            tool_use_id: tuid.into(),
            tool: "bash".into(),
            timestamp: SystemTime::now(),
            headline: "h".into(),
            body,
            status: EntryStatus::Pending,
        }
    }

    #[test]
    fn push_routes_by_agent_key() {
        let mut b = AllAgentBuffers::default();
        b.push_entry(entry("main", "t1", EntryBody::None));
        b.push_entry(entry("agent_a", "t2", EntryBody::None));
        assert_eq!(b.get("main").unwrap().entries.len(), 1);
        assert_eq!(b.get("agent_a").unwrap().entries.len(), 1);
    }

    #[test]
    fn update_before_entry_is_applied_when_entry_arrives() {
        // The PostToolUse Update can race ahead of the PreToolUse Entry on a
        // fast tool (e.g. small Read). The orphan-update buffer must hold the
        // update until push_entry brings the matching entry in.
        let mut b = AllAgentBuffers::default();
        b.apply_update(DisplayEntryUpdate {
            tool_use_id: "t1".into(),
            status: EntryStatus::Ok,
            append_body: Some(EntryBody::Text {
                text: "5 bytes".into(),
            }),
        });
        // No matching entry — update is parked, not lost.
        assert!(b.get("main").is_none());

        b.push_entry(entry("main", "t1", EntryBody::None));

        let rec = b.get("main").unwrap().entries.front().unwrap();
        assert_eq!(rec.entry.status, EntryStatus::Ok);
        assert!(matches!(&rec.entry.body, EntryBody::Text { text } if text == "5 bytes"));
    }

    #[test]
    fn update_marks_status() {
        let mut b = AllAgentBuffers::default();
        b.push_entry(entry("main", "t1", EntryBody::None));
        b.apply_update(DisplayEntryUpdate {
            tool_use_id: "t1".into(),
            status: EntryStatus::Ok,
            append_body: Some(EntryBody::Text {
                text: "done".into(),
            }),
        });
        let ring = b.get("main").unwrap();
        let rec = ring.entries.front().unwrap();
        assert_eq!(rec.entry.status, EntryStatus::Ok);
    }

    #[test]
    fn append_live_attaches_chunks_to_matching_entry() {
        let mut b = AllAgentBuffers::default();
        b.push_entry(entry(
            "main",
            "t1",
            EntryBody::LiveStream {
                tool_use_id: "t1".into(),
            },
        ));
        b.append_live("main", "t1", "stderr-discarded", b"oh no\n".to_vec());
        let rec = &b.get("main").unwrap().entries[0];
        assert_eq!(rec.live.len(), 1);
        assert_eq!(rec.live[0].stream, "stderr-discarded");
        assert_eq!(rec.live[0].bytes, b"oh no\n");
    }
}
