#![deny(clippy::unwrap_used, clippy::expect_used)]

mod application;
mod cli;
mod domain;
mod error;
mod infra;
mod output;
mod sanitize;
#[cfg(feature = "tui")]
mod tui;

use error::{install_panic_handler, OutputMode};

fn main() {
    let output_mode = detect_output_mode_early();
    install_panic_handler(output_mode);

    let exit_code = match cli::run() {
        Ok(()) => 0,
        Err(e) => {
            let code = e.exit_code();
            if code != 0 {
                match output_mode {
                    OutputMode::Json => {
                        let report = e.to_error_report();
                        if let Ok(json) = serde_json::to_string(&report) {
                            eprintln!("{json}");
                        }
                    }
                    OutputMode::Text => {
                        if let Some(fix) = e.fix_suggestion() {
                            eprintln!("error: {e}\n  hint: {fix}");
                        } else {
                            eprintln!("error: {e}");
                        }
                    }
                }
            }
            code
        }
    };

    std::process::exit(i32::from(exit_code));
}

fn detect_output_mode_early() -> OutputMode {
    let args: Vec<String> = std::env::args().collect();
    for (i, arg) in args.iter().enumerate() {
        if arg == "--output" {
            if let Some(val) = args.get(i + 1) {
                if val == "json" {
                    return OutputMode::Json;
                }
            }
        } else if arg == "--output=json" {
            return OutputMode::Json;
        }
    }
    OutputMode::Text
}
