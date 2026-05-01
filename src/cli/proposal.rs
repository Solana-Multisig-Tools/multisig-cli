use super::{build_context, load_config_only, next_verb, parse_value, GlobalOpts};
use crate::application::{inspect, proposal};
use crate::domain::proposal::{ProposalStatus, ProposalSummary, Vote};
use crate::error::{MsigError, OutputMode};
use crate::infra::accounts::proposal::ProposalAccount;
use crate::infra::rpc::RpcProvider;
use crate::output::{json, table};

pub fn run(globals: GlobalOpts, mut parser: lexopt::Parser) -> Result<(), MsigError> {
    let verb = next_verb(&mut parser, "proposal")?;
    match verb.as_str() {
        "list" => cmd_list(globals, &mut parser),
        "pending" => cmd_queue(globals, &mut parser, QueueMode::Pending),
        "executable" => cmd_queue(globals, &mut parser, QueueMode::Executable),
        "needs-me" => cmd_queue(globals, &mut parser, QueueMode::NeedsMe),
        "show" => cmd_show(globals, &mut parser),
        "simulate" => cmd_simulate(globals, &mut parser),
        "approve" => cmd_vote(globals, &mut parser, Vote::Approve),
        "reject" => cmd_vote(globals, &mut parser, Vote::Reject),
        "cancel" => cmd_vote(globals, &mut parser, Vote::Cancel),
        "execute" => cmd_execute(globals, &mut parser),
        other => Err(MsigError::Usage(format!(
            "unknown proposal command: '{other}'"
        ))),
    }
}

fn cmd_simulate(globals: GlobalOpts, parser: &mut lexopt::Parser) -> Result<(), MsigError> {
    use lexopt::Arg::*;
    let mut proposal_ref: Option<String> = None;
    let mut verbose = false;

    while let Some(arg) = parser.next().map_err(|e| MsigError::Usage(e.to_string()))? {
        match arg {
            Long("verbose") => verbose = true,
            Short('h') | Long("help") => {
                super::help::print_resource_help("proposal");
                return Ok(());
            }
            Value(val) if proposal_ref.is_none() => {
                proposal_ref = Some(
                    val.into_string()
                        .map_err(|_| MsigError::Usage("invalid UTF-8".into()))?,
                );
            }
            _ => return Err(MsigError::Usage("unexpected argument".into())),
        }
    }

    let proposal_ref = proposal_ref
        .ok_or_else(|| MsigError::Usage("proposal index or address is required".into()))?;
    let cfg = load_config_only(&globals)?;
    let output_mode = crate::output::detect_output_mode(globals.output.as_deref());
    let rpc = crate::infra::rpc::SolanaRpcClient::with_commitment(&cfg.cluster, &cfg.commitment);
    let signer = crate::infra::signer::resolve_signer(
        globals.ledger.as_deref(),
        globals.keypair.as_deref(),
        cfg.keypair.as_deref(),
    )?;
    let multisig_pubkey = resolve_multisig_for_read(&globals, &cfg, None)?;
    let index = resolve_proposal_index(&rpc, &proposal_ref, &multisig_pubkey, &cfg)?;

    let plan =
        proposal::prepare_execute_proposal(&rpc, &multisig_pubkey, index, signer.pubkey(), &cfg)?;
    let watched_accounts: Vec<String> = plan
        .watched_accounts
        .iter()
        .map(ToString::to_string)
        .collect();
    let account_refs: Vec<&str> = watched_accounts.iter().map(String::as_str).collect();
    let before = rpc.get_multiple_accounts(&account_refs)?;

    let (recent_blockhash, _) = rpc.get_latest_blockhash()?;
    let tx_b64 = crate::application::pipeline::compile_unsigned_legacy_transaction_base64(
        &[plan.instruction],
        &signer.pubkey(),
        &recent_blockhash,
    )?;
    let simulation = rpc.simulate_transaction_with_accounts(&tx_b64, &watched_accounts)?;
    let diffs = build_account_diffs(&watched_accounts, &before, &simulation.accounts, &cfg);

    match output_mode {
        OutputMode::Json => json::print_json(&serde_json::json!({
            "action": "proposal_simulate",
            "proposal_index": index,
            "ok": simulation.err.is_none(),
            "error": simulation.err,
            "units_consumed": simulation.units_consumed,
            "account_diffs": diffs,
            "logs": simulation.logs,
        })),
        OutputMode::Text => render_simulation(index, &simulation, &diffs, verbose),
    }
    Ok(())
}

fn cmd_list(globals: GlobalOpts, parser: &mut lexopt::Parser) -> Result<(), MsigError> {
    use lexopt::Arg::*;
    let mut limit: u64 = 10;
    let mut multisig_override: Option<String> = None;
    let mut status_filter: Option<String> = None;

    while let Some(arg) = parser.next().map_err(|e| MsigError::Usage(e.to_string()))? {
        match arg {
            Long("limit") => {
                let v = parse_value(parser, "--limit")?;
                limit = v
                    .parse()
                    .map_err(|_| MsigError::Usage(format!("invalid limit: '{v}'")))?;
            }
            Long("multisig") => multisig_override = Some(parse_value(parser, "--multisig")?),
            Long("status") => status_filter = Some(parse_value(parser, "--status")?),
            Short('h') | Long("help") => {
                super::help::print_resource_help("proposal");
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

    let mut proposals = inspect::list_proposals(&rpc, &multisig_pubkey, limit, &cfg.program_id)?;

    // Apply status filter if provided
    if let Some(ref filter) = status_filter {
        let filter_lower = filter.to_lowercase();
        proposals.retain(|p| p.status.label().to_lowercase() == filter_lower);
    }

    match output_mode {
        OutputMode::Json => json::print_json(&proposals),
        OutputMode::Text => render_proposals(&proposals),
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum QueueMode {
    Pending,
    Executable,
    NeedsMe,
}

fn cmd_queue(
    globals: GlobalOpts,
    parser: &mut lexopt::Parser,
    mode: QueueMode,
) -> Result<(), MsigError> {
    use lexopt::Arg::*;
    let mut limit: u64 = 25;
    let mut multisig_override: Option<String> = None;

    while let Some(arg) = parser.next().map_err(|e| MsigError::Usage(e.to_string()))? {
        match arg {
            Long("limit") => {
                let v = parse_value(parser, "--limit")?;
                limit = v
                    .parse()
                    .map_err(|_| MsigError::Usage(format!("invalid limit: '{v}'")))?;
            }
            Long("multisig") => multisig_override = Some(parse_value(parser, "--multisig")?),
            Short('h') | Long("help") => {
                super::help::print_resource_help("proposal");
                return Ok(());
            }
            _ => return Err(MsigError::Usage("unexpected flag".into())),
        }
    }

    let cfg = load_config_only(&globals)?;
    let output_mode = crate::output::detect_output_mode(globals.output.as_deref());
    let rpc = crate::infra::rpc::SolanaRpcClient::with_commitment(&cfg.cluster, &cfg.commitment);
    let multisig_pubkey = resolve_multisig_for_read(&globals, &cfg, multisig_override.as_deref())?;

    let mut proposals = inspect::list_proposals(&rpc, &multisig_pubkey, limit, &cfg.program_id)?;
    match mode {
        QueueMode::Pending => {
            proposals.retain(|p| {
                matches!(
                    p.status,
                    ProposalStatus::Active { .. }
                        | ProposalStatus::Approved { .. }
                        | ProposalStatus::Executing
                )
            });
        }
        QueueMode::Executable => {
            proposals.retain(|p| p.status.is_approved());
        }
        QueueMode::NeedsMe => {
            let signer = crate::infra::signer::resolve_signer(
                globals.ledger.as_deref(),
                globals.keypair.as_deref(),
                cfg.keypair.as_deref(),
            )?;
            let signer_pubkey = signer.pubkey();
            let info = inspect::fetch_multisig_info(
                &rpc,
                &multisig_pubkey,
                cfg.vault_index,
                &cfg.program_id,
            )?;
            let member = info.members.iter().find(|m| m.key == signer_pubkey);
            match member {
                Some(member) if member.permissions.can_vote() => {}
                Some(_) => {
                    return Err(MsigError::PermissionDenied(
                        "signer does not have Vote permission".into(),
                    ));
                }
                None => {
                    return Err(MsigError::PermissionDenied(
                        "signer is not a member of this multisig".into(),
                    ));
                }
            }

            let mut filtered = Vec::new();
            for summary in proposals
                .into_iter()
                .filter(|p| matches!(p.status, ProposalStatus::Active { .. }))
            {
                let detail = inspect::get_proposal_detail(
                    &rpc,
                    &multisig_pubkey,
                    summary.index,
                    &cfg.program_id,
                    cfg.truncate_addresses,
                )?;
                let already_voted = detail.approved.contains(&signer_pubkey)
                    || detail.rejected.contains(&signer_pubkey)
                    || detail.cancelled.contains(&signer_pubkey);
                if !already_voted {
                    filtered.push(summary);
                }
            }
            proposals = filtered;
        }
    }

    match output_mode {
        OutputMode::Json => json::print_json(&proposals),
        OutputMode::Text => render_proposals(&proposals),
    }
    Ok(())
}

fn resolve_multisig_for_read(
    globals: &GlobalOpts,
    cfg: &crate::infra::config::Config,
    override_addr: Option<&str>,
) -> Result<solana_pubkey::Pubkey, MsigError> {
    let addr_str = override_addr
        .or(globals.multisig.as_deref())
        .or(cfg.multisig.as_deref())
        .ok_or_else(|| MsigError::Config("no multisig address".into()))?;
    let resolved = crate::infra::config::labels::resolve_address(addr_str, &cfg.labels)?;
    resolved
        .parse()
        .map_err(|_| MsigError::Usage(format!("invalid multisig address: '{resolved}'")))
}

fn resolve_proposal_index(
    rpc: &dyn RpcProvider,
    proposal_ref: &str,
    multisig: &solana_pubkey::Pubkey,
    cfg: &crate::infra::config::Config,
) -> Result<u64, MsigError> {
    if let Ok(index) = proposal_ref.parse::<u64>() {
        return Ok(index);
    }

    let resolved = crate::infra::config::labels::resolve_address(proposal_ref, &cfg.labels)
        .map_err(|_| {
            MsigError::Usage(format!(
                "proposal reference must be an index or proposal address, got '{proposal_ref}'"
            ))
        })?;
    let proposal_pubkey: solana_pubkey::Pubkey = resolved
        .parse()
        .map_err(|_| MsigError::Usage(format!("invalid proposal address: '{resolved}'")))?;
    let account = rpc
        .get_account_info(&proposal_pubkey.to_string())?
        .ok_or_else(|| {
            MsigError::Transaction(format!("proposal account {proposal_pubkey} not found"))
        })?;
    let expected_owner = cfg.program_id.to_string();
    if account.owner != expected_owner {
        return Err(MsigError::Transaction(format!(
            "proposal account {proposal_pubkey} is owned by {}, expected {}",
            account.owner, expected_owner
        )));
    }
    let proposal = ProposalAccount::parse(&account.data)?;
    if proposal.multisig != *multisig {
        return Err(MsigError::Transaction(format!(
            "proposal account {proposal_pubkey} belongs to multisig {}, not {}",
            proposal.multisig, multisig
        )));
    }
    Ok(proposal.transaction_index)
}

fn render_proposals(proposals: &[ProposalSummary]) {
    if proposals.is_empty() {
        println!("No proposals found.");
        return;
    }
    let headers = &["Index", "Status", "Votes", "Time", "Proposal"];
    let rows: Vec<Vec<String>> = proposals
        .iter()
        .map(|p| {
            let time = p
                .status
                .timestamp()
                .map(table::format_relative_time)
                .unwrap_or_default();
            vec![
                format!("{}", p.index),
                table::format_status(p.status.label()),
                format!("{}/{}", p.approved_count, p.threshold),
                time,
                p.address.to_string(),
            ]
        })
        .collect();
    table::print_table(headers, &rows);
}

fn cmd_show(globals: GlobalOpts, parser: &mut lexopt::Parser) -> Result<(), MsigError> {
    use lexopt::Arg::*;
    let mut proposal_ref: Option<String> = None;
    let mut verbose = false;

    while let Some(arg) = parser.next().map_err(|e| MsigError::Usage(e.to_string()))? {
        match arg {
            Long("verbose") => verbose = true,
            Short('h') | Long("help") => {
                super::help::print_resource_help("proposal");
                return Ok(());
            }
            Value(val) if proposal_ref.is_none() => {
                proposal_ref = Some(
                    val.into_string()
                        .map_err(|_| MsigError::Usage("invalid UTF-8".into()))?,
                );
            }
            _ => return Err(MsigError::Usage("unexpected argument".into())),
        }
    }

    let proposal_ref = proposal_ref
        .ok_or_else(|| MsigError::Usage("proposal index or address is required".into()))?;

    let cfg = load_config_only(&globals)?;
    let output_mode = crate::output::detect_output_mode(globals.output.as_deref());
    let rpc = crate::infra::rpc::SolanaRpcClient::with_commitment(&cfg.cluster, &cfg.commitment);

    let addr_str = globals
        .multisig
        .as_deref()
        .or(cfg.multisig.as_deref())
        .ok_or_else(|| MsigError::Config("no multisig address".into()))?;
    let resolved = crate::infra::config::labels::resolve_address(addr_str, &cfg.labels)?;
    let multisig_pubkey: solana_pubkey::Pubkey = resolved
        .parse()
        .map_err(|_| MsigError::Usage(format!("invalid multisig address: '{resolved}'")))?;
    let index = resolve_proposal_index(&rpc, &proposal_ref, &multisig_pubkey, &cfg)?;

    let detail = inspect::get_proposal_detail(
        &rpc,
        &multisig_pubkey,
        index,
        &cfg.program_id,
        cfg.truncate_addresses,
    )?;

    match output_mode {
        OutputMode::Json => json::print_json(&detail),
        OutputMode::Text => {
            render_proposal_detail(index, &detail, &cfg.labels, verbose, cfg.truncate_addresses);
        }
    }
    Ok(())
}

fn render_proposal_detail(
    index: u64,
    detail: &crate::domain::proposal::ProposalDetail,
    labels: &std::collections::HashMap<String, String>,
    verbose: bool,
    truncate: bool,
) {
    println!("Proposal #{index}");
    println!(
        "  Status: {}",
        table::format_status(detail.summary.status.label())
    );
    println!(
        "  Votes: {}/{}",
        detail.summary.approved_count, detail.summary.threshold
    );
    println!("  Type: {}", detail.transaction_type.label());
    if !detail.approved.is_empty() {
        println!("  Approved by:");
        for pk in &detail.approved {
            println!("    {}", format_labeled_address(pk, labels, truncate));
        }
    }
    if !detail.rejected.is_empty() {
        println!("  Rejected by:");
        for pk in &detail.rejected {
            println!("    {}", format_labeled_address(pk, labels, truncate));
        }
    }
    if !detail.cancelled.is_empty() {
        println!("  Cancelled by:");
        for pk in &detail.cancelled {
            println!("    {}", format_labeled_address(pk, labels, truncate));
        }
    }
    if let Some(ref vtx) = detail.vault_tx {
        println!("  Vault index: {}", vtx.vault_index);
        println!("  Instructions: {}", vtx.instruction_count);
        for (i, ix) in vtx.instructions.iter().enumerate() {
            let data_bytes = ix.data_hex.len() / 2;
            if let Some(ref decoded) = ix.decoded {
                println!("    [{i}] {decoded}");
                println!("        program: {} ({})", ix.program_name, ix.program_id);
            } else {
                println!("    [{i}] {} ({})", ix.program_name, ix.program_id);
            }
            println!(
                "        accounts: {}, data: {data_bytes} bytes",
                ix.accounts.len()
            );
            if verbose {
                println!("        account metas:");
                for acct in &ix.accounts {
                    let mut flags = Vec::new();
                    if acct.is_writable {
                        flags.push("writable");
                    } else {
                        flags.push("readonly");
                    }
                    if acct.is_signer {
                        flags.push("signer");
                    }
                    println!(
                        "          {} ({})",
                        format_labeled_address(&acct.address, labels, truncate),
                        flags.join(", ")
                    );
                }
                if ix.data_hex.is_empty() {
                    println!("        data: <empty>");
                } else {
                    println!("        data: 0x{}", ix.data_hex);
                }
            }
        }
    }
    if let Some(ref ctx) = detail.config_tx {
        println!("  Config actions: {}", ctx.action_count);
        for (idx, action) in ctx.actions.iter().enumerate() {
            println!("    [{idx}] {}", format_config_action(action));
        }
    }
}

fn format_config_action(action: &crate::domain::transaction::ConfigAction) -> String {
    use crate::domain::transaction::ConfigAction;
    match action {
        ConfigAction::AddMember {
            member,
            permissions,
        } => format!("Add member {member} permissions={permissions}"),
        ConfigAction::RemoveMember { member } => format!("Remove member {member}"),
        ConfigAction::ChangeThreshold { new_threshold } => {
            format!("Change threshold to {new_threshold}")
        }
        ConfigAction::SetTimeLock { new_time_lock } => {
            format!("Set time lock to {new_time_lock}s")
        }
        ConfigAction::AddSpendingLimit {
            create_key,
            vault_index,
            mint,
            amount,
            period,
            members,
            destinations,
        } => format!(
            "Add spending limit create_key={create_key} vault_index={vault_index} mint={mint} amount={amount} period={period:?} members={} destinations={}",
            members.len(),
            destinations.len()
        ),
        ConfigAction::RemoveSpendingLimit { spending_limit } => {
            format!("Remove spending limit {spending_limit}")
        }
        ConfigAction::SetRentCollector { new_rent_collector } => match new_rent_collector {
            Some(rent_collector) => format!("Set rent collector to {rent_collector}"),
            None => "Clear rent collector".to_string(),
        },
        ConfigAction::Unknown { tag, raw } => {
            format!("Unknown config action tag={tag} bytes={}", raw.len())
        }
    }
}

fn format_labeled_address(
    pubkey: &solana_pubkey::Pubkey,
    labels: &std::collections::HashMap<String, String>,
    truncate: bool,
) -> String {
    crate::infra::config::labels::format_address(&pubkey.to_string(), labels, truncate)
}

#[derive(Debug, Clone, serde::Serialize)]
struct AccountDiff {
    address: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    label: Option<String>,
    exists_before: bool,
    exists_after: bool,
    lamports_before: u64,
    lamports_after: u64,
    lamports_delta: i128,
    #[serde(skip_serializing_if = "Option::is_none")]
    owner_before: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    owner_after: Option<String>,
    data_len_before: usize,
    data_len_after: usize,
    data_changed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    token: Option<TokenDiff>,
}

impl AccountDiff {
    fn changed(&self) -> bool {
        self.exists_before != self.exists_after
            || self.lamports_delta != 0
            || self.owner_before != self.owner_after
            || self.data_changed
            || self
                .token
                .as_ref()
                .map(|token| token.amount_delta != 0)
                .unwrap_or(false)
    }
}

#[derive(Debug, Clone, serde::Serialize)]
struct TokenDiff {
    mint: String,
    owner: String,
    amount_before: u64,
    amount_after: u64,
    amount_delta: i128,
    #[serde(skip_serializing_if = "Option::is_none")]
    decimals: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    symbol: Option<String>,
}

#[derive(Debug, Clone)]
struct TokenAccountView {
    mint: solana_pubkey::Pubkey,
    owner: solana_pubkey::Pubkey,
    amount: u64,
}

fn build_account_diffs(
    addresses: &[String],
    before: &[Option<crate::infra::rpc::AccountData>],
    after: &[Option<crate::infra::rpc::AccountData>],
    cfg: &crate::infra::config::Config,
) -> Vec<AccountDiff> {
    addresses
        .iter()
        .enumerate()
        .map(|(idx, address)| {
            let before_account = before.get(idx).and_then(Option::as_ref);
            let after_account = after.get(idx).and_then(Option::as_ref);
            let before_token = before_account.and_then(parse_token_account);
            let after_token = after_account.and_then(parse_token_account);
            let token = build_token_diff(before_token, after_token, cfg);
            AccountDiff {
                address: address.clone(),
                label: cfg.labels.get(address).cloned(),
                exists_before: before_account.is_some(),
                exists_after: after_account.is_some(),
                lamports_before: before_account.map(|account| account.lamports).unwrap_or(0),
                lamports_after: after_account.map(|account| account.lamports).unwrap_or(0),
                lamports_delta: i128::from(
                    after_account.map(|account| account.lamports).unwrap_or(0),
                ) - i128::from(
                    before_account.map(|account| account.lamports).unwrap_or(0),
                ),
                owner_before: before_account.map(|account| account.owner.clone()),
                owner_after: after_account.map(|account| account.owner.clone()),
                data_len_before: before_account
                    .map(|account| account.data.len())
                    .unwrap_or(0),
                data_len_after: after_account.map(|account| account.data.len()).unwrap_or(0),
                data_changed: match (before_account, after_account) {
                    (Some(before), Some(after)) => before.data != after.data,
                    (None, None) => false,
                    _ => true,
                },
                token,
            }
        })
        .collect()
}

fn parse_token_account(account: &crate::infra::rpc::AccountData) -> Option<TokenAccountView> {
    const TOKEN_PROGRAM: &str = "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA";
    const TOKEN_2022_PROGRAM: &str = "TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb";
    if account.owner != TOKEN_PROGRAM && account.owner != TOKEN_2022_PROGRAM {
        return None;
    }
    if account.data.len() < 72 {
        return None;
    }
    let mut mint = [0u8; 32];
    mint.copy_from_slice(&account.data[..32]);
    let mut owner = [0u8; 32];
    owner.copy_from_slice(&account.data[32..64]);
    let amount = u64::from_le_bytes([
        account.data[64],
        account.data[65],
        account.data[66],
        account.data[67],
        account.data[68],
        account.data[69],
        account.data[70],
        account.data[71],
    ]);
    Some(TokenAccountView {
        mint: solana_pubkey::Pubkey::from(mint),
        owner: solana_pubkey::Pubkey::from(owner),
        amount,
    })
}

fn build_token_diff(
    before: Option<TokenAccountView>,
    after: Option<TokenAccountView>,
    cfg: &crate::infra::config::Config,
) -> Option<TokenDiff> {
    let view = after.as_ref().or(before.as_ref())?;
    let mint = view.mint.to_string();
    let (symbol, decimals) = token_metadata(&mint, cfg);
    Some(TokenDiff {
        mint,
        owner: view.owner.to_string(),
        amount_before: before.as_ref().map(|token| token.amount).unwrap_or(0),
        amount_after: after.as_ref().map(|token| token.amount).unwrap_or(0),
        amount_delta: i128::from(after.as_ref().map(|token| token.amount).unwrap_or(0))
            - i128::from(before.as_ref().map(|token| token.amount).unwrap_or(0)),
        decimals,
        symbol,
    })
}

fn token_metadata(mint: &str, cfg: &crate::infra::config::Config) -> (Option<String>, Option<u8>) {
    let known = match mint {
        "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v" => Some(("USDC".to_string(), 6)),
        "Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB" => Some(("USDT".to_string(), 6)),
        "So11111111111111111111111111111111111111112" => Some(("WSOL".to_string(), 9)),
        _ => None,
    };
    if let Some((symbol, decimals)) = known {
        return (Some(symbol), Some(decimals));
    }
    let symbol = cfg
        .tokens
        .iter()
        .find(|(_, configured_mint)| configured_mint.as_str() == mint)
        .map(|(symbol, _)| symbol.clone());
    (symbol, None)
}

fn render_simulation(
    index: u64,
    simulation: &crate::infra::rpc::types::SimulationResult,
    diffs: &[AccountDiff],
    verbose: bool,
) {
    println!("Simulation for proposal #{index}");
    match &simulation.err {
        Some(err) => println!("  Result: FAILED ({err})"),
        None => println!("  Result: OK"),
    }
    println!("  Compute units: {}", simulation.units_consumed);

    let visible_diffs: Vec<&AccountDiff> = diffs
        .iter()
        .filter(|diff| verbose || diff.changed())
        .collect();
    if visible_diffs.is_empty() {
        println!("  Account changes: none detected");
    } else {
        println!("  Account changes:");
        for diff in visible_diffs {
            render_account_diff(diff);
        }
    }

    if (verbose || simulation.err.is_some()) && !simulation.logs.is_empty() {
        println!("  Logs:");
        for log in &simulation.logs {
            println!("    {}", crate::sanitize::sanitize_display(log));
        }
    }
}

fn render_account_diff(diff: &AccountDiff) {
    let name = diff
        .label
        .as_ref()
        .map(|label| format!("{label} ({})", abbreviate(&diff.address)))
        .unwrap_or_else(|| diff.address.clone());
    println!("    {name}");
    if diff.exists_before != diff.exists_after {
        let state = match (diff.exists_before, diff.exists_after) {
            (false, true) => "created",
            (true, false) => "closed",
            _ => "changed",
        };
        println!("      account: {state}");
    }
    if diff.lamports_delta != 0 {
        println!(
            "      SOL: {} -> {} ({})",
            crate::output::format_sol(diff.lamports_before),
            crate::output::format_sol(diff.lamports_after),
            format_signed_sol(diff.lamports_delta),
        );
    }
    if let Some(token) = &diff.token {
        if token.amount_delta != 0 {
            let suffix = token
                .symbol
                .as_deref()
                .map(|symbol| format!(" {symbol}"))
                .unwrap_or_default();
            println!(
                "      token: {} -> {} ({}{}) mint {}",
                format_token_amount(token.amount_before, token.decimals),
                format_token_amount(token.amount_after, token.decimals),
                format_signed_token(token.amount_delta, token.decimals),
                suffix,
                token.mint,
            );
        }
    }
    if diff.owner_before != diff.owner_after {
        println!(
            "      owner: {} -> {}",
            diff.owner_before.as_deref().unwrap_or("(none)"),
            diff.owner_after.as_deref().unwrap_or("(none)")
        );
    }
    if diff.data_changed
        && diff
            .token
            .as_ref()
            .map(|t| t.amount_delta == 0)
            .unwrap_or(true)
    {
        println!(
            "      data: {} bytes -> {} bytes",
            diff.data_len_before, diff.data_len_after
        );
    }
}

fn format_signed_sol(delta: i128) -> String {
    let sign = if delta >= 0 { "+" } else { "-" };
    let amount = delta.unsigned_abs();
    let lamports = u64::try_from(amount).unwrap_or(u64::MAX);
    format!("{sign}{} SOL", crate::output::format_sol(lamports))
}

fn format_token_amount(amount: u64, decimals: Option<u8>) -> String {
    let Some(decimals) = decimals else {
        return amount.to_string();
    };
    if decimals == 0 {
        return amount.to_string();
    }
    let divisor = 10u64.pow(u32::from(decimals));
    let whole = amount / divisor;
    let frac = amount % divisor;
    let frac_str = format!("{frac:0>width$}", width = usize::from(decimals));
    format!("{whole}.{}", frac_str.trim_end_matches('0'))
        .trim_end_matches('.')
        .to_string()
}

fn format_signed_token(delta: i128, decimals: Option<u8>) -> String {
    let sign = if delta >= 0 { "+" } else { "-" };
    let amount = delta.unsigned_abs();
    let amount = u64::try_from(amount).unwrap_or(u64::MAX);
    format!("{sign}{}", format_token_amount(amount, decimals))
}

fn abbreviate(address: &str) -> String {
    if address.len() <= 8 {
        address.to_string()
    } else {
        format!("{}...{}", &address[..4], &address[address.len() - 4..])
    }
}

fn cmd_vote(globals: GlobalOpts, parser: &mut lexopt::Parser, vote: Vote) -> Result<(), MsigError> {
    use lexopt::Arg::*;
    let mut proposal_ref: Option<String> = None;

    while let Some(arg) = parser.next().map_err(|e| MsigError::Usage(e.to_string()))? {
        match arg {
            Short('h') | Long("help") => {
                super::help::print_resource_help("proposal");
                return Ok(());
            }
            Value(val) if proposal_ref.is_none() => {
                proposal_ref = Some(
                    val.into_string()
                        .map_err(|_| MsigError::Usage("invalid UTF-8".into()))?,
                );
            }
            _ => return Err(MsigError::Usage("unexpected argument".into())),
        }
    }

    let proposal_ref = proposal_ref
        .ok_or_else(|| MsigError::Usage("proposal index or address is required".into()))?;
    let output_mode = crate::output::detect_output_mode(globals.output.as_deref());
    let is_json = matches!(output_mode, OutputMode::Json);

    let ctx = build_context(&globals)?;
    let multisig = ctx.resolve_multisig(None)?;
    let index = resolve_proposal_index(ctx.rpc(), &proposal_ref, &multisig, &ctx.config)?;

    // Use quiet pipeline for JSON mode
    let result = if is_json {
        crate::application::proposal::create_vote_proposal_quiet(
            ctx.rpc(),
            ctx.signer.as_ref(),
            &multisig,
            index,
            vote,
            &ctx.config,
            globals.dry_run,
            globals.yes,
        )?
    } else {
        proposal::create_vote_proposal(
            ctx.rpc(),
            ctx.signer.as_ref(),
            &multisig,
            index,
            vote,
            &ctx.config,
            globals.dry_run,
            globals.yes,
        )?
    };

    if is_json {
        let json = serde_json::json!({
            "action": format!("proposal_{}", vote.label().to_lowercase()),
            "proposal_index": index,
            "signature": result,
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&json).unwrap_or_default()
        );
    }
    Ok(())
}

fn cmd_execute(globals: GlobalOpts, parser: &mut lexopt::Parser) -> Result<(), MsigError> {
    use lexopt::Arg::*;
    let mut proposal_ref: Option<String> = None;

    while let Some(arg) = parser.next().map_err(|e| MsigError::Usage(e.to_string()))? {
        match arg {
            Short('h') | Long("help") => {
                super::help::print_resource_help("proposal");
                return Ok(());
            }
            Value(val) if proposal_ref.is_none() => {
                proposal_ref = Some(
                    val.into_string()
                        .map_err(|_| MsigError::Usage("invalid UTF-8".into()))?,
                );
            }
            _ => return Err(MsigError::Usage("unexpected argument".into())),
        }
    }

    let proposal_ref = proposal_ref
        .ok_or_else(|| MsigError::Usage("proposal index or address is required".into()))?;
    let output_mode = crate::output::detect_output_mode(globals.output.as_deref());
    let is_json = matches!(output_mode, OutputMode::Json);

    let ctx = build_context(&globals)?;
    let multisig = ctx.resolve_multisig(None)?;
    let index = resolve_proposal_index(ctx.rpc(), &proposal_ref, &multisig, &ctx.config)?;

    let result = if is_json {
        crate::application::proposal::execute_proposal_quiet(
            ctx.rpc(),
            ctx.signer.as_ref(),
            &multisig,
            index,
            &ctx.config,
            globals.dry_run,
            globals.yes,
        )?
    } else {
        proposal::execute_proposal(
            ctx.rpc(),
            ctx.signer.as_ref(),
            &multisig,
            index,
            &ctx.config,
            globals.dry_run,
            globals.yes,
        )?
    };

    if is_json {
        let json = serde_json::json!({
            "action": "proposal_execute",
            "proposal_index": index,
            "signature": result,
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&json).unwrap_or_default()
        );
    }
    Ok(())
}
