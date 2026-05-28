use crate::metrics::{fd_count, format_bytes, format_uptime, Metrics};
use crate::ring::{AgentRing, AllAgentBuffers, EntryRecord};
use oby_core::{EntryBody, EntryStatus};
use ratatui::{
    backend::Backend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph},
    Terminal,
};

pub struct FeedView {
    pub selected_agent: String,
    pub agent_index: usize,
    /// How many entries up from the bottom (tail) the user has scrolled.
    /// 0 == auto-tail (latest entry visible). >0 == paused at an older
    /// position; new entries still arrive but the visible window stays
    /// anchored relative to the bottom of the ring.
    pub scroll_offset: usize,
}

impl Default for FeedView {
    fn default() -> Self {
        Self {
            selected_agent: "main".into(),
            agent_index: 0,
            scroll_offset: 0,
        }
    }
}

impl FeedView {
    pub fn cycle_agent(&mut self, buffers: &AllAgentBuffers, dir: i32) {
        let keys: Vec<String> = buffers.agents().map(|a| a.agent_key.clone()).collect();
        if keys.is_empty() {
            return;
        }
        let new = ((self.agent_index as i32 + dir).rem_euclid(keys.len() as i32)) as usize;
        self.agent_index = new;
        self.selected_agent = keys[new].clone();
        // Switching agents implies "show me what's happening" — back to tail.
        self.scroll_offset = 0;
    }

    fn ring_len(&self, buffers: &AllAgentBuffers) -> usize {
        buffers
            .get(&self.selected_agent)
            .map(|r| r.entries.len())
            .unwrap_or(0)
    }

    pub fn scroll_up(&mut self, buffers: &AllAgentBuffers, by: usize) {
        let max = self.ring_len(buffers).saturating_sub(1);
        self.scroll_offset = (self.scroll_offset + by).min(max);
    }

    pub fn scroll_down(&mut self, by: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(by);
    }

    pub fn scroll_to_top(&mut self, buffers: &AllAgentBuffers) {
        self.scroll_offset = self.ring_len(buffers).saturating_sub(1);
    }

    pub fn scroll_to_bottom(&mut self) {
        self.scroll_offset = 0;
    }
}

pub fn render<B: Backend>(
    term: &mut Terminal<B>,
    view: &FeedView,
    buffers: &AllAgentBuffers,
    metrics: &Metrics,
) -> anyhow::Result<()> {
    term.draw(|f| {
        let size = f.size();
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1), // metrics bar
                Constraint::Length(1), // title bar
                Constraint::Min(1),    // entries
                Constraint::Length(3), // agent picker
            ])
            .split(size);

        f.render_widget(metrics_bar(metrics, buffers), chunks[0]);
        f.render_widget(header(view), chunks[1]);
        let ring = buffers.get(&view.selected_agent);
        let (list, mut state) = entries_block(ring, view.scroll_offset);
        // Stateful render: ratatui scrolls the viewport so the selected entry
        // is visible. We pick the selection based on scroll_offset (0 = tail).
        f.render_stateful_widget(list, chunks[2], &mut state);
        f.render_widget(agent_picker(buffers, &view.selected_agent), chunks[3]);
    })?;
    Ok(())
}

fn header(view: &FeedView) -> Paragraph<'static> {
    let title = format!(
        " oby — agent: {}   (Ctrl-G claude · ←/→ agent · ↑/↓ scroll · PgUp/PgDn · g/G or Home/End · q quit)",
        view.selected_agent
    );
    Paragraph::new(title).style(Style::default().add_modifier(Modifier::REVERSED))
}

fn metrics_bar(m: &Metrics, buffers: &AllAgentBuffers) -> Paragraph<'static> {
    let agents = buffers.agents().count();
    let fd = fd_count()
        .map(|n| n.to_string())
        .unwrap_or_else(|| "?".into());
    let line = format!(
        " agents {} · entries {} · updates {} ({} orph) · bytes {} · conns {} · err {}/{} · fd {} · up {}",
        agents,
        m.entries_received,
        m.updates_received,
        m.updates_orphaned,
        format_bytes(m.agent_bytes),
        m.agent_connections,
        m.accept_errors,
        m.parse_errors,
        fd,
        format_uptime(m.uptime()),
    );
    Paragraph::new(line).style(Style::default().fg(Color::DarkGray))
}

fn entries_block(ring: Option<&AgentRing>, scroll_offset: usize) -> (List<'static>, ListState) {
    let items: Vec<ListItem> = match ring {
        None => vec![ListItem::new("no entries yet")],
        Some(r) => r.entries.iter().map(format_entry).collect(),
    };
    let mut state = ListState::default();
    if !items.is_empty() {
        let last = items.len() - 1;
        let selected = last.saturating_sub(scroll_offset);
        state.select(Some(selected));
    }
    let title = if scroll_offset > 0 {
        format!("activity  [scrolled +{scroll_offset} · End/G to tail]")
    } else {
        "activity".into()
    };
    let list = List::new(items).block(Block::default().borders(Borders::ALL).title(title));
    (list, state)
}

fn format_entry(rec: &EntryRecord) -> ListItem<'static> {
    let mut lines: Vec<Line> = Vec::new();
    let status_glyph = match rec.entry.status {
        EntryStatus::Pending => "▸",
        EntryStatus::Ok => "✓",
        EntryStatus::Error => "✗",
    };
    lines.push(Line::from(vec![
        Span::styled(
            status_glyph,
            Style::default().fg(status_color(rec.entry.status)),
        ),
        Span::raw(" "),
        Span::raw(rec.entry.headline.clone()),
    ]));
    if let EntryBody::Text { text } = &rec.entry.body {
        lines.push(Line::from(Span::styled(
            text.clone(),
            Style::default().fg(Color::DarkGray),
        )));
    }
    for chunk in &rec.live {
        let tag = format!("  [{}] ", chunk.stream);
        let body = String::from_utf8_lossy(&chunk.bytes);
        for line in body.lines() {
            lines.push(Line::from(vec![
                Span::styled(tag.clone(), Style::default().fg(Color::DarkGray)),
                Span::raw(line.to_string()),
            ]));
        }
    }
    ListItem::new(lines)
}

fn status_color(s: EntryStatus) -> Color {
    match s {
        EntryStatus::Pending => Color::Yellow,
        EntryStatus::Ok => Color::Green,
        EntryStatus::Error => Color::Red,
    }
}

fn agent_picker(buffers: &AllAgentBuffers, selected: &str) -> Paragraph<'static> {
    let mut spans: Vec<Span> = Vec::new();
    let agents: Vec<&AgentRing> = buffers.agents().collect();
    if agents.is_empty() {
        spans.push(Span::raw(" (no agents yet) "));
    }
    for (i, r) in agents.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw("   "));
        }
        let label = r
            .agent_type
            .as_deref()
            .map(|t| format!("{} [{}]", r.agent_key, t))
            .unwrap_or_else(|| r.agent_key.clone());
        let style = if r.agent_key == selected {
            Style::default().add_modifier(Modifier::REVERSED)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        spans.push(Span::styled(label, style));
    }
    Paragraph::new(Line::from(spans)).block(Block::default().borders(Borders::TOP).title("agents"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use oby_core::{DisplayEntry, EntryStatus};
    use std::time::SystemTime;

    fn entry(agent: &str, tuid: &str) -> DisplayEntry {
        DisplayEntry {
            agent_key: agent.into(),
            tool_use_id: tuid.into(),
            tool: "bash".into(),
            timestamp: SystemTime::now(),
            headline: "ls".into(),
            body: EntryBody::None,
            status: EntryStatus::Pending,
        }
    }

    #[test]
    fn cycle_agent_wraps_around() {
        let mut b = AllAgentBuffers::default();
        b.push_entry(entry("main", "t1"));
        b.push_entry(entry("agent_a", "t2"));
        let first_key = b.agents().next().unwrap().agent_key.clone();
        let mut v = FeedView {
            selected_agent: first_key,
            ..FeedView::default()
        };
        v.cycle_agent(&b, 1);
        assert_ne!(v.selected_agent, "");
        v.cycle_agent(&b, 1);
        v.cycle_agent(&b, 1); // wraps
    }

    #[test]
    fn scroll_clamps_at_top_and_bottom() {
        let mut b = AllAgentBuffers::default();
        for i in 0..5 {
            b.push_entry(entry("main", &format!("t{i}")));
        }
        let mut v = FeedView::default();
        // At tail by default.
        assert_eq!(v.scroll_offset, 0);
        // Scrolling down at tail is a no-op.
        v.scroll_down(3);
        assert_eq!(v.scroll_offset, 0);
        // Scroll up by 100 — clamps to len-1 = 4.
        v.scroll_up(&b, 100);
        assert_eq!(v.scroll_offset, 4);
        // Now scroll_down by 2.
        v.scroll_down(2);
        assert_eq!(v.scroll_offset, 2);
        // Jump to bottom.
        v.scroll_to_bottom();
        assert_eq!(v.scroll_offset, 0);
        // Jump to top.
        v.scroll_to_top(&b);
        assert_eq!(v.scroll_offset, 4);
    }

    #[test]
    fn switching_agent_returns_to_tail() {
        let mut b = AllAgentBuffers::default();
        for i in 0..3 {
            b.push_entry(entry("main", &format!("t{i}")));
            b.push_entry(entry("agent_a", &format!("a{i}")));
        }
        let mut v = FeedView {
            selected_agent: "main".into(),
            ..FeedView::default()
        };
        v.scroll_up(&b, 2);
        assert_eq!(v.scroll_offset, 2);
        v.cycle_agent(&b, 1);
        // Switching agent should snap back to tail on the new agent.
        assert_eq!(v.scroll_offset, 0);
    }
}
