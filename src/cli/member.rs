use super::{build_context, load_config_only, next_verb, parse_value, GlobalOpts};
use crate::application::{config_tx, inspect};
use crate::error::{MsigError, OutputMode};
use crate::output::{json, table};

pub fn run(globals: GlobalOpts, mut parser: lexopt::Parser) -> Result<(), MsigError> {
    let verb = next_verb(&mut parser, "member")?;
    match verb.as_str() {
        "list" => cmd_list(globals, &mut parser),
        "add" => cmd_add(globals, &mut parser),
        "remove" => cmd_remove(globals, &mut parser),
        other => Err(MsigError::Usage(format!(
            "unknown member command: '{other}'"
        ))),
    }
}

fn cmd_list(globals: GlobalOpts, parser: &mut lexopt::Parser) -> Result<(), MsigError> {
    use lexopt::Arg::*;
    let mut multisig_override: Option<String> = None;
    while let Some(arg) = parser.next().map_err(|e| MsigError::Usage(e.to_string()))? {
        match arg {
            Long("multisig") => multisig_override = Some(parse_value(parser, "--multisig")?),
            Short('h') | Long("help") => {
                super::help::print_resource_help("member");
                return Ok(());
            }
            _ => return Err(MsigError::Usage("unexpected flag".into())),
        }
    }

    let cfg = load_config_only(&globals)?;
    let output_mode = crate::output::detect_output_mode(globals.output.as_deref());
    let rpc = crate::infra::rpc::SolanaRpcClient::with_commitment(&cfg.cluster, &cfg.commitment);

    let addr_str = multisig_override
        .as_deref()
        .or(globals.multisig.as_deref())
        .or(cfg.multisig.as_deref())
        .ok_or_else(|| MsigError::Config("no multisig address".into()))?;
    let resolved = crate::infra::config::labels::resolve_address(addr_str, &cfg.labels)?;
    let multisig_pubkey: solana_pubkey::Pubkey = resolved
        .parse()
        .map_err(|_| MsigError::Usage(format!("invalid multisig address: '{resolved}'")))?;

    let program_id = cfg.program_id;
    let vault_index = globals.vault_index.unwrap_or(cfg.vault_index);
    let info = inspect::fetch_multisig_info(&rpc, &multisig_pubkey, vault_index, &program_id)?;

    match output_mode {
        OutputMode::Json => json::print_json(&info.members),
        OutputMode::Text => {
            let headers = &["Member", "Permissions"];
            let rows: Vec<Vec<String>> = info
                .members
                .iter()
                .map(|m| {
                    vec![
                        crate::infra::config::labels::format_address(
                            &m.key.to_string(),
                            &cfg.labels,
                            cfg.truncate_addresses,
                        ),
                        table::format_permissions(m.permissions.0),
                    ]
                })
                .collect();
            table::print_table(headers, &rows);
        }
    }
    Ok(())
}

fn cmd_add(globals: GlobalOpts, parser: &mut lexopt::Parser) -> Result<(), MsigError> {
    use lexopt::Arg::*;
    let mut member_addr: Option<String> = None;
    let mut permissions_str: Option<String> = None;

    while let Some(arg) = parser.next().map_err(|e| MsigError::Usage(e.to_string()))? {
        match arg {
            Long("permissions") => permissions_str = Some(parse_value(parser, "--permissions")?),
            Short('h') | Long("help") => {
                super::help::print_resource_help("member");
                return Ok(());
            }
            Value(val) if member_addr.is_none() => {
                member_addr = Some(
                    val.into_string()
                        .map_err(|_| MsigError::Usage("invalid UTF-8".into()))?,
                );
            }
            _ => return Err(MsigError::Usage("unexpected argument".into())),
        }
    }

    let member_addr =
        member_addr.ok_or_else(|| MsigError::Usage("member address is required".into()))?;
    let permissions = config_tx::parse_permissions(
        permissions_str
            .as_deref()
            .unwrap_or("initiate,vote,execute"),
    )?;

    let output_mode = crate::output::detect_output_mode(globals.output.as_deref());
    let is_json = matches!(output_mode, OutputMode::Json);

    let ctx = build_context(&globals)?;
    let multisig = ctx.resolve_multisig(None)?;
    let result = config_tx::create_member_proposal(
        ctx.rpc(),
        ctx.signer.as_ref(),
        &multisig,
        &member_addr,
        permissions,
        true,
        &ctx.config,
        globals.dry_run,
        globals.yes,
        is_json,
    )?;

    if is_json {
        let json = serde_json::json!({
            "action": "member_add",
            "signature": result,
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&json).unwrap_or_default()
        );
    }
    Ok(())
}

fn cmd_remove(globals: GlobalOpts, parser: &mut lexopt::Parser) -> Result<(), MsigError> {
    use lexopt::Arg::*;
    let mut member_addr: Option<String> = None;

    while let Some(arg) = parser.next().map_err(|e| MsigError::Usage(e.to_string()))? {
        match arg {
            Short('h') | Long("help") => {
                super::help::print_resource_help("member");
                return Ok(());
            }
            Value(val) if member_addr.is_none() => {
                member_addr = Some(
                    val.into_string()
                        .map_err(|_| MsigError::Usage("invalid UTF-8".into()))?,
                );
            }
            _ => return Err(MsigError::Usage("unexpected argument".into())),
        }
    }

    let member_addr =
        member_addr.ok_or_else(|| MsigError::Usage("member address is required".into()))?;

    let output_mode = crate::output::detect_output_mode(globals.output.as_deref());
    let is_json = matches!(output_mode, OutputMode::Json);

    let ctx = build_context(&globals)?;
    let multisig = ctx.resolve_multisig(None)?;
    let result = config_tx::create_member_proposal(
        ctx.rpc(),
        ctx.signer.as_ref(),
        &multisig,
        &member_addr,
        0,
        false,
        &ctx.config,
        globals.dry_run,
        globals.yes,
        is_json,
    )?;

    if is_json {
        let json = serde_json::json!({
            "action": "member_remove",
            "signature": result,
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&json).unwrap_or_default()
        );
    }
    Ok(())
}
