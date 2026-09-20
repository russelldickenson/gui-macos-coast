mod session;

pub use alacritty_terminal::vte::ansi::CursorShape;
pub use session::{default_shell, SelectionKind, Session, SessionEvent, SpawnOptions};
