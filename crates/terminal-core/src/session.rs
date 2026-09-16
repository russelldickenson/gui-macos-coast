//! A single shell session: PTY + `alacritty_terminal` emulation state.
//!
//! The PTY reader runs on a background thread and pumps bytes through the
//! terminal parser. The UI thread pulls rendered state out via [`Session`].

use std::io::{Read, Write};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use alacritty_terminal::{
    event::{Event, EventListener},
    term::{test::TermSize, Config},
    vte::ansi::{CursorShape, CursorStyle, Processor},
    Term,
};
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};

/// Events produced by a [`Session`] for the UI to consume.
#[derive(Debug, Clone)]
pub enum SessionEvent {
    /// New output was parsed into the grid; repaint.
    Wakeup,
    /// Child requested a window/tab title change.
    Title(String),
    /// Reset title to the default.
    ResetTitle,
    /// Terminal bell.
    Bell,
    /// OSC-52 clipboard store request.
    ClipboardStore(String),
    /// Child process exited with the given status code (`None` = signaled).
    Exited(Option<i32>),
}

/// Forwards alacritty events to the session channel, writing PTY responses
/// (e.g. device-attribute replies) straight back to the child.
#[derive(Clone)]
pub struct SessionEventProxy {
    tx: mpsc::Sender<SessionEvent>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
}

impl EventListener for SessionEventProxy {
    fn send_event(&self, event: Event) {
        let mapped = match event {
            Event::Wakeup => Some(SessionEvent::Wakeup),
            Event::Title(title) => Some(SessionEvent::Title(title)),
            Event::ResetTitle => Some(SessionEvent::ResetTitle),
            Event::Bell => Some(SessionEvent::Bell),
            Event::ClipboardStore(_, text) => Some(SessionEvent::ClipboardStore(text)),
            Event::PtyWrite(text) => {
                if let Ok(mut writer) = self.writer.lock() {
                    let _ = writer.write_all(text.as_bytes());
                    let _ = writer.flush();
                }
                None
            }
            _ => None,
        };
        if let Some(mapped) = mapped {
            let _ = self.tx.send(mapped);
        }
    }
}

/// Options for spawning a session.
pub struct SpawnOptions {
    /// Program to run (e.g. the user's shell).
    pub shell: String,
    /// Arguments for the program.
    pub args: Vec<String>,
    /// Initial size in columns / rows.
    pub cols: u16,
    pub rows: u16,
    /// Scrollback history length in lines.
    pub scrollback: usize,
    /// The cursor shape shown until an app requests a different one (e.g.
    /// via DECSCUSR).
    pub cursor_shape: CursorShape,
}

impl SpawnOptions {
    /// Spawn the user's default login shell.
    pub fn login_shell(cols: u16, rows: u16) -> Self {
        Self {
            shell: default_shell(),
            args: vec!["-l".to_string()],
            cols,
            rows,
            scrollback: 10_000,
            cursor_shape: CursorShape::Block,
        }
    }
}

/// The user's default shell from `$SHELL`, falling back to `/bin/zsh`.
pub fn default_shell() -> String {
    std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string())
}

/// A running shell session.
///
/// `Session` is `Send + Sync` and freely shareable (`Arc<Session>`): the
/// emulation state, PTY handles, and event queue all live behind mutexes.
pub struct Session {
    term: Arc<Mutex<Term<SessionEventProxy>>>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    master: Mutex<Box<dyn MasterPty + Send>>,
    child: Arc<Mutex<Option<Box<dyn Child + Send + Sync>>>>,
    events: Mutex<mpsc::Receiver<SessionEvent>>,
    dims: Mutex<(u16, u16)>,
}

impl Session {
    pub fn spawn(opts: SpawnOptions) -> anyhow::Result<Self> {
        let pty_system = native_pty_system();
        let size = PtySize {
            rows: opts.rows,
            cols: opts.cols,
            pixel_width: 0,
            pixel_height: 0,
        };
        let pair = pty_system.openpty(size)?;

        let mut cmd = CommandBuilder::new(&opts.shell);
        for arg in &opts.args {
            cmd.arg(arg);
        }
        cmd.env("TERM", "xterm-256color");
        // Login shells already cd to $HOME; be explicit anyway.
        if let Some(home) = std::env::var_os("HOME") {
            cmd.cwd(home);
        }

        let child = pair.slave.spawn_command(cmd)?;
        drop(pair.slave);

        let (tx, events) = mpsc::channel();
        let writer: Arc<Mutex<Box<dyn Write + Send>>> =
            Arc::new(Mutex::new(pair.master.take_writer()?));
        let proxy = SessionEventProxy {
            tx: tx.clone(),
            writer: writer.clone(),
        };

        let config = Config {
            scrolling_history: opts.scrollback,
            default_cursor_style: CursorStyle {
                shape: opts.cursor_shape,
                blinking: false,
            },
            ..Config::default()
        };
        let term = Arc::new(Mutex::new(Term::new(
            config,
            &TermSize::new(opts.cols as usize, opts.rows as usize),
            proxy,
        )));

        // PTY -> parser pump.
        let pump_term = term.clone();
        let pump_tx = tx.clone();
        let mut reader = pair.master.try_clone_reader()?;
        std::thread::Builder::new()
            .name("coast-pty-reader".to_string())
            .spawn(move || {
                let mut processor: Processor = Processor::new();
                let mut buf = [0u8; 8 * 1024];
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) => break, // EOF: child exited.
                        Ok(n) => {
                            if let Ok(mut term) = pump_term.lock() {
                                processor.advance(&mut *term, &buf[..n]);
                            }
                            let _ = pump_tx.send(SessionEvent::Wakeup);
                        }
                        Err(_) => break, // EIO on slave close, etc.
                    }
                }
            })?;

        // Child exit watcher.
        let child: Arc<Mutex<Option<Box<dyn Child + Send + Sync>>>> =
            Arc::new(Mutex::new(Some(child)));
        let wait_child = child.clone();
        let wait_tx = tx.clone();
        std::thread::Builder::new()
            .name("coast-child-wait".to_string())
            .spawn(move || {
                let status = wait_child
                    .lock()
                    .ok()
                    .and_then(|mut guard| guard.take())
                    .and_then(|mut child| child.wait().ok());
                let code = status.map(|s| s.exit_code() as i32);
                let _ = wait_tx.send(SessionEvent::Exited(code));
            })?;

        Ok(Self {
            term,
            writer,
            master: Mutex::new(pair.master),
            child,
            events: Mutex::new(events),
            dims: Mutex::new((opts.cols, opts.rows)),
        })
    }

    /// Non-blocking event poll for the UI thread.
    pub fn try_recv_event(&self) -> Option<SessionEvent> {
        self.events
            .lock()
            .expect("session events poisoned")
            .try_recv()
            .ok()
    }

    /// Blocking event receive with timeout for the UI thread.
    pub fn recv_event_timeout(&self, timeout: Duration) -> Option<SessionEvent> {
        self.events
            .lock()
            .expect("session events poisoned")
            .recv_timeout(timeout)
            .ok()
    }

    /// Write user input to the child.
    pub fn write(&self, data: &[u8]) -> anyhow::Result<()> {
        let mut writer = self.writer.lock().expect("pty writer poisoned");
        writer.write_all(data)?;
        writer.flush()?;
        Ok(())
    }

    /// Resize both the emulation grid and the PTY.
    pub fn resize(&self, cols: u16, rows: u16) -> anyhow::Result<()> {
        let mut dims = self.dims.lock().expect("session dims poisoned");
        if cols == dims.0 && rows == dims.1 {
            return Ok(());
        }
        if let Ok(mut term) = self.term.lock() {
            term.resize(TermSize::new(cols as usize, rows as usize));
        }
        self.master
            .lock()
            .expect("pty master poisoned")
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })?;
        *dims = (cols, rows);
        Ok(())
    }

    /// Run a closure against the emulation state (UI render path).
    pub fn with_term<R>(&self, f: impl FnOnce(&Term<SessionEventProxy>) -> R) -> R {
        let term = self.term.lock().expect("terminal state poisoned");
        f(&term)
    }

    /// Visible screen contents as plain text (useful for tests).
    pub fn screen_text(&self) -> String {
        self.with_term(|term| {
            let mut out = String::new();
            let mut last_line = None;
            for indexed in term.grid().display_iter() {
                let line = indexed.point.line.0;
                if last_line != Some(line) {
                    if last_line.is_some() {
                        out.push('\n');
                    }
                    last_line = Some(line);
                }
                out.push(indexed.cell.c);
            }
            out.lines()
                .map(|l| l.trim_end().to_string())
                .collect::<Vec<_>>()
                .join("\n")
        })
    }

    pub fn cols(&self) -> u16 {
        self.dims.lock().expect("session dims poisoned").0
    }

    pub fn rows(&self) -> u16 {
        self.dims.lock().expect("session dims poisoned").1
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        if let Ok(mut guard) = self.child.lock() {
            if let Some(mut child) = guard.take() {
                let _ = child.kill();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn wait_for_text(session: &Session, needle: &str, timeout: Duration) -> String {
        let start = Instant::now();
        loop {
            // Drain events so the channel doesn't fill up.
            while session.try_recv_event().is_some() {}
            let text = session.screen_text();
            if text.contains(needle) || start.elapsed() > timeout {
                return text;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn spawns_shell_and_captures_output() {
        let session = Session::spawn(SpawnOptions {
            shell: "/bin/sh".to_string(),
            args: vec![
                "-c".to_string(),
                "echo hello-coast-marker".to_string(),
            ],
            cols: 80,
            rows: 24,
            scrollback: 1000,
            cursor_shape: CursorShape::Block,
        })
        .expect("spawn session");

        let text = wait_for_text(&session, "hello-coast-marker", Duration::from_secs(10));
        assert!(
            text.contains("hello-coast-marker"),
            "expected marker in screen text, got:\n{text}"
        );
    }

    #[test]
    fn resize_and_write_roundtrip() {
        let session = Session::spawn(SpawnOptions {
            shell: "/bin/sh".to_string(),
            args: vec![],
            cols: 80,
            rows: 24,
            scrollback: 1000,
            cursor_shape: CursorShape::Block,
        })
        .expect("spawn session");

        session.resize(100, 30).expect("resize");
        assert_eq!(session.cols(), 100);
        assert_eq!(session.rows(), 30);

        session
            .write(b"echo roundtrip-ok-123\n")
            .expect("write to pty");
        let text = wait_for_text(&session, "roundtrip-ok-123", Duration::from_secs(10));
        assert!(
            text.contains("roundtrip-ok-123"),
            "expected echo output, got:\n{text}"
        );
    }
}
