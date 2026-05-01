use super::{build_context, next_verb, parse_value, GlobalOpts};
use crate::application::transfer;
use crate::error::{MsigError, OutputMode};
use crate::infra::config::tokens;

pub fn run(globals: GlobalOpts, mut parser: lexopt::Parser) -> Result<(), MsigError> {
    let verb = next_verb(&mut parser, "transfer")?;
    match verb.as_str() {
        "sol" => cmd_sol(globals, &mut parser),
        "spl" => cmd_spl(globals, &mut parser),
        other => Err(MsigError::Usage(format!(
            "unknown transfer command: '{other}'"
        ))),
    }
}

fn cmd_sol(globals: GlobalOpts, parser: &mut lexopt::Parser) -> Result<(), MsigError> {
    use lexopt::Arg::*;
    let mut amount_str: Option<String> = None;
    let mut recipient: Option<String> = None;
    let mut memo: Option<String> = None;

    while let Some(arg) = parser.next().map_err(|e| MsigError::Usage(e.to_string()))? {
        match arg {
            Long("memo") => memo = Some(parse_value(parser, "--memo")?),
            Short('h') | Long("help") => {
                super::help::print_resource_help("transfer");
                return Ok(());
            }
            Value(val) => {
                let s = val
                    .into_string()
                    .map_err(|_| MsigError::Usage("invalid UTF-8".into()))?;
                if amount_str.is_none() {
                    amount_str = Some(s);
                } else if recipient.is_none() {
                    recipient = Some(s);
                } else {
                    return Err(MsigError::Usage("too many arguments".into()));
                }
            }
            _ => return Err(MsigError::Usage("unexpected flag".into())),
        }
    }

    let amount_str = amount_str.ok_or_else(|| MsigError::Usage("amount is required".into()))?;
    let recipient = recipient.ok_or_else(|| MsigError::Usage("recipient is required".into()))?;
    let amount = tokens::parse_human_amount(&amount_str, 9)?;

    let output_mode = crate::output::detect_output_mode(globals.output.as_deref());
    let is_json = matches!(output_mode, OutputMode::Json);

    let ctx = build_context(&globals)?;
    let multisig = ctx.resolve_multisig(None)?;
    let vault_index = ctx.vault_index(None);
    let result = if is_json {
        transfer::create_transfer_proposal_quiet(
            ctx.rpc(),
            ctx.signer.as_ref(),
            &multisig,
            amount,
            "native",
            &recipient,
            vault_index,
            memo.as_deref(),
            &ctx.config,
            globals.dry_run,
            globals.yes,
        )?
    } else {
        transfer::create_transfer_proposal(
            ctx.rpc(),
            ctx.signer.as_ref(),
            &multisig,
            amount,
            "native",
            &recipient,
            vault_index,
            memo.as_deref(),
            &ctx.config,
            globals.dry_run,
            globals.yes,
        )?
    };

    if is_json {
        let json = serde_json::json!({
            "action": "transfer_sol",
            "signature": result,
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&json).unwrap_or_default()
        );
    }
    Ok(())
}

fn cmd_spl(globals: GlobalOpts, parser: &mut lexopt::Parser) -> Result<(), MsigError> {
    use lexopt::Arg::*;
    let mut token: Option<String> = None;
    let mut amount_str: Option<String> = None;
    let mut recipient: Option<String> = None;
    let mut memo: Option<String> = None;

    while let Some(arg) = parser.next().map_err(|e| MsigError::Usage(e.to_string()))? {
        match arg {
            Long("memo") => memo = Some(parse_value(parser, "--memo")?),
            Short('h') | Long("help") => {
                super::help::print_resource_help("transfer");
                return Ok(());
            }
            Value(val) => {
                let s = val
                    .into_string()
                    .map_err(|_| MsigError::Usage("invalid UTF-8".into()))?;
                if token.is_none() {
                    token = Some(s);
                } else if amount_str.is_none() {
                    amount_str = Some(s);
                } else if recipient.is_none() {
                    recipient = Some(s);
                } else {
                    return Err(MsigError::Usage("too many arguments".into()));
                }
            }
            _ => return Err(MsigError::Usage("unexpected flag".into())),
        }
    }

    let token = token.ok_or_else(|| MsigError::Usage("token is required".into()))?;
    let amount_str = amount_str.ok_or_else(|| MsigError::Usage("amount is required".into()))?;
    let recipient = recipient.ok_or_else(|| MsigError::Usage("recipient is required".into()))?;

    let ctx = build_context(&globals)?;

    let token_info = tokens::resolve_token(&token, &ctx.config.tokens)?;
    let decimals = if token_info.decimals > 0 {
        token_info.decimals
    } else {
        tokens::fetch_token_decimals(ctx.rpc(), &token_info.mint)?
    };
    let amount = tokens::parse_human_amount(&amount_str, decimals)?;

    let output_mode = crate::output::detect_output_mode(globals.output.as_deref());
    let is_json = matches!(output_mode, OutputMode::Json);

    let multisig = ctx.resolve_multisig(None)?;
    let vault_index = ctx.vault_index(None);
    let result = if is_json {
        transfer::create_transfer_proposal_quiet(
            ctx.rpc(),
            ctx.signer.as_ref(),
            &multisig,
            amount,
            &token_info.mint,
            &recipient,
            vault_index,
            memo.as_deref(),
            &ctx.config,
            globals.dry_run,
            globals.yes,
        )?
    } else {
        transfer::create_transfer_proposal(
            ctx.rpc(),
            ctx.signer.as_ref(),
            &multisig,
            amount,
            &token_info.mint,
            &recipient,
            vault_index,
            memo.as_deref(),
            &ctx.config,
            globals.dry_run,
            globals.yes,
        )?
    };

    if is_json {
        let json = serde_json::json!({
            "action": "transfer_spl",
            "signature": result,
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&json).unwrap_or_default()
        );
    }
    Ok(())
}
