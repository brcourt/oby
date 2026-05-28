use crate::key::{decide, FeedNav, InputDecision, ViewState};
use crate::pty::{spawn_claude, watch_child};
use crate::ring::AllAgentBuffers;
use crate::sockets::spawn_listeners;
use crate::tui::{render, FeedView};
use anyhow::Result;
use crossterm::{
    event::{self, Event},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use nix::sys::signal::{kill, Signal};
use nix::unistd::Pid;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use uuid::Uuid;

pub fn run(rest: Vec<String>) -> ExitCode {
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
    match rt.block_on(run_async(rest)) {
        Ok(_) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("oby: {e:#}");
            ExitCode::FAILURE
        }
    }
}

async fn run_async(rest: Vec<String>) -> Result<()> {
    // 1) Set up socket dir + env.
    let session_id = Uuid::new_v4().simple().to_string();
    let socket_dir = runtime_dir().join("obi").join(session_id);
    std::env::set_var("OBS_ACTIVE", "1");
    std::env::set_var("OBS_SOCKET_DIR", &socket_dir);
    let _cleanup = SocketDirCleanup(socket_dir.clone());

    // 2) Start listeners.
    let buffers = Arc::new(Mutex::new(AllAgentBuffers::default()));
    spawn_listeners(socket_dir.clone(), buffers.clone()).await?;

    // 3) Enter raw mode + alt screen.
    let (cols, rows) = crossterm::terminal::size()?;
    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = ratatui::backend::CrosstermBackend::new(stdout);
    let mut term = ratatui::Terminal::new(backend)?;

    // 4) Spawn claude in pty.
    let mut pty = spawn_claude(rest, cols, rows)?;
    let claude_pid = pty.child.process_id();
    let child_done = watch_child(std::mem::replace(&mut pty.child, dummy_child()));

    // 5) Set up pty reader/writer.
    let mut master_reader = pty.pair.master.try_clone_reader()?;
    let mut master_writer = pty.pair.master.take_writer()?;

    // 6) State.
    let mut view_state = ViewState::Claude;
    let mut feed = FeedView::default();

    // Pump pty output to terminal in a background thread.
    let view_state_shared = Arc::new(Mutex::new(view_state));
    let view_state_for_reader = view_state_shared.clone();
    std::thread::spawn(move || {
        let mut buf = [0u8; 8192];
        let mut out = std::io::stdout();
        loop {
            match master_reader.read(&mut buf) {
                Ok(0) | Err(_) => return,
                Ok(n) => {
                    if *view_state_for_reader.lock().unwrap() == ViewState::Claude {
                        let _ = out.write_all(&buf[..n]);
                        let _ = out.flush();
                    }
                    // When feed view is active, we discard claude's bytes (next time we
                    // flip back we'll SIGWINCH and claude repaints).
                }
            }
        }
    });

    // 7) Main input loop.
    loop {
        // If claude exited, leave.
        if child_done.try_recv().is_ok() {
            break;
        }

        // Tick rate: render the feed at ~30Hz when active.
        if view_state == ViewState::Feed {
            render(&mut term, &feed, &buffers.lock().unwrap())?;
        }

        if event::poll(Duration::from_millis(33))? {
            if let Event::Key(key) = event::read()? {
                match decide(key, view_state) {
                    InputDecision::ToggleView => {
                        view_state = view_state.toggle();
                        *view_state_shared.lock().unwrap() = view_state;
                        if view_state == ViewState::Claude {
                            // Force claude to repaint its full screen.
                            if let Some(pid) = claude_pid {
                                let _ = kill(Pid::from_raw(pid as i32), Signal::SIGWINCH);
                            }
                            // Clear the screen so the alt-screen reveals claude cleanly.
                            execute!(
                                std::io::stdout(),
                                crossterm::terminal::Clear(crossterm::terminal::ClearType::All)
                            )?;
                        }
                    }
                    InputDecision::Forward(bytes) => {
                        if view_state == ViewState::Claude && !bytes.is_empty() {
                            master_writer.write_all(&bytes)?;
                            master_writer.flush()?;
                        }
                    }
                    InputDecision::NavigateFeed(nav) => {
                        match nav {
                            FeedNav::AgentPrev => feed.cycle_agent(&buffers.lock().unwrap(), -1),
                            FeedNav::AgentNext => feed.cycle_agent(&buffers.lock().unwrap(), 1),
                            FeedNav::Quit => break,
                            // v0.1 scrolling is unimplemented (ratatui list auto-scrolls to bottom).
                            FeedNav::ScrollUp | FeedNav::ScrollDown => {}
                        }
                    }
                }
            }
        }
    }

    // 8) Restore terminal.
    disable_raw_mode()?;
    execute!(std::io::stdout(), LeaveAlternateScreen)?;
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

// Placeholder child for when we move the real one into watch_child().
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
