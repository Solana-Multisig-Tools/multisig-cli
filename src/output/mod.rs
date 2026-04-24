pub mod json;
pub mod table;

pub use crate::error::OutputMode;

pub fn detect_output_mode(flag: Option<&str>) -> OutputMode {
    match flag {
        Some("json") => OutputMode::Json,
        Some("table") | Some(_) => OutputMode::Text,
        None => {
            if stdout_is_tty() {
                OutputMode::Text
            } else {
                OutputMode::Json
            }
        }
    }
}

pub fn should_use_color() -> bool {
    std::env::var_os("NO_COLOR").is_none()
}

fn stdout_is_tty() -> bool {
    std::io::IsTerminal::is_terminal(&std::io::stdout())
}
