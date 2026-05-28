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
}

impl Default for FeedView {
    fn default() -> Self {
        Self {
            selected_agent: "main".into(),
            agent_index: 0,
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
        let (list, mut state) = entries_block(ring);
        // Stateful render with the last entry selected → ratatui scrolls so the
        // most recent activity stays visible at the bottom. Without this, items
        // append to the back of the VecDeque but get clipped below the viewport
        // and the user sees only the very first frame's worth of entries forever.
        f.render_stateful_widget(list, chunks[2], &mut state);
        f.render_widget(agent_picker(buffers, &view.selected_agent), chunks[3]);
    })?;
    Ok(())
}

fn header(view: &FeedView) -> Paragraph<'static> {
    let title = format!(
        " oby — agent: {}   (Ctrl-G: back to claude   ←/→: switch agent   q: quit)",
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

fn entries_block(ring: Option<&AgentRing>) -> (List<'static>, ListState) {
    let items: Vec<ListItem> = match ring {
        None => vec![ListItem::new("no entries yet")],
        Some(r) => r.entries.iter().map(format_entry).collect(),
    };
    let mut state = ListState::default();
    if !items.is_empty() {
        state.select(Some(items.len() - 1));
    }
    let list = List::new(items).block(Block::default().borders(Borders::ALL).title("activity"));
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
}
