use crate::key::{decide, FeedNav, InputDecision, ViewState};
use crate::metrics::Metrics;
use crate::pty::{spawn_claude, watch_child};
use crate::ring::AllAgentBuffers;
use crate::sockets::spawn_listeners;
use crate::tui::{render, FeedView};
use anyhow::Result;
use crossterm::{
    event::{
        self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
        Event, MouseEventKind,
    },
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use portable_pty::PtySize;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use uuid::Uuid;

pub fn run(rest: Vec<String>) -> ExitCode {
    // Set env BEFORE the multi-threaded tokio runtime starts. std::env::set_var
    // is unsound once worker threads exist.
    let session_id = Uuid::new_v4().simple().to_string();
    let socket_dir = runtime_dir().join("obi").join(session_id);
    std::env::set_var("OBS_ACTIVE", "1");
    std::env::set_var("OBS_SOCKET_DIR", &socket_dir);

    let rt = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("oby: failed to start runtime: {e}");
            return ExitCode::FAILURE;
        }
    };
    match rt.block_on(run_async(rest, socket_dir)) {
        Ok(_) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("oby: {e:#}");
            ExitCode::FAILURE
        }
    }
}

async fn run_async(rest: Vec<String>, socket_dir: PathBuf) -> Result<()> {
    let _cleanup = SocketDirCleanup(socket_dir.clone());

    let buffers = Arc::new(Mutex::new(AllAgentBuffers::default()));
    let metrics = Arc::new(Mutex::new(Metrics::default()));
    spawn_listeners(socket_dir, buffers.clone(), metrics.clone()).await?;

    let (cols, rows) = crossterm::terminal::size()?;
    enable_raw_mode()?;
    // Don't EnterAlternateScreen at startup — claude uses its own alt-screen
    // internally, and most terminals' "scrollback in alt-screen" feature only
    // works for the *first* alt-screen entry. If we enter first, claude's
    // entry becomes a nested one and the terminal disables scrollback. By
    // staying in the main buffer here, claude's alt-screen entry is the
    // primary one and the terminal preserves scrollback as if you ran claude
    // unwrapped. We enter alt-screen only when toggling to the feed view.
    //
    // Clear the visible screen + home the cursor so claude paints onto a
    // clean canvas instead of over the shell's prompt + history. Uses
    // ClearType::All (\x1b[2J) which preserves terminal scrollback in
    // modern terminals — the user can still scroll up to see whatever was
    // on screen before they ran `oby claude`.
    execute!(
        std::io::stdout(),
        EnableBracketedPaste,
        crossterm::terminal::Clear(crossterm::terminal::ClearType::All),
        crossterm::cursor::MoveTo(0, 0),
    )?;
    let _term_guard = TerminalGuard;
    let backend = ratatui::backend::CrosstermBackend::new(std::io::stdout());
    let mut term = ratatui::Terminal::new(backend)?;

    let mut pty = spawn_claude(rest, cols, rows)?;
    let child_done = watch_child(std::mem::replace(&mut pty.child, dummy_child()));

    let mut master_reader = pty.pair.master.try_clone_reader()?;
    let mut master_writer = pty.pair.master.take_writer()?;

    let view_state = Arc::new(Mutex::new(ViewState::Claude));
    let mut feed = FeedView::default();

    let view_state_reader = view_state.clone();
    std::thread::spawn(move || {
        let mut buf = [0u8; 8192];
        let mut out = std::io::stdout();
        loop {
            match master_reader.read(&mut buf) {
                Ok(0) | Err(_) => return,
                Ok(n) => {
                    if *view_state_reader.lock().unwrap() == ViewState::Claude {
                        let _ = out.write_all(&buf[..n]);
                        let _ = out.flush();
                    }
                }
            }
        }
    });

    loop {
        if child_done.try_recv().is_ok() {
            break;
        }

        let current = *view_state.lock().unwrap();
        if current == ViewState::Feed {
            // Snapshot metrics out of the mutex BEFORE taking the buffers
            // lock so we never hold both across the ratatui draw.
            let metrics_snapshot = *metrics.lock().unwrap();
            render(
                &mut term,
                &mut feed,
                &buffers.lock().unwrap(),
                &metrics_snapshot,
            )?;
        }

        if event::poll(Duration::from_millis(33))? {
            match event::read()? {
                Event::Mouse(me) if current == ViewState::Feed => match me.kind {
                    MouseEventKind::ScrollUp => feed.scroll_up(3),
                    MouseEventKind::ScrollDown => feed.scroll_down(3),
                    _ => {}
                },
                Event::Paste(text) if current == ViewState::Claude => {
                    // Forward pasted text to claude wrapped in bracketed-paste markers,
                    // so claude's own input handler treats it as a single paste rather
                    // than character-by-character keystrokes (which can trigger autocomplete
                    // or multi-line input quirks).
                    master_writer.write_all(b"\x1b[200~")?;
                    master_writer.write_all(text.as_bytes())?;
                    master_writer.write_all(b"\x1b[201~")?;
                    master_writer.flush()?;
                }
                Event::Paste(_) => {}
                Event::Key(key) => match decide(key, current) {
                    InputDecision::ToggleView => {
                        let new_state = current.toggle();
                        *view_state.lock().unwrap() = new_state;
                        if new_state == ViewState::Claude {
                            // Leaving the feed: restore the main buffer (claude's
                            // TUI from before we entered alt-screen) and force a
                            // resize-based repaint in case claude wrote anything
                            // in between (the reader thread discards bytes in
                            // feed view, so the main buffer is stale).
                            // Disable mouse capture so the user gets their
                            // terminal's native scrollback / text selection
                            // back in the Claude view.
                            execute!(std::io::stdout(), DisableMouseCapture, LeaveAlternateScreen,)?;
                            let (cur_cols, cur_rows) =
                                crossterm::terminal::size().unwrap_or((cols, rows));
                            let shrunk = cur_rows.saturating_sub(1).max(1);
                            let _ = pty.pair.master.resize(PtySize {
                                rows: shrunk,
                                cols: cur_cols,
                                pixel_width: 0,
                                pixel_height: 0,
                            });
                            tokio::time::sleep(Duration::from_millis(50)).await;
                            let _ = pty.pair.master.resize(PtySize {
                                rows: cur_rows,
                                cols: cur_cols,
                                pixel_width: 0,
                                pixel_height: 0,
                            });
                        } else {
                            // Entering the feed: switch to alt-screen so claude's
                            // TUI is stashed by the terminal, enable mouse capture
                            // so the wheel can scroll the feed (the terminal's
                            // native scrollback doesn't apply in alt-screen), then
                            // invalidate ratatui's diff buffer so the first feed
                            // frame paints every cell.
                            execute!(std::io::stdout(), EnterAlternateScreen, EnableMouseCapture,)?;
                            term.clear()?;
                        }
                    }
                    InputDecision::Forward(bytes) => {
                        if current == ViewState::Claude && !bytes.is_empty() {
                            master_writer.write_all(&bytes)?;
                            master_writer.flush()?;
                        }
                    }
                    InputDecision::NavigateFeed(nav) => match nav {
                        FeedNav::AgentPrev => feed.cycle_agent(&buffers.lock().unwrap(), -1),
                        FeedNav::AgentNext => feed.cycle_agent(&buffers.lock().unwrap(), 1),
                        FeedNav::ScrollUp => feed.scroll_up(1),
                        FeedNav::ScrollDown => feed.scroll_down(1),
                        FeedNav::PageUp => feed.scroll_up(10),
                        FeedNav::PageDown => feed.scroll_down(10),
                        FeedNav::JumpTop => feed.scroll_to_top(),
                        FeedNav::JumpBottom => feed.scroll_to_bottom(),
                        FeedNav::DeleteAgent => {
                            let to_remove = feed.selected_agent.clone();
                            let removed = {
                                let mut b = buffers.lock().unwrap();
                                b.remove_agent(&to_remove)
                            };
                            if removed {
                                // Snap selection back to main and reset scroll.
                                feed.selected_agent = "main".into();
                                feed.agent_index = 0;
                                feed.scroll_offset = 0;
                            }
                        }
                        FeedNav::Quit => break,
                    },
                },
                _ => {}
            }
        }
    }

    Ok(())
}

fn runtime_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("XDG_RUNTIME_DIR") {
        return PathBuf::from(d);
    }
    PathBuf::from("/tmp")
}

struct SocketDirCleanup(PathBuf);
impl Drop for SocketDirCleanup {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct TerminalGuard;
impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = execute!(
            std::io::stdout(),
            DisableMouseCapture,
            DisableBracketedPaste,
            LeaveAlternateScreen,
        );
        let _ = disable_raw_mode();
    }
}

fn dummy_child() -> Box<dyn portable_pty::Child + Send + Sync> {
    #[derive(Debug)]
    struct Dummy;
    impl portable_pty::ChildKiller for Dummy {
        fn kill(&mut self) -> std::io::Result<()> {
            Ok(())
        }
        fn clone_killer(&self) -> Box<dyn portable_pty::ChildKiller + Send + Sync> {
            Box::new(Dummy)
        }
    }
    impl portable_pty::Child for Dummy {
        fn process_id(&self) -> Option<u32> {
            None
        }
        fn wait(&mut self) -> std::io::Result<portable_pty::ExitStatus> {
            Ok(portable_pty::ExitStatus::with_exit_code(0))
        }
        fn try_wait(&mut self) -> std::io::Result<Option<portable_pty::ExitStatus>> {
            Ok(Some(portable_pty::ExitStatus::with_exit_code(0)))
        }
    }
    Box::new(Dummy)
}
