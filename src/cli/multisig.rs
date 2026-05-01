use super::{build_context, load_config_only, next_verb, parse_value, GlobalOpts};
use crate::application::{config_tx, inspect, multisig};
use crate::domain::transaction::SpendingLimitPeriod;
use crate::error::{MsigError, OutputMode};
use crate::output::{format_sol, json, table};
use solana_pubkey::Pubkey;

pub fn run(globals: GlobalOpts, mut parser: lexopt::Parser) -> Result<(), MsigError> {
    let verb = next_verb(&mut parser, "multisig")?;
    match verb.as_str() {
        "create" => cmd_create(globals, &mut parser),
        "info" => cmd_info(globals, &mut parser),
        "set-threshold" => cmd_set_threshold(globals, &mut parser),
        "set-timelock" | "set-time-lock" => cmd_set_time_lock(globals, &mut parser),
        "add-spending-limit" => cmd_add_spending_limit(globals, &mut parser),
        "remove-spending-limit" => cmd_remove_spending_limit(globals, &mut parser),
        other => Err(MsigError::Usage(format!(
            "unknown multisig command: '{other}'"
        ))),
    }
}

fn cmd_set_threshold(globals: GlobalOpts, parser: &mut lexopt::Parser) -> Result<(), MsigError> {
    use lexopt::Arg::*;
    let mut threshold: Option<u16> = None;
    let mut memo: Option<String> = None;

    while let Some(arg) = parser.next().map_err(|e| MsigError::Usage(e.to_string()))? {
        match arg {
            Long("memo") => memo = Some(parse_value(parser, "--memo")?),
            Value(val) if threshold.is_none() => {
                let raw = val
                    .into_string()
                    .map_err(|_| MsigError::Usage("invalid UTF-8".into()))?;
                threshold = Some(
                    raw.parse()
                        .map_err(|_| MsigError::Usage(format!("invalid threshold: '{raw}'")))?,
                );
            }
            Short('h') | Long("help") => {
                super::help::print_resource_help("multisig");
                return Ok(());
            }
            _ => return Err(MsigError::Usage("unexpected argument".into())),
        }
    }

    let threshold = threshold.ok_or_else(|| MsigError::Usage("threshold is required".into()))?;
    let output_mode = crate::output::detect_output_mode(globals.output.as_deref());
    let is_json = matches!(output_mode, OutputMode::Json);

    let ctx = build_context(&globals)?;
    let multisig = ctx.resolve_multisig(None)?;
    let result = config_tx::create_change_threshold_proposal(
        ctx.rpc(),
        ctx.signer.as_ref(),
        &multisig,
        threshold,
        memo.as_deref(),
        &ctx.config,
        globals.dry_run,
        globals.yes,
        is_json,
    )?;

    if is_json {
        json::print_json(&serde_json::json!({
            "action": "multisig_set_threshold",
            "threshold": threshold,
            "signature": result,
        }));
    }
    Ok(())
}

fn cmd_set_time_lock(globals: GlobalOpts, parser: &mut lexopt::Parser) -> Result<(), MsigError> {
    use lexopt::Arg::*;
    let mut seconds: Option<u32> = None;
    let mut memo: Option<String> = None;

    while let Some(arg) = parser.next().map_err(|e| MsigError::Usage(e.to_string()))? {
        match arg {
            Long("memo") => memo = Some(parse_value(parser, "--memo")?),
            Value(val) if seconds.is_none() => {
                let raw = val
                    .into_string()
                    .map_err(|_| MsigError::Usage("invalid UTF-8".into()))?;
                seconds = Some(
                    raw.parse()
                        .map_err(|_| MsigError::Usage(format!("invalid timelock: '{raw}'")))?,
                );
            }
            Short('h') | Long("help") => {
                super::help::print_resource_help("multisig");
                return Ok(());
            }
            _ => return Err(MsigError::Usage("unexpected argument".into())),
        }
    }

    let seconds = seconds.ok_or_else(|| MsigError::Usage("timelock seconds is required".into()))?;
    let output_mode = crate::output::detect_output_mode(globals.output.as_deref());
    let is_json = matches!(output_mode, OutputMode::Json);

    let ctx = build_context(&globals)?;
    let multisig = ctx.resolve_multisig(None)?;
    let result = config_tx::create_set_time_lock_proposal(
        ctx.rpc(),
        ctx.signer.as_ref(),
        &multisig,
        seconds,
        memo.as_deref(),
        &ctx.config,
        globals.dry_run,
        globals.yes,
        is_json,
    )?;

    if is_json {
        json::print_json(&serde_json::json!({
            "action": "multisig_set_timelock",
            "seconds": seconds,
            "signature": result,
        }));
    }
    Ok(())
}

fn cmd_add_spending_limit(
    globals: GlobalOpts,
    parser: &mut lexopt::Parser,
) -> Result<(), MsigError> {
    use lexopt::Arg::*;

    let mut mint: Option<String> = None;
    let mut amount: Option<u64> = None;
    let mut period: Option<SpendingLimitPeriod> = None;
    let mut members: Option<String> = None;
    let mut destinations: Option<String> = None;
    let mut create_key: Option<String> = None;
    let mut vault_index_override: Option<u8> = None;
    let mut memo: Option<String> = None;

    while let Some(arg) = parser.next().map_err(|e| MsigError::Usage(e.to_string()))? {
        match arg {
            Long("mint") => mint = Some(parse_value(parser, "--mint")?),
            Long("amount") => {
                let raw = parse_value(parser, "--amount")?;
                amount = Some(
                    raw.parse()
                        .map_err(|_| MsigError::Usage(format!("invalid amount: '{raw}'")))?,
                );
            }
            Long("period") => {
                period = Some(parse_spending_limit_period(&parse_value(
                    parser, "--period",
                )?)?)
            }
            Long("members") => members = Some(parse_value(parser, "--members")?),
            Long("destinations") => destinations = Some(parse_value(parser, "--destinations")?),
            Long("create-key") => create_key = Some(parse_value(parser, "--create-key")?),
            Long("vault-index") => {
                let raw = parse_value(parser, "--vault-index")?;
                vault_index_override = Some(
                    raw.parse()
                        .map_err(|_| MsigError::Usage(format!("invalid vault-index: '{raw}'")))?,
                );
            }
            Long("memo") => memo = Some(parse_value(parser, "--memo")?),
            Short('h') | Long("help") => {
                super::help::print_resource_help("multisig");
                return Ok(());
            }
            _ => {
                return Err(MsigError::Usage(
                    "unexpected argument for multisig add-spending-limit".into(),
                ));
            }
        }
    }

    let mint = mint.ok_or_else(|| MsigError::Usage("--mint is required".into()))?;
    let amount = amount.ok_or_else(|| MsigError::Usage("--amount is required".into()))?;
    let period = period.ok_or_else(|| MsigError::Usage("--period is required".into()))?;
    let members = members.ok_or_else(|| MsigError::Usage("--members is required".into()))?;

    let output_mode = crate::output::detect_output_mode(globals.output.as_deref());
    let is_json = matches!(output_mode, OutputMode::Json);

    let ctx = build_context(&globals)?;
    let multisig = ctx.resolve_multisig(None)?;
    let vault_index = ctx.vault_index(vault_index_override.or(globals.vault_index));
    let mint = parse_mint(&mint, &ctx.config.labels)?;
    let member_pubkeys = parse_pubkey_csv(&members, "members", &ctx.config.labels)?;
    let destination_pubkeys = match destinations {
        Some(raw) => parse_pubkey_csv(&raw, "destinations", &ctx.config.labels)?,
        None => Vec::new(),
    };
    let create_key = create_key
        .as_deref()
        .map(|value| parse_labeled_pubkey(value, &ctx.config.labels))
        .transpose()?;

    let result = config_tx::create_add_spending_limit_proposal(
        ctx.rpc(),
        ctx.signer.as_ref(),
        &multisig,
        vault_index,
        mint,
        amount,
        period,
        member_pubkeys,
        destination_pubkeys,
        create_key,
        memo.as_deref(),
        &ctx.config,
        globals.dry_run,
        globals.yes,
        is_json,
    )?;

    if is_json {
        json::print_json(&serde_json::json!({
            "action": "multisig_add_spending_limit",
            "spending_limit": result.spending_limit.to_string(),
            "create_key": result.create_key.to_string(),
            "vault_index": vault_index,
            "signature": result.signature,
        }));
    } else {
        println!("Spending limit: {}", result.spending_limit);
        println!("Create key:     {}", result.create_key);
    }

    Ok(())
}

fn cmd_remove_spending_limit(
    globals: GlobalOpts,
    parser: &mut lexopt::Parser,
) -> Result<(), MsigError> {
    use lexopt::Arg::*;

    let mut spending_limit: Option<String> = None;
    let mut memo: Option<String> = None;

    while let Some(arg) = parser.next().map_err(|e| MsigError::Usage(e.to_string()))? {
        match arg {
            Value(value) if spending_limit.is_none() => {
                spending_limit = Some(
                    value
                        .into_string()
                        .map_err(|_| MsigError::Usage("invalid UTF-8".into()))?,
                );
            }
            Long("spending-limit") => {
                spending_limit = Some(parse_value(parser, "--spending-limit")?)
            }
            Long("memo") => memo = Some(parse_value(parser, "--memo")?),
            Short('h') | Long("help") => {
                super::help::print_resource_help("multisig");
                return Ok(());
            }
            _ => {
                return Err(MsigError::Usage(
                    "unexpected argument for multisig remove-spending-limit".into(),
                ));
            }
        }
    }

    let spending_limit = spending_limit
        .ok_or_else(|| MsigError::Usage("spending limit address is required".into()))?;
    let output_mode = crate::output::detect_output_mode(globals.output.as_deref());
    let is_json = matches!(output_mode, OutputMode::Json);
    let ctx = build_context(&globals)?;
    let multisig = ctx.resolve_multisig(None)?;
    let spending_limit = parse_labeled_pubkey(&spending_limit, &ctx.config.labels)?;

    let signature = config_tx::create_remove_spending_limit_proposal(
        ctx.rpc(),
        ctx.signer.as_ref(),
        &multisig,
        spending_limit,
        memo.as_deref(),
        &ctx.config,
        globals.dry_run,
        globals.yes,
        is_json,
    )?;

    if is_json {
        json::print_json(&serde_json::json!({
            "action": "multisig_remove_spending_limit",
            "spending_limit": spending_limit.to_string(),
            "signature": signature,
        }));
    }

    Ok(())
}

fn parse_spending_limit_period(raw: &str) -> Result<SpendingLimitPeriod, MsigError> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "one-time" | "onetime" | "once" => Ok(SpendingLimitPeriod::OneTime),
        "day" | "daily" => Ok(SpendingLimitPeriod::Day),
        "week" | "weekly" => Ok(SpendingLimitPeriod::Week),
        "month" | "monthly" => Ok(SpendingLimitPeriod::Month),
        other => Err(MsigError::Usage(format!(
            "invalid period '{other}'. Expected one-time, day, week, or month."
        ))),
    }
}

fn parse_mint(
    raw: &str,
    labels: &std::collections::HashMap<String, String>,
) -> Result<Pubkey, MsigError> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "native" | "sol" => Ok(Pubkey::default()),
        _ => parse_labeled_pubkey(raw, labels),
    }
}

fn parse_pubkey_csv(
    raw: &str,
    field: &str,
    labels: &std::collections::HashMap<String, String>,
) -> Result<Vec<Pubkey>, MsigError> {
    let mut values = Vec::new();
    for part in raw.split(',') {
        let value = part.trim();
        if value.is_empty() {
            continue;
        }
        values.push(parse_labeled_pubkey(value, labels)?);
    }
    if values.is_empty() {
        return Err(MsigError::Usage(format!("{field} cannot be empty")));
    }
    Ok(values)
}

fn parse_labeled_pubkey(
    raw: &str,
    labels: &std::collections::HashMap<String, String>,
) -> Result<Pubkey, MsigError> {
    let resolved = crate::infra::config::labels::resolve_address(raw, labels)
        .map_err(|_| MsigError::Usage(format!("invalid pubkey or label: '{raw}'")))?;
    resolved
        .parse()
        .map_err(|_| MsigError::Usage(format!("invalid pubkey: '{resolved}'")))
}

fn cmd_create(globals: GlobalOpts, parser: &mut lexopt::Parser) -> Result<(), MsigError> {
    use lexopt::Arg::*;
    let mut threshold: Option<u16> = None;
    let mut members: Option<String> = None;
    let mut rent_collector: Option<String> = None;
    let mut memo: Option<String> = None;

    while let Some(arg) = parser.next().map_err(|e| MsigError::Usage(e.to_string()))? {
        match arg {
            Long("threshold") => {
                let v = parse_value(parser, "--threshold")?;
                threshold = Some(
                    v.parse()
                        .map_err(|_| MsigError::Usage(format!("invalid threshold: '{v}'")))?,
                );
            }
            Long("members") => members = Some(parse_value(parser, "--members")?),
            Long("rent-collector") => {
                rent_collector = Some(parse_value(parser, "--rent-collector")?)
            }
            Long("memo") => memo = Some(parse_value(parser, "--memo")?),
            Short('h') | Long("help") => {
                super::help::print_resource_help("multisig");
                return Ok(());
            }
            _ => {
                return Err(MsigError::Usage(
                    "unexpected flag for multisig create".into(),
                ))
            }
        }
    }

    let threshold = threshold.ok_or_else(|| MsigError::Usage("--threshold is required".into()))?;
    let members_str = members.ok_or_else(|| MsigError::Usage("--members is required".into()))?;
    let member_list: Vec<String> = members_str
        .split(',')
        .map(|s| s.trim().to_string())
        .collect();

    let output_mode = crate::output::detect_output_mode(globals.output.as_deref());
    let is_json = matches!(output_mode, OutputMode::Json);

    let ctx = build_context(&globals)?;
    let result = if is_json {
        multisig::create_multisig_quiet(
            ctx.rpc(),
            ctx.signer.as_ref(),
            threshold,
            &member_list,
            rent_collector.as_deref(),
            memo.as_deref(),
            &ctx.config,
            globals.dry_run,
            globals.yes,
        )?
    } else {
        multisig::create_multisig(
            ctx.rpc(),
            ctx.signer.as_ref(),
            threshold,
            &member_list,
            rent_collector.as_deref(),
            memo.as_deref(),
            &ctx.config,
            globals.dry_run,
            globals.yes,
        )?
    };

    if is_json {
        let json = serde_json::json!({
            "action": "multisig_create",
            "signature": result.signature,
            "multisig_address": result.multisig_address,
            "create_key": result.create_key,
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&json).unwrap_or_default()
        );
    }
    Ok(())
}

fn cmd_info(globals: GlobalOpts, parser: &mut lexopt::Parser) -> Result<(), MsigError> {
    use lexopt::Arg::*;
    let mut multisig_override: Option<String> = None;
    while let Some(arg) = parser.next().map_err(|e| MsigError::Usage(e.to_string()))? {
        match arg {
            Long("multisig") => multisig_override = Some(parse_value(parser, "--multisig")?),
            Short('h') | Long("help") => {
                super::help::print_resource_help("multisig");
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

    let vault_index = globals.vault_index.unwrap_or(cfg.vault_index);
    let program_id = globals
        .program_id
        .as_deref()
        .map(|id| {
            id.parse()
                .map_err(|_| MsigError::Usage(format!("invalid program-id: '{id}'")))
        })
        .transpose()?
        .unwrap_or(cfg.program_id);

    let info = inspect::fetch_multisig_info(&rpc, &multisig_pubkey, vault_index, &program_id)?;

    match output_mode {
        OutputMode::Json => json::print_json(&info),
        OutputMode::Text => {
            println!("Multisig: {}", info.address);
            println!("Threshold: {}/{}", info.threshold, info.members.len());
            println!("Transaction index: {}", info.transaction_index);
            println!("Time lock: {}s", info.time_lock);
            println!(
                "Vault: {} ({} SOL)",
                info.vault_address,
                format_sol(info.vault_balance_lamports)
            );
            if let Some(rc) = &info.rent_collector {
                println!("Rent collector: {rc}");
            }
            println!();
            let headers = &["Member", "Permissions"];
            let rows: Vec<Vec<String>> = info
                .members
                .iter()
                .map(|m| {
                    vec![
                        m.key.to_string(),
                        table::format_permissions(m.permissions.0),
                    ]
                })
                .collect();
            table::print_table(headers, &rows);
        }
    }
    Ok(())
}
