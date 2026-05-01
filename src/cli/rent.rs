use super::{build_context, next_verb, parse_value, GlobalOpts};
use crate::application::{config_tx, rent};
use crate::error::{MsigError, OutputMode};

pub fn run(globals: GlobalOpts, mut parser: lexopt::Parser) -> Result<(), MsigError> {
    let verb = next_verb(&mut parser, "rent")?;
    match verb.as_str() {
        "reclaim" => cmd_reclaim(globals, &mut parser),
        "set-collector" => cmd_set_collector(globals, &mut parser),
        other => Err(MsigError::Usage(format!("unknown rent command: '{other}'"))),
    }
}

fn cmd_reclaim(globals: GlobalOpts, parser: &mut lexopt::Parser) -> Result<(), MsigError> {
    use lexopt::Arg::*;
    let mut last_n: u64 = 100;

    while let Some(arg) = parser.next().map_err(|e| MsigError::Usage(e.to_string()))? {
        match arg {
            Long("last-n") => {
                let v = parse_value(parser, "--last-n")?;
                last_n = v
                    .parse()
                    .map_err(|_| MsigError::Usage(format!("invalid last-n: '{v}'")))?;
            }
            Short('h') | Long("help") => {
                super::help::print_resource_help("rent");
                return Ok(());
            }
            _ => return Err(MsigError::Usage("unexpected flag".into())),
        }
    }

    let ctx = build_context(&globals)?;
    let multisig = ctx.resolve_multisig(None)?;
    rent::claim_rent(
        ctx.rpc(),
        ctx.signer.as_ref(),
        &multisig,
        last_n,
        globals.dry_run,
        &ctx.config,
        globals.yes,
        &ctx.program_id,
    )?;
    Ok(())
}

fn cmd_set_collector(globals: GlobalOpts, parser: &mut lexopt::Parser) -> Result<(), MsigError> {
    use lexopt::Arg::*;
    let mut addr: Option<String> = None;
    let mut memo: Option<String> = None;

    while let Some(arg) = parser.next().map_err(|e| MsigError::Usage(e.to_string()))? {
        match arg {
            Long("memo") => memo = Some(parse_value(parser, "--memo")?),
            Short('h') | Long("help") => {
                super::help::print_resource_help("rent");
                return Ok(());
            }
            Value(val) if addr.is_none() => {
                addr = Some(
                    val.into_string()
                        .map_err(|_| MsigError::Usage("invalid UTF-8".into()))?,
                );
            }
            _ => return Err(MsigError::Usage("unexpected argument".into())),
        }
    }

    let addr_str =
        addr.ok_or_else(|| MsigError::Usage("rent collector address is required".into()))?;

    let output_mode = crate::output::detect_output_mode(globals.output.as_deref());
    let is_json = matches!(output_mode, OutputMode::Json);

    let ctx = build_context(&globals)?;
    let multisig = ctx.resolve_multisig(None)?;
    let result = config_tx::create_set_rent_collector_proposal(
        ctx.rpc(),
        ctx.signer.as_ref(),
        &multisig,
        Some(&addr_str),
        memo.as_deref(),
        &ctx.config,
        globals.dry_run,
        globals.yes,
        is_json,
    )?;

    if is_json {
        let json = serde_json::json!({
            "action": "rent_set_collector",
            "signature": result,
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&json).unwrap_or_default()
        );
    }
    Ok(())
}
