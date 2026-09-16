mod session;

pub use alacritty_terminal::vte::ansi::CursorShape;
pub use session::{default_shell, Session, SessionEvent, SpawnOptions};
