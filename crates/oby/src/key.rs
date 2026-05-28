use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum ViewState {
    Claude,
    Feed,
}

impl ViewState {
    pub fn toggle(self) -> Self {
        match self {
            ViewState::Claude => ViewState::Feed,
            ViewState::Feed => ViewState::Claude,
        }
    }
}

/// Decision the input loop makes per keystroke from the real terminal.
pub enum InputDecision {
    /// Toggle the view state.
    ToggleView,
    /// Forward this raw byte sequence to claude's pty.
    Forward(Vec<u8>),
    /// Consume — used in Feed view for picker nav (up/down/enter).
    NavigateFeed(FeedNav),
}

pub enum FeedNav {
    AgentPrev,
    AgentNext,
    ScrollUp,
    ScrollDown,
    Quit,
}

/// The reserved toggle key. Configurable in a later plan; v0.1 hard-codes Ctrl-G.
pub fn is_toggle(ev: &KeyEvent) -> bool {
    ev.code == KeyCode::Char('g') && ev.modifiers.contains(KeyModifiers::CONTROL)
}

/// Decide what to do with a key event given the current view state.
pub fn decide(ev: KeyEvent, state: ViewState) -> InputDecision {
    if is_toggle(&ev) {
        return InputDecision::ToggleView;
    }
    match state {
        ViewState::Claude => InputDecision::Forward(serialize_key_for_pty(ev)),
        ViewState::Feed => match ev.code {
            KeyCode::Up => InputDecision::NavigateFeed(FeedNav::ScrollUp),
            KeyCode::Down => InputDecision::NavigateFeed(FeedNav::ScrollDown),
            KeyCode::Left => InputDecision::NavigateFeed(FeedNav::AgentPrev),
            KeyCode::Right => InputDecision::NavigateFeed(FeedNav::AgentNext),
            KeyCode::Char('q') => InputDecision::NavigateFeed(FeedNav::Quit),
            _ => InputDecision::Forward(Vec::new()),
        },
    }
}

/// Serialize a key event into bytes a pty understands.
pub fn serialize_key_for_pty(ev: KeyEvent) -> Vec<u8> {
    match ev.code {
        KeyCode::Char(c) => {
            if ev.modifiers.contains(KeyModifiers::CONTROL) {
                // ASCII control: Ctrl-A = 0x01, Ctrl-Z = 0x1A, etc.
                let lo = c.to_ascii_lowercase();
                if lo.is_ascii_lowercase() {
                    return vec![(lo as u8) - b'a' + 1];
                }
            }
            let mut s = String::new();
            s.push(c);
            s.into_bytes()
        }
        KeyCode::Enter => vec![b'\r'],
        KeyCode::Tab => vec![b'\t'],
        KeyCode::Backspace => vec![0x7f],
        KeyCode::Esc => vec![0x1b],
        KeyCode::Up => b"\x1b[A".to_vec(),
        KeyCode::Down => b"\x1b[B".to_vec(),
        KeyCode::Right => b"\x1b[C".to_vec(),
        KeyCode::Left => b"\x1b[D".to_vec(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(c: char, mods: KeyModifiers) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), mods)
    }

    #[test]
    fn toggle_state() {
        assert_eq!(ViewState::Claude.toggle(), ViewState::Feed);
        assert_eq!(ViewState::Feed.toggle(), ViewState::Claude);
    }

    #[test]
    fn ctrl_g_is_toggle() {
        assert!(is_toggle(&key('g', KeyModifiers::CONTROL)));
        assert!(!is_toggle(&key('g', KeyModifiers::NONE)));
        assert!(!is_toggle(&key('f', KeyModifiers::CONTROL)));
    }

    #[test]
    fn ctrl_a_serializes_to_0x01() {
        let bytes = serialize_key_for_pty(key('a', KeyModifiers::CONTROL));
        assert_eq!(bytes, vec![0x01]);
    }

    #[test]
    fn enter_is_cr() {
        let ev = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(serialize_key_for_pty(ev), vec![b'\r']);
    }

    #[test]
    fn ctrl_g_decides_toggle_in_either_state() {
        let ev = key('g', KeyModifiers::CONTROL);
        assert!(matches!(
            decide(ev, ViewState::Claude),
            InputDecision::ToggleView
        ));
        assert!(matches!(
            decide(ev, ViewState::Feed),
            InputDecision::ToggleView
        ));
    }
}
