use crate::metrics::{fd_count, format_bytes, format_uptime, Metrics};
use crate::ring::{AgentRing, AllAgentBuffers, EntryRecord};
use oby_core::{EntryBody, EntryStatus};
use ratatui::{
    backend::Backend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Terminal,
};

pub struct FeedView {
    pub selected_agent: String,
    pub agent_index: usize,
    /// Lines from the bottom of the rendered content.
    /// 0 = auto-tail (latest visible). >0 = paused that many lines above the
    /// tail; new entries continue to arrive but the visible window stays
    /// anchored relative to the bottom. Clamped to a sensible max at render
    /// time once we know the total line count and viewport height.
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

    /// Scroll up by `by` lines. No upper clamp here; the render path clamps
    /// to whatever max makes sense given the current total line count and
    /// viewport height.
    pub fn scroll_up(&mut self, by: usize) {
        self.scroll_offset = self.scroll_offset.saturating_add(by);
    }

    pub fn scroll_down(&mut self, by: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(by);
    }

    /// Jump to the oldest line. We don't know the exact max here, so set to
    /// a very large value and let the render path clamp.
    pub fn scroll_to_top(&mut self) {
        self.scroll_offset = usize::MAX / 2;
    }

    pub fn scroll_to_bottom(&mut self) {
        self.scroll_offset = 0;
    }
}

pub fn render<B: Backend>(
    term: &mut Terminal<B>,
    view: &mut FeedView,
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
        let entries_area = chunks[2];
        // Inner height = block area minus the top + bottom border lines.
        let inner_height = entries_area.height.saturating_sub(2);
        let lines = build_lines(ring);
        let total = lines.len() as u16;
        let max_scroll = total.saturating_sub(inner_height);
        // Clamp the user's requested offset to what actually makes sense.
        let effective_offset = (view.scroll_offset as u16).min(max_scroll);
        // Re-anchor the FeedView to the clamped value so the next keypress is
        // operating on a real number, not the saturated "jump to top" sentinel.
        view.scroll_offset = effective_offset as usize;
        // Paragraph::scroll wants lines-from-top; we store lines-from-bottom.
        let scroll_y = max_scroll - effective_offset;

        let title = if effective_offset > 0 {
            format!("activity  [scrolled +{effective_offset} lines · End/G to tail]")
        } else {
            "activity".into()
        };
        let p = Paragraph::new(lines)
            .block(Block::default().borders(Borders::ALL).title(title))
            .scroll((scroll_y, 0));
        f.render_widget(p, entries_area);

        f.render_widget(agent_picker(buffers, &view.selected_agent), chunks[3]);
    })?;
    Ok(())
}

fn header(view: &FeedView) -> Paragraph<'static> {
    let title = format!(
        " oby — agent: {}   (Ctrl-G claude · ←/→ agent · ↑/↓ scroll · PgUp/PgDn · g/G · d delete · q quit)",
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

fn build_lines(ring: Option<&AgentRing>) -> Vec<Line<'static>> {
    match ring {
        None => vec![Line::from("no entries yet")],
        Some(r) => {
            let mut out = Vec::with_capacity(r.entries.len() * 3);
            for rec in &r.entries {
                append_entry_lines(rec, &mut out);
            }
            out
        }
    }
}

fn append_entry_lines(rec: &EntryRecord, lines: &mut Vec<Line<'static>>) {
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
        spans.push(status_dot(r));
        spans.push(Span::raw(" "));
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

/// Green for alive (or main, always), red for destroyed. Driven by the
/// `destroyed` flag the wrapper sets when CC's SubagentStop hook fires for
/// the agent. Main is never marked destroyed — it's the session itself.
fn status_dot(ring: &AgentRing) -> Span<'static> {
    let color = if ring.destroyed {
        Color::Red
    } else {
        Color::Green
    };
    Span::styled("●", Style::default().fg(color))
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
    fn scroll_down_clamps_at_zero() {
        let mut v = FeedView::default();
        assert_eq!(v.scroll_offset, 0);
        v.scroll_down(50);
        assert_eq!(v.scroll_offset, 0);
        v.scroll_up(7);
        assert_eq!(v.scroll_offset, 7);
        v.scroll_down(3);
        assert_eq!(v.scroll_offset, 4);
        v.scroll_down(100);
        assert_eq!(v.scroll_offset, 0);
    }

    #[test]
    fn scroll_to_top_uses_sentinel_clamped_at_render() {
        // scroll_to_top doesn't know the max line count, so it uses a large
        // sentinel; the render path clamps it to the actual max. We just
        // assert here that it's set to something very large.
        let mut v = FeedView::default();
        v.scroll_to_top();
        assert!(v.scroll_offset > 1_000_000);
        v.scroll_to_bottom();
        assert_eq!(v.scroll_offset, 0);
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
        v.scroll_up(20);
        assert_eq!(v.scroll_offset, 20);
        v.cycle_agent(&b, 1);
        assert_eq!(v.scroll_offset, 0);
    }
}
