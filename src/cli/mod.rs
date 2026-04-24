pub mod config;
pub mod help;
pub mod member;
pub mod multisig;
pub mod program;
pub mod proposal;
pub mod rent;
pub mod template;
pub mod transfer;
pub mod tx;
pub mod vault;

use crate::application::context::CommandContext;
use crate::error::MsigError;
use crate::infra::config::GlobalFlags;

/// Global CLI options parsed from argv.
#[derive(Debug, Default)]
pub struct GlobalOpts {
    pub cluster: Option<String>,
    pub keypair: Option<String>,
    pub ledger: Option<String>,
    pub multisig: Option<String>,
    pub vault_index: Option<u8>,
    pub output: Option<String>,
    pub yes: bool,
    pub priority_fee: Option<u64>,
    pub commitment: Option<String>,
    pub no_color: bool,
    pub dry_run: bool,
    pub program_id: Option<String>,
}

impl GlobalOpts {
    pub(crate) fn to_global_flags(&self) -> GlobalFlags {
        GlobalFlags {
            cluster: self.cluster.clone(),
            keypair: self.keypair.clone(),
            multisig: self.multisig.clone(),
            commitment: self.commitment.clone(),
            vault_index: self.vault_index,
            priority_fee: self.priority_fee,
            program_id: self.program_id.clone(),
        }
    }
}

/// Parse a flag value from the parser.
pub fn parse_value(parser: &mut lexopt::Parser, flag_name: &str) -> Result<String, MsigError> {
    parser
        .value()
        .map_err(|_| MsigError::Usage(format!("{flag_name} requires a value")))?
        .into_string()
        .map_err(|_| MsigError::Usage(format!("{flag_name} value is not valid UTF-8")))
}

/// Get the next positional argument (the verb) or handle --help.
pub fn next_verb(parser: &mut lexopt::Parser, resource: &str) -> Result<String, MsigError> {
    use lexopt::Arg::*;

    match parser.next() {
        Ok(Some(arg)) => match arg {
            Value(val) => val
                .into_string()
                .map_err(|_| MsigError::Usage("invalid UTF-8 in verb".into())),
            Short('h') | Long("help") => {
                help::print_resource_help(resource);
                Err(MsigError::HelpDisplayed)
            }
            _ => Err(MsigError::Usage(format!(
                "expected a verb after '{resource}'. Run 'msig {resource} --help'."
            ))),
        },
        Ok(None) => {
            help::print_resource_help(resource);
            Err(MsigError::HelpDisplayed)
        }
        Err(e) => Err(MsigError::Usage(e.to_string())),
    }
}

fn take_required_arg(args: &[String], idx: &mut usize, flag: &str) -> Result<String, MsigError> {
    *idx += 1;
    args.get(*idx)
        .cloned()
        .ok_or_else(|| MsigError::Usage(format!("{flag} requires a value")))
}

fn split_long_flag(arg: &str) -> (&str, Option<&str>) {
    match arg.split_once('=') {
        Some((name, value)) => (name, Some(value)),
        None => (arg, None),
    }
}

fn parse_u8_flag(value: String, flag: &str) -> Result<u8, MsigError> {
    value
        .parse()
        .map_err(|_| MsigError::Usage(format!("invalid {flag}: '{value}'")))
}

fn parse_u64_flag(value: String, flag: &str) -> Result<u64, MsigError> {
    value
        .parse()
        .map_err(|_| MsigError::Usage(format!("invalid {flag}: '{value}'")))
}

fn inline_or_next(
    inline_value: Option<&str>,
    args: &[String],
    idx: &mut usize,
    flag: &str,
) -> Result<String, MsigError> {
    match inline_value {
        Some(value) => Ok(value.to_string()),
        None => take_required_arg(args, idx, flag),
    }
}

fn validate_global_opts(globals: &GlobalOpts) -> Result<(), MsigError> {
    if let Some(output) = globals.output.as_deref() {
        match output {
            "json" | "table" => {}
            other => {
                return Err(MsigError::Usage(format!(
                    "invalid --output '{other}'. Expected json or table."
                )));
            }
        }
    }

    if let Some(commitment) = globals.commitment.as_deref() {
        match commitment {
            "processed" | "confirmed" | "finalized" => {}
            other => {
                return Err(MsigError::Usage(format!(
                    "invalid --commitment '{other}'. Expected processed, confirmed, or finalized."
                )));
            }
        }
    }

    Ok(())
}

#[cfg(feature = "tui")]
fn run_tui_command(globals: GlobalOpts) -> Result<(), MsigError> {
    let enabled = std::env::var("MSIG_ENABLE_EXPERIMENTAL_TUI")
        .map(|value| matches!(value.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"))
        .unwrap_or(false);
    if enabled {
        crate::tui::launch_tui(globals)
    } else {
        Err(MsigError::Usage(
            "the TUI is experimental and disabled in this release; use the CLI commands for now"
                .into(),
        ))
    }
}

#[cfg(not(feature = "tui"))]
fn run_tui_command(_globals: GlobalOpts) -> Result<(), MsigError> {
    Err(MsigError::Usage(
        "the TUI is not included in this build; use the CLI commands for now".into(),
    ))
}

/// Lift known global flags from anywhere in argv so commands stay scriptable
/// without paying for a larger parser dependency.
fn preparse_global_opts(args: Vec<String>) -> Result<(GlobalOpts, Vec<String>), MsigError> {
    let mut globals = GlobalOpts::default();
    let mut remaining = Vec::with_capacity(args.len());
    let mut idx = 0;

    while idx < args.len() {
        let arg = &args[idx];

        if arg == "--" {
            remaining.extend(args[idx..].iter().cloned());
            break;
        }

        if arg == "-y" {
            globals.yes = true;
            idx += 1;
            continue;
        }

        if let Some(long) = arg.strip_prefix("--") {
            let (name, inline_value) = split_long_flag(long);
            match name {
                "cluster" => {
                    globals.cluster =
                        Some(inline_or_next(inline_value, &args, &mut idx, "--cluster")?);
                }
                "keypair" => {
                    globals.keypair =
                        Some(inline_or_next(inline_value, &args, &mut idx, "--keypair")?);
                }
                "multisig" => {
                    globals.multisig =
                        Some(inline_or_next(inline_value, &args, &mut idx, "--multisig")?);
                }
                "vault-index" => {
                    let value = inline_or_next(inline_value, &args, &mut idx, "--vault-index")?;
                    globals.vault_index = Some(parse_u8_flag(value, "vault-index")?);
                }
                "output" => {
                    globals.output =
                        Some(inline_or_next(inline_value, &args, &mut idx, "--output")?);
                }
                "priority-fee" => {
                    let value = inline_or_next(inline_value, &args, &mut idx, "--priority-fee")?;
                    globals.priority_fee = Some(parse_u64_flag(value, "priority-fee")?);
                }
                "commitment" => {
                    globals.commitment = Some(inline_or_next(
                        inline_value,
                        &args,
                        &mut idx,
                        "--commitment",
                    )?);
                }
                "program-id" => {
                    globals.program_id = Some(inline_or_next(
                        inline_value,
                        &args,
                        &mut idx,
                        "--program-id",
                    )?);
                }
                "ledger" => {
                    if let Some(value) = inline_value {
                        globals.ledger = Some(value.to_string());
                    } else {
                        let next = args.get(idx + 1);
                        let consumes_value = next
                            .map(|value| {
                                value.parse::<u32>().is_ok() || value.starts_with("usb://ledger")
                            })
                            .unwrap_or(false);
                        if consumes_value {
                            idx += 1;
                            globals.ledger = Some(args[idx].clone());
                        } else {
                            globals.ledger = Some(String::new());
                        }
                    }
                }
                "yes" => globals.yes = true,
                "no-color" => globals.no_color = true,
                "dry-run" => globals.dry_run = true,
                _ => remaining.push(arg.clone()),
            }
            idx += 1;
            continue;
        }

        remaining.push(arg.clone());
        idx += 1;
    }

    Ok((globals, remaining))
}

/// Build a CommandContext for write operations that need a signer.
fn build_context(globals: &GlobalOpts) -> Result<CommandContext, MsigError> {
    let cfg = crate::infra::config::load_config(&globals.to_global_flags())?;
    CommandContext::build(
        cfg,
        globals.ledger.as_deref(),
        globals.keypair.as_deref(),
        globals.program_id.as_deref(),
    )
}

/// Load config without a signer (for read-only operations).
fn load_config_only(globals: &GlobalOpts) -> Result<crate::infra::config::Config, MsigError> {
    crate::infra::config::load_config(&globals.to_global_flags())
}

/// Main entry point. Parses global flags, dispatches to resource handlers.
pub fn run() -> Result<(), MsigError> {
    use lexopt::Arg::*;
    let raw_args: Vec<String> = std::env::args().skip(1).collect();
    let (mut globals, args) = preparse_global_opts(raw_args)?;
    let mut parser = lexopt::Parser::from_args(args);

    // Phase 1: Parse global flags + first positional (resource).
    let resource = loop {
        match parser.next() {
            Ok(Some(Long("cluster"))) => {
                globals.cluster = Some(parse_value(&mut parser, "--cluster")?)
            }
            Ok(Some(Long("keypair"))) => {
                globals.keypair = Some(parse_value(&mut parser, "--keypair")?)
            }
            Ok(Some(Long("ledger"))) => {
                let val = parser
                    .value()
                    .map(|v| v.into_string().unwrap_or_default())
                    .unwrap_or_default();
                globals.ledger = Some(val);
            }
            Ok(Some(Long("multisig"))) => {
                globals.multisig = Some(parse_value(&mut parser, "--multisig")?)
            }
            Ok(Some(Long("vault-index"))) => {
                let v = parse_value(&mut parser, "--vault-index")?;
                globals.vault_index = Some(
                    v.parse()
                        .map_err(|_| MsigError::Usage(format!("invalid vault-index: '{v}'")))?,
                );
            }
            Ok(Some(Long("output"))) => {
                globals.output = Some(parse_value(&mut parser, "--output")?)
            }
            Ok(Some(Short('y'))) | Ok(Some(Long("yes"))) => globals.yes = true,
            Ok(Some(Long("priority-fee"))) => {
                let v = parse_value(&mut parser, "--priority-fee")?;
                globals.priority_fee = Some(
                    v.parse()
                        .map_err(|_| MsigError::Usage(format!("invalid priority-fee: '{v}'")))?,
                );
            }
            Ok(Some(Long("commitment"))) => {
                globals.commitment = Some(parse_value(&mut parser, "--commitment")?)
            }
            Ok(Some(Long("no-color"))) => globals.no_color = true,
            Ok(Some(Long("dry-run"))) => globals.dry_run = true,
            Ok(Some(Long("program-id"))) => {
                globals.program_id = Some(parse_value(&mut parser, "--program-id")?)
            }
            Ok(Some(Short('h'))) | Ok(Some(Long("help"))) => {
                help::print_help();
                return Ok(());
            }
            Ok(Some(Long("version"))) => {
                println!("msig {}", help::VERSION);
                return Ok(());
            }
            Ok(Some(Value(val))) => {
                break val
                    .into_string()
                    .map_err(|_| MsigError::Usage("invalid UTF-8".into()))?;
            }
            Ok(Some(Short(c))) => return Err(MsigError::Usage(format!("unknown flag: '-{c}'"))),
            Ok(Some(Long(f))) => return Err(MsigError::Usage(format!("unknown flag: '--{f}'"))),
            Ok(None) => {
                validate_global_opts(&globals)?;
                help::print_help();
                return Ok(());
            }
            Err(e) => return Err(MsigError::Usage(e.to_string())),
        }
    };

    if globals.no_color {
        std::env::set_var("NO_COLOR", "1");
    }
    validate_global_opts(&globals)?;

    // Phase 2: Dispatch to resource handler.
    match resource.as_str() {
        "config" => self::config::run(globals, parser),
        "multisig" => multisig::run(globals, parser),
        "vault" => vault::run(globals, parser),
        "member" => member::run(globals, parser),
        "proposal" => proposal::run(globals, parser),
        "transfer" => transfer::run(globals, parser),
        "template" => template::run(globals, parser),
        "tx" => tx::run(globals, parser),
        "program" => program::run(globals, parser),
        "rent" => rent::run(globals, parser),
        "tui" => run_tui_command(globals),
        _ => Err(MsigError::Usage(format!(
            "unknown command: '{resource}'. Run 'msig --help'."
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> (GlobalOpts, Vec<String>) {
        let args = args.iter().map(|arg| arg.to_string()).collect();
        preparse_global_opts(args).unwrap_or_else(|e| panic!("{e}"))
    }

    #[test]
    fn preparses_global_flags_after_command() {
        let (globals, remaining) =
            parse(&["proposal", "list", "--output", "json", "--cluster=devnet"]);
        assert_eq!(globals.output.as_deref(), Some("json"));
        assert_eq!(globals.cluster.as_deref(), Some("devnet"));
        assert_eq!(remaining, vec!["proposal", "list"]);
    }

    #[test]
    fn preparses_vault_index_after_tx_create() {
        let (globals, remaining) = parse(&[
            "tx",
            "create",
            "--program",
            "11111111111111111111111111111111",
            "--vault-index",
            "3",
        ]);
        assert_eq!(globals.vault_index, Some(3));
        assert_eq!(
            remaining,
            vec![
                "tx",
                "create",
                "--program",
                "11111111111111111111111111111111"
            ]
        );
    }

    #[test]
    fn ledger_without_value_does_not_consume_command() {
        let (globals, remaining) = parse(&["--ledger", "proposal", "--help"]);
        assert_eq!(globals.ledger.as_deref(), Some(""));
        assert_eq!(remaining, vec!["proposal", "--help"]);
    }

    #[test]
    fn ledger_consumes_numeric_value() {
        let (globals, remaining) = parse(&["proposal", "approve", "1", "--ledger", "2"]);
        assert_eq!(globals.ledger.as_deref(), Some("2"));
        assert_eq!(remaining, vec!["proposal", "approve", "1"]);
    }

    #[test]
    fn validates_output_mode() {
        let (globals, _) = parse(&["proposal", "list", "--output", "yaml"]);
        assert!(validate_global_opts(&globals).is_err());
    }

    #[test]
    fn validates_commitment() {
        let (globals, _) = parse(&["proposal", "list", "--commitment", "rooted"]);
        assert!(validate_global_opts(&globals).is_err());
    }

    #[test]
    fn tui_command_is_disabled_without_dev_gate() {
        std::env::remove_var("MSIG_ENABLE_EXPERIMENTAL_TUI");
        match run_tui_command(GlobalOpts::default()) {
            Ok(()) => panic!("TUI command should be disabled without the dev gate"),
            Err(err) => assert!(err.to_string().contains("TUI")),
        }
    }
}
