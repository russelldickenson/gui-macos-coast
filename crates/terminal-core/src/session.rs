//! A single shell session: PTY + `alacritty_terminal` emulation state.
//!
//! The PTY reader runs on a background thread and pumps bytes through the
//! terminal parser. The UI thread pulls rendered state out via [`Session`].

use std::io::{Read, Write};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use alacritty_terminal::{
    event::{Event, EventListener},
    grid::{Dimensions, Scroll},
    index::{Column, Line, Point, Side},
    selection::{Selection, SelectionType},
    term::{test::TermSize, Config, TermMode},
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
    /// Directory the shell should start in; `None` falls back to `$HOME`.
    pub cwd: Option<std::path::PathBuf>,
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
            cwd: None,
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
        // Advertise 24-bit color support so apps that gate their palette on
        // this (tig, vim, tmux, ...) use their full truecolor styling
        // instead of falling back to a more conservative one.
        cmd.env("COLORTERM", "truecolor");
        if let Some(cwd) = &opts.cwd {
            cmd.cwd(cwd);
        } else if let Some(home) = std::env::var_os("HOME") {
            // Login shells already cd to $HOME; be explicit anyway.
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

    /// The shell's own pid (the direct child spawned into the PTY).
    fn shell_pid(&self) -> Option<libc::pid_t> {
        self.child
            .lock()
            .ok()
            .and_then(|guard| guard.as_ref().and_then(|child| child.process_id()))
            .map(|pid| pid as libc::pid_t)
    }

    /// The pid of whichever process currently owns the PTY's foreground
    /// process group, as the kernel reports it via `tcgetpgrp` — in the
    /// common case (no job control weirdness) this is the pid of the
    /// group's leader, i.e. the actual foreground command.
    fn foreground_pid(&self) -> Option<libc::pid_t> {
        let fd = self.master.lock().ok().and_then(|m| m.as_raw_fd())?;
        let pgid = unsafe { libc::tcgetpgrp(fd) };
        (pgid > 0).then_some(pgid)
    }

    /// Whether something other than the idle shell prompt currently owns
    /// the terminal's foreground process group — e.g. a build, an editor,
    /// an ssh session. Used to decide whether closing the tab needs
    /// confirmation.
    pub fn has_foreground_process(&self) -> bool {
        match (self.shell_pid(), self.foreground_pid()) {
            (Some(shell), Some(fg)) => fg != shell,
            _ => false,
        }
    }

    /// The executable name of the foreground process when it isn't the
    /// idle shell (e.g. "vim", "cargo"); `None` at the prompt or if the name
    /// can't be read.
    pub fn foreground_process_name(&self) -> Option<String> {
        if !self.has_foreground_process() {
            return None;
        }
        proc_name(self.foreground_pid()?)
    }

    /// The current working directory of whichever process is in the
    /// foreground (the running command if there is one, otherwise the
    /// shell itself) — drives the directory breadcrumb. `None` if it can't
    /// be determined (process just exited, sandboxing, etc).
    pub fn current_dir(&self) -> Option<std::path::PathBuf> {
        let pid = self.foreground_pid().or_else(|| self.shell_pid())?;
        proc_cwd(pid)
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

    /// Scroll the viewport by `delta` lines (positive scrolls back into
    /// history, negative scrolls forward toward the live output).
    pub fn scroll_lines(&self, delta: i32) {
        if let Ok(mut term) = self.term.lock() {
            term.scroll_display(Scroll::Delta(delta));
        }
    }

    /// Jump straight back to the live output (bottom of scrollback).
    pub fn scroll_to_bottom(&self) {
        if let Ok(mut term) = self.term.lock() {
            term.scroll_display(Scroll::Bottom);
        }
    }

    /// Whether the app has switched into application cursor key mode
    /// (DECCKM, `CSI ?1h`) — full-screen apps like vim/tig set this so
    /// arrow keys arrive as `ESC O A`..`D` instead of the default `ESC [
    /// A`..`D`. Keyboard input needs to match whichever mode is active, or
    /// the app's input parser can fail to recognize the arrow sequence.
    pub fn application_cursor_keys(&self) -> bool {
        self.with_term(|term| term.mode().contains(TermMode::APP_CURSOR))
    }

    /// Send pasted text to the child, wrapped in bracketed-paste markers
    /// when the app asked for them (so shells/editors treat it as literal
    /// input rather than typed keystrokes).
    pub fn paste(&self, text: &str) -> anyhow::Result<()> {
        let bracketed = self.with_term(|term| term.mode().contains(TermMode::BRACKETED_PASTE));
        if bracketed {
            // Strip ESC so pasted text can't end the bracket early.
            let text = text.replace('\x1b', "");
            self.write(format!("\x1b[200~{text}\x1b[201~").as_bytes())
        } else {
            // Terminals expect CR for line breaks in unbracketed pastes.
            self.write(text.replace("\r\n", "\r").replace('\n', "\r").as_bytes())
        }
    }

    /// Begin a selection at viewport cell (`col`, `row`). `right_half` is
    /// whether the pointer was in the cell's right half, so a drag includes
    /// or excludes the boundary cell like other terminals do.
    pub fn start_selection(&self, kind: SelectionKind, col: usize, row: usize, right_half: bool) {
        if let Ok(mut term) = self.term.lock() {
            let point = viewport_point(&term, col, row);
            let ty = match kind {
                SelectionKind::Simple => SelectionType::Simple,
                SelectionKind::Word => SelectionType::Semantic,
                SelectionKind::Line => SelectionType::Lines,
            };
            term.selection = Some(Selection::new(ty, point, side(right_half)));
        }
    }

    /// Extend the active selection to viewport cell (`col`, `row`).
    pub fn update_selection(&self, col: usize, row: usize, right_half: bool) {
        if let Ok(mut term) = self.term.lock() {
            let point = viewport_point(&term, col, row);
            if let Some(selection) = term.selection.as_mut() {
                selection.update(point, side(right_half));
            }
        }
    }

    pub fn clear_selection(&self) {
        if let Ok(mut term) = self.term.lock() {
            if term.selection.is_some() {
                term.selection = None;
            }
        }
    }

    /// The selected text, if any (`None` when nothing, or only an empty
    /// range, is selected).
    pub fn selection_text(&self) -> Option<String> {
        self.with_term(|term| term.selection_to_string())
            .filter(|text| !text.is_empty())
    }

    /// How far back the viewport is scrolled, and how many lines of
    /// scrollback history are available — for driving a scrollbar.
    pub fn scroll_position(&self) -> (usize, usize) {
        self.with_term(|term| {
            let grid = term.grid();
            (grid.display_offset(), grid.history_size())
        })
    }
}

/// How a selection grows as it's dragged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionKind {
    /// Character by character.
    Simple,
    /// Whole words (double-click).
    Word,
    /// Whole lines (triple-click).
    Line,
}

fn side(right_half: bool) -> Side {
    if right_half {
        Side::Right
    } else {
        Side::Left
    }
}

/// Map a viewport cell to a grid point, accounting for scrollback offset.
fn viewport_point<T>(term: &Term<T>, col: usize, row: usize) -> Point {
    let grid = term.grid();
    let col = col.min(grid.columns().saturating_sub(1));
    let row = row.min(grid.screen_lines().saturating_sub(1));
    Point::new(
        Line(row as i32 - grid.display_offset() as i32),
        Column(col),
    )
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

/// The current working directory of a process, via macOS's
/// `proc_pidinfo(PROC_PIDVNODEPATHINFO)` — the same mechanism Activity
/// Monitor / `lsof` use, no `/proc` filesystem needed.
fn proc_cwd(pid: libc::pid_t) -> Option<std::path::PathBuf> {
    use std::mem::MaybeUninit;

    let mut info = MaybeUninit::<libc::proc_vnodepathinfo>::uninit();
    let size = std::mem::size_of::<libc::proc_vnodepathinfo>() as libc::c_int;
    let written = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDVNODEPATHINFO,
            0,
            info.as_mut_ptr() as *mut libc::c_void,
            size,
        )
    };
    if written != size {
        return None;
    }
    let info = unsafe { info.assume_init() };

    // `vip_path` is laid out as a contiguous MAXPATHLEN (1024) byte,
    // NUL-terminated C string, just split into `[[c_char; 32]; 32]` to
    // dodge a const-generic limitation in `libc`; read it back as flat bytes.
    let path_bytes: &[u8] = unsafe {
        std::slice::from_raw_parts(info.pvi_cdir.vip_path.as_ptr() as *const u8, 1024)
    };
    let end = path_bytes.iter().position(|&b| b == 0).unwrap_or(path_bytes.len());
    if end == 0 {
        return None;
    }
    Some(std::path::PathBuf::from(std::str::from_utf8(&path_bytes[..end]).ok()?))
}

/// A process's short executable name via `proc_name`.
fn proc_name(pid: libc::pid_t) -> Option<String> {
    let mut buf = [0u8; 256];
    let n = unsafe { libc::proc_name(pid, buf.as_mut_ptr() as *mut libc::c_void, buf.len() as u32) };
    if n <= 0 {
        return None;
    }
    let name = std::str::from_utf8(&buf[..n as usize]).ok()?.trim();
    (!name.is_empty()).then(|| name.to_string())
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
            cwd: None,
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
            cwd: None,
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

    #[test]
    fn current_dir_tracks_cd() {
        let session = Session::spawn(SpawnOptions {
            shell: "/bin/sh".to_string(),
            args: vec![],
            cols: 80,
            rows: 24,
            scrollback: 1000,
            cursor_shape: CursorShape::Block,
            cwd: None,
        })
        .expect("spawn session");

        let tmp_raw = std::env::temp_dir().join(format!("coast-cwd-test-{}", std::process::id()));
        std::fs::create_dir_all(&tmp_raw).unwrap();
        let tmp = tmp_raw.canonicalize().unwrap();

        session
            .write(format!("cd '{}'\necho cd-done\n", tmp.display()).as_bytes())
            .expect("write to pty");
        wait_for_text(&session, "cd-done", Duration::from_secs(10));

        // The shell needs a beat after echoing to actually finish chdir
        // and settle back at the prompt as the pty's foreground process.
        let mut seen = None;
        for _ in 0..50 {
            if let Some(dir) = session.current_dir() {
                if dir == tmp {
                    seen = Some(dir);
                    break;
                }
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        assert_eq!(seen, Some(tmp.clone()), "current_dir never reported {tmp:?}");

        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn spawn_honors_explicit_cwd() {
        let dir = std::env::temp_dir().join(format!("coast-cwd-probe-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let dir = dir.canonicalize().unwrap();

        let session = Session::spawn(SpawnOptions {
            shell: "/bin/sh".to_string(),
            args: vec![],
            cols: 80,
            rows: 24,
            scrollback: 1000,
            cursor_shape: CursorShape::Block,
            cwd: Some(dir.clone()),
        })
        .expect("spawn session");

        session.write(b"pwd\n").expect("write to pty");
        let text = wait_for_text(&session, dir.to_str().unwrap(), Duration::from_secs(10));
        assert!(
            text.contains(dir.to_str().unwrap()),
            "expected pwd to report {dir:?}, got:\n{text}"
        );

        std::fs::remove_dir_all(&dir).ok();
    }
}
