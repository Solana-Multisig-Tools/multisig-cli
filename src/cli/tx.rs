use super::{build_context, load_config_only, next_verb, parse_value, GlobalOpts};
use crate::application::{inspect, vault_tx};
use crate::error::{MsigError, OutputMode};
use crate::infra::instruction::{AccountMeta, Instruction};
use crate::infra::signer::offline;
use crate::output::json;
use solana_pubkey::Pubkey;

pub fn run(globals: GlobalOpts, mut parser: lexopt::Parser) -> Result<(), MsigError> {
    let verb = next_verb(&mut parser, "tx")?;
    match verb.as_str() {
        "show" => cmd_show(globals, &mut parser),
        "list" => cmd_list(globals, &mut parser),
        "create" => cmd_create(globals, &mut parser),
        "export" => cmd_export(globals, &mut parser),
        "status" => cmd_status(globals, &mut parser),
        "combine" => cmd_combine(globals, &mut parser),
        "import" => cmd_import(globals, &mut parser),
        other => Err(MsigError::Usage(format!("unknown tx command: '{other}'"))),
    }
}

fn cmd_show(globals: GlobalOpts, parser: &mut lexopt::Parser) -> Result<(), MsigError> {
    use lexopt::Arg::*;
    let mut index: Option<u64> = None;

    while let Some(arg) = parser.next().map_err(|e| MsigError::Usage(e.to_string()))? {
        match arg {
            Value(val) if index.is_none() => {
                let s = val
                    .into_string()
                    .map_err(|_| MsigError::Usage("invalid UTF-8".into()))?;
                index = Some(
                    s.parse()
                        .map_err(|_| MsigError::Usage(format!("invalid index: '{s}'")))?,
                );
            }
            Short('h') | Long("help") => {
                super::help::print_resource_help("tx");
                return Ok(());
            }
            _ => return Err(MsigError::Usage("unexpected argument".into())),
        }
    }

    let index = index.ok_or_else(|| MsigError::Usage("transaction index is required".into()))?;

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

    let detail = inspect::get_proposal_detail(&rpc, &multisig_pubkey, index, &cfg.program_id)?;

    match output_mode {
        OutputMode::Json => json::print_json(&detail),
        OutputMode::Text => {
            println!("Transaction #{index}");
            println!("  Type: {}", detail.transaction_type.label());
            println!("  Status: {}", detail.summary.status.label());
            if let Some(ref vtx) = detail.vault_tx {
                println!("  Creator: {}", vtx.creator);
                println!("  Vault index: {}", vtx.vault_index);
                println!("  Instructions: {}", vtx.instruction_count);
                for (i, ix) in vtx.instructions.iter().enumerate() {
                    println!("    [{i}] {} ({})", ix.program_name, ix.program_id);
                    println!("        data: {}", ix.data_hex);
                }
            }
            if let Some(ref ctx) = detail.config_tx {
                println!("  Creator: {}", ctx.creator);
                println!("  Actions: {}", ctx.action_count);
            }
        }
    }
    Ok(())
}

fn cmd_list(globals: GlobalOpts, parser: &mut lexopt::Parser) -> Result<(), MsigError> {
    use lexopt::Arg::*;
    let mut limit: u64 = 10;
    while let Some(arg) = parser.next().map_err(|e| MsigError::Usage(e.to_string()))? {
        match arg {
            Long("limit") => {
                let v = super::parse_value(parser, "--limit")?;
                limit = v
                    .parse()
                    .map_err(|_| MsigError::Usage(format!("invalid limit: '{v}'")))?;
            }
            Short('h') | Long("help") => {
                super::help::print_resource_help("tx");
                return Ok(());
            }
            _ => return Err(MsigError::Usage("unexpected flag".into())),
        }
    }
    let cfg = super::load_config_only(&globals)?;
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

    let proposals = inspect::list_proposals(&rpc, &multisig_pubkey, limit, &cfg.program_id)?;

    match output_mode {
        OutputMode::Json => json::print_json(&proposals),
        OutputMode::Text => {
            if proposals.is_empty() {
                println!("No proposals found.");
                return Ok(());
            }
            let headers = &["#", "Status", "Votes", "Time", "Address"];
            let rows: Vec<Vec<String>> = proposals
                .iter()
                .map(|p| {
                    let time = p
                        .status
                        .timestamp()
                        .map(crate::output::table::format_relative_time)
                        .unwrap_or_default();
                    vec![
                        format!("{}", p.index),
                        crate::output::table::format_status(p.status.label()),
                        format!("{}/{}", p.approved_count, p.threshold),
                        time,
                        p.address.to_string(),
                    ]
                })
                .collect();
            crate::output::table::print_table(headers, &rows);
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// tx create
// ---------------------------------------------------------------------------

fn cmd_create(globals: GlobalOpts, parser: &mut lexopt::Parser) -> Result<(), MsigError> {
    use lexopt::Arg::*;

    let mut program: Option<String> = None;
    let mut accounts: Vec<String> = Vec::new();
    let mut data: Vec<u8> = Vec::new();
    let mut vault_message: Option<Vec<u8>> = None;
    let mut description: Option<String> = None;
    let mut vault_index_override: Option<u8> = None;

    while let Some(arg) = parser.next().map_err(|e| MsigError::Usage(e.to_string()))? {
        match arg {
            Long("program") => program = Some(parse_value(parser, "--program")?),
            Long("account") => accounts.push(parse_value(parser, "--account")?),
            Long("data") => data = parse_create_data(&parse_value(parser, "--data")?)?,
            Long("vault-message") => {
                vault_message = Some(parse_vault_message(&parse_value(
                    parser,
                    "--vault-message",
                )?)?)
            }
            Long("vault-index") => {
                let value = parse_value(parser, "--vault-index")?;
                vault_index_override =
                    Some(value.parse().map_err(|_| {
                        MsigError::Usage(format!("invalid vault-index: '{value}'"))
                    })?);
            }
            Long("description") => description = Some(parse_value(parser, "--description")?),
            Short('h') | Long("help") => {
                super::help::print_resource_help("tx");
                return Ok(());
            }
            _ => {
                return Err(MsigError::Usage(
                    "unexpected argument for 'tx create'. Run 'msig tx --help'.".into(),
                ));
            }
        }
    }

    let output_mode = crate::output::detect_output_mode(globals.output.as_deref());
    let is_json = matches!(output_mode, OutputMode::Json);

    let ctx = build_context(&globals)?;
    let multisig = ctx.resolve_multisig(None)?;
    let vault_index = ctx.vault_index(vault_index_override.or(globals.vault_index));
    let (vault, _) = crate::infra::pda::vault_pda(&multisig, vault_index, &ctx.config.program_id);

    if let Some(message) = vault_message {
        if program.is_some() || !accounts.is_empty() || !data.is_empty() {
            return Err(MsigError::Usage(
                "--vault-message cannot be combined with --program, --account, or --data".into(),
            ));
        }
        preview_vault_message(vault_index, &vault, message.len());
        let description = description.unwrap_or_else(|| "Create vault transaction message".into());
        let result = vault_tx::create_vault_transaction_message_proposal(
            ctx.rpc(),
            ctx.signer.as_ref(),
            &multisig,
            message,
            vault_index,
            description,
            &ctx.config,
            globals.dry_run,
            globals.yes,
            is_json,
        )?;

        if is_json {
            json::print_json(&serde_json::json!({
                "action": "tx_create_vault_message",
                "transaction_index": result.transaction_index,
                "vault_index": vault_index,
                "vault": result.vault.to_string(),
                "transaction": result.transaction.to_string(),
                "proposal": result.proposal.to_string(),
                "signature": result.signature,
            }));
        }

        return Ok(());
    }

    let program = program.ok_or_else(|| {
        MsigError::Usage("--program is required unless --vault-message is provided".into())
    })?;
    let program_id = resolve_create_pubkey(
        &program,
        &vault,
        &multisig,
        &ctx.config.program_id,
        &ctx.config.labels,
    )?;

    let mut metas = Vec::with_capacity(accounts.len());
    for account in &accounts {
        metas.push(parse_create_account(
            account,
            &vault,
            &multisig,
            &ctx.config.program_id,
            &ctx.config.labels,
        )?);
    }

    for meta in &metas {
        if meta.is_signer && meta.pubkey != vault {
            return Err(MsigError::Usage(format!(
                "custom vault instructions can only mark the active vault as signer; {} is not the vault",
                meta.pubkey
            )));
        }
    }

    let instruction = Instruction {
        program_id,
        accounts: metas,
        data,
    };
    preview_create_instruction(vault_index, &vault, &instruction);

    let description =
        description.unwrap_or_else(|| format!("Create custom vault instruction for {program_id}"));
    let result = vault_tx::create_vault_transaction_proposal(
        ctx.rpc(),
        ctx.signer.as_ref(),
        &multisig,
        vec![instruction],
        vault_index,
        description,
        &ctx.config,
        globals.dry_run,
        globals.yes,
        is_json,
    )?;

    if is_json {
        json::print_json(&serde_json::json!({
            "action": "tx_create",
            "program": program_id.to_string(),
            "transaction_index": result.transaction_index,
            "vault_index": vault_index,
            "vault": result.vault.to_string(),
            "transaction": result.transaction.to_string(),
            "proposal": result.proposal.to_string(),
            "signature": result.signature,
        }));
    }

    Ok(())
}

fn parse_create_account(
    spec: &str,
    vault: &Pubkey,
    multisig: &Pubkey,
    squads_program_id: &Pubkey,
    labels: &std::collections::HashMap<String, String>,
) -> Result<AccountMeta, MsigError> {
    let mut parts = spec.split(':');
    let address = parts
        .next()
        .ok_or_else(|| MsigError::Usage("account spec cannot be empty".into()))?;
    if address.trim().is_empty() {
        return Err(MsigError::Usage("account address cannot be empty".into()));
    }

    let mut writable = false;
    let mut signer = false;
    for flag in parts {
        match flag.to_ascii_lowercase().as_str() {
            "writable" | "write" | "w" | "mut" => writable = true,
            "readonly" | "read" | "r" => writable = false,
            "signer" | "sign" | "s" => signer = true,
            "nonsigner" | "nosigner" | "no-signer" => signer = false,
            "" => {
                return Err(MsigError::Usage(format!(
                    "account spec '{spec}' contains an empty flag"
                )));
            }
            other => {
                return Err(MsigError::Usage(format!(
                    "unknown account flag '{other}'. Use writable, readonly, signer, or nonsigner."
                )));
            }
        }
    }

    let pubkey = resolve_create_pubkey(address, vault, multisig, squads_program_id, labels)?;
    Ok(if writable {
        AccountMeta::new(pubkey, signer)
    } else {
        AccountMeta::new_readonly(pubkey, signer)
    })
}

fn resolve_create_pubkey(
    value: &str,
    vault: &Pubkey,
    multisig: &Pubkey,
    squads_program_id: &Pubkey,
    labels: &std::collections::HashMap<String, String>,
) -> Result<Pubkey, MsigError> {
    match value {
        "vault" => Ok(*vault),
        "multisig" => Ok(*multisig),
        "program_id" | "squads_program" | "squads_program_id" => Ok(*squads_program_id),
        _ => {
            let resolved = crate::infra::config::labels::resolve_address(value, labels)
                .map_err(|_| MsigError::Usage(format!("invalid pubkey or label: '{value}'")))?;
            resolved
                .parse()
                .map_err(|_| MsigError::Usage(format!("invalid pubkey: '{resolved}'")))
        }
    }
}

fn parse_create_data(value: &str) -> Result<Vec<u8>, MsigError> {
    if let Some(rest) = value.strip_prefix("base64:") {
        use base64::Engine as _;
        return base64::engine::general_purpose::STANDARD
            .decode(rest)
            .map_err(|e| MsigError::Usage(format!("invalid base64 data: {e}")));
    }
    if let Some(rest) = value.strip_prefix("utf8:") {
        return Ok(rest.as_bytes().to_vec());
    }
    parse_create_hex(value)
}

fn parse_vault_message(value: &str) -> Result<Vec<u8>, MsigError> {
    let trimmed = value.trim();
    if trimmed.starts_with('[') {
        let bytes: Vec<u8> = serde_json::from_str(trimmed).map_err(|e| {
            MsigError::Usage(format!(
                "invalid transaction message byte array; expected JSON like [1,2,3]: {e}"
            ))
        })?;
        if bytes.is_empty() {
            return Err(MsigError::Usage(
                "transaction message byte array cannot be empty".into(),
            ));
        }
        return Ok(bytes);
    }
    if let Some(rest) = trimmed.strip_prefix("base64:") {
        use base64::Engine as _;
        return base64::engine::general_purpose::STANDARD
            .decode(rest)
            .map_err(|e| MsigError::Usage(format!("invalid base64 vault message: {e}")));
    }
    parse_create_hex(trimmed)
}

fn parse_create_hex(value: &str) -> Result<Vec<u8>, MsigError> {
    let trimmed = value
        .trim()
        .strip_prefix("0x")
        .or_else(|| value.trim().strip_prefix("0X"))
        .unwrap_or(value.trim());
    let cleaned: String = trimmed
        .chars()
        .filter(|ch| !ch.is_ascii_whitespace() && *ch != '_')
        .collect();

    if !cleaned.len().is_multiple_of(2) {
        return Err(MsigError::Usage(
            "hex data must contain an even number of digits".into(),
        ));
    }

    let mut bytes = Vec::with_capacity(cleaned.len() / 2);
    for idx in (0..cleaned.len()).step_by(2) {
        bytes.push(
            u8::from_str_radix(&cleaned[idx..idx + 2], 16)
                .map_err(|_| MsigError::Usage("hex data contains invalid characters".into()))?,
        );
    }
    Ok(bytes)
}

fn preview_create_instruction(vault_index: u8, vault: &Pubkey, instruction: &Instruction) {
    eprintln!();
    eprintln!("Custom vault instruction");
    eprintln!("  Vault index: {vault_index}");
    eprintln!("  Vault:       {vault}");
    eprintln!("  Program:     {}", instruction.program_id);
    eprintln!("  Accounts:    {}", instruction.accounts.len());
    for account in &instruction.accounts {
        let mut flags = Vec::new();
        if account.is_signer {
            flags.push("signer");
        }
        if account.is_writable {
            flags.push("writable");
        }
        if flags.is_empty() {
            eprintln!("    {}", account.pubkey);
        } else {
            eprintln!("    {} ({})", account.pubkey, flags.join(", "));
        }
    }
    eprintln!("  Data bytes:  {}", instruction.data.len());
}

fn preview_vault_message(vault_index: u8, vault: &Pubkey, message_len: usize) {
    eprintln!();
    eprintln!("Vault transaction message");
    eprintln!("  Vault index:   {vault_index}");
    eprintln!("  Vault:         {vault}");
    eprintln!("  Message bytes: {message_len}");
}

// ---------------------------------------------------------------------------
// tx export
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OfflineAction {
    Approve,
    Reject,
    Cancel,
    Execute,
}

impl OfflineAction {
    fn parse(value: &str) -> Result<Self, MsigError> {
        match value {
            "approve" => Ok(Self::Approve),
            "reject" => Ok(Self::Reject),
            "cancel" => Ok(Self::Cancel),
            "execute" => Ok(Self::Execute),
            _ => Err(MsigError::Usage(format!(
                "invalid offline action '{value}'. Expected approve, reject, cancel, or execute."
            ))),
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Approve => "approve",
            Self::Reject => "reject",
            Self::Cancel => "cancel",
            Self::Execute => "execute",
        }
    }

    fn decoded_name(self) -> &'static str {
        match self {
            Self::Approve => "proposal_approve",
            Self::Reject => "proposal_reject",
            Self::Cancel => "proposal_cancel",
            Self::Execute => "proposal_execute",
        }
    }

    fn vote(self) -> Option<crate::domain::proposal::Vote> {
        match self {
            Self::Approve => Some(crate::domain::proposal::Vote::Approve),
            Self::Reject => Some(crate::domain::proposal::Vote::Reject),
            Self::Cancel => Some(crate::domain::proposal::Vote::Cancel),
            Self::Execute => None,
        }
    }
}

fn cmd_export(globals: GlobalOpts, parser: &mut lexopt::Parser) -> Result<(), MsigError> {
    use lexopt::Arg::*;

    let mut index: Option<u64> = None;
    let mut file_path: Option<String> = None;
    let mut action = OfflineAction::Approve;

    while let Some(arg) = parser.next().map_err(|e| MsigError::Usage(e.to_string()))? {
        match arg {
            Value(val) if index.is_none() => {
                let raw = val
                    .into_string()
                    .map_err(|_| MsigError::Usage("invalid UTF-8".into()))?;
                index =
                    Some(raw.parse().map_err(|_| {
                        MsigError::Usage(format!("invalid proposal index: '{raw}'"))
                    })?);
            }
            Long("file") => {
                file_path = Some(parse_value(parser, "--file")?);
            }
            Long("action") => {
                action = OfflineAction::parse(&parse_value(parser, "--action")?)?;
            }
            Short('h') | Long("help") => {
                println!("Usage: msig tx export <INDEX> [--action approve|reject|cancel|execute] [--file FILE]");
                println!();
                println!("Export a signable Solana transaction to a .sqds file.");
                println!();
                println!("Options:");
                println!("  --action <ACTION>  Action to sign (default: approve)");
                println!("  --file <FILE>  Output file path (default: proposal-<INDEX>.sqds)");
                return Ok(());
            }
            _ => {
                return Err(MsigError::Usage(
                    "unexpected argument for 'tx export'. Run 'msig tx export --help'.".into(),
                ));
            }
        }
    }

    let index = index.ok_or_else(|| MsigError::Usage("proposal index is required".into()))?;

    let cfg = load_config_only(&globals)?;
    let signer = crate::infra::signer::resolve_signer(
        globals.ledger.as_deref(),
        globals.keypair.as_deref(),
        cfg.keypair.as_deref(),
    )?;
    let member = signer.pubkey();

    let multisig_addr_str = globals
        .multisig
        .as_deref()
        .or(cfg.multisig.as_deref())
        .ok_or_else(|| {
            MsigError::Config(
                "no multisig address specified. Use --multisig <ADDRESS> or set it in config."
                    .into(),
            )
        })?;
    let resolved = crate::infra::config::labels::resolve_address(multisig_addr_str, &cfg.labels)?;

    let rpc = crate::infra::rpc::SolanaRpcClient::with_commitment(&cfg.cluster, &cfg.commitment);

    let ms_pubkey: Pubkey = resolved
        .parse()
        .map_err(|_| MsigError::Config(format!("invalid multisig address: '{resolved}'")))?;

    let export = build_offline_export(&rpc, &cfg, &ms_pubkey, index, member, action)?;

    let params = offline::ExportParams {
        version: 1,
        exporter_pubkey: member,
        exporter_label: None,
        multisig_address: ms_pubkey,
        threshold: export.threshold,
        members: export.members,
        create_key: export.create_key,
        transaction_index: index,
        vault_index: export.vault_index,
        message_bytes: export.message_bytes,
        decoded_instructions: export.decoded_instructions,
        existing_signatures: vec![],
        cluster: cfg.cluster.clone(),
    };

    let file = offline::export_transaction(&params, signer.as_ref())?;

    let output_path =
        file_path.unwrap_or_else(|| format!("proposal-{index}-{}.sqds", action.label()));
    let output_path = std::path::Path::new(&output_path);

    offline::write_offline_file(&file, output_path)?;

    println!(
        "Exported proposal #{index} {} transaction to {}",
        action.label(),
        output_path.display()
    );
    println!("  Multisig:  {ms_pubkey}");
    println!("  Signer:    {member}");
    println!("  Blockhash: {}", export.recent_blockhash);
    eprintln!("  Warning: recent blockhashes expire quickly; sign and push promptly.");

    Ok(())
}

struct OfflineExportBuild {
    threshold: u16,
    members: Vec<(Pubkey, u8)>,
    create_key: [u8; 32],
    vault_index: u8,
    message_bytes: Vec<u8>,
    decoded_instructions: Vec<offline::DecodedInstruction>,
    recent_blockhash: String,
}

fn build_offline_export(
    rpc: &crate::infra::rpc::SolanaRpcClient,
    cfg: &crate::infra::config::Config,
    multisig: &Pubkey,
    index: u64,
    member: Pubkey,
    action: OfflineAction,
) -> Result<OfflineExportBuild, MsigError> {
    let ms_addr = multisig.to_string();
    let (proposal_addr, _) = crate::infra::pda::proposal_pda(multisig, index, &cfg.program_id);
    let (tx_addr, _) = crate::infra::pda::transaction_pda(multisig, index, &cfg.program_id);
    let proposal_addr_str = proposal_addr.to_string();
    let tx_addr_str = tx_addr.to_string();
    let accounts = rpc.get_multiple_accounts(&[&ms_addr, &proposal_addr_str, &tx_addr_str])?;

    let ms_account = accounts[0]
        .as_ref()
        .ok_or_else(|| MsigError::MultisigNotFound(ms_addr.clone()))?;
    let ms = crate::infra::accounts::multisig::MultisigAccount::parse(&ms_account.data)?;
    let proposal_account = accounts[1]
        .as_ref()
        .ok_or(MsigError::ProposalNotFound(index))?;
    let proposal =
        crate::infra::accounts::proposal::ProposalAccount::parse(&proposal_account.data)?;
    let tx_account = accounts[2].as_ref().ok_or_else(|| {
        MsigError::Transaction(format!(
            "transaction account not found for proposal #{index}"
        ))
    })?;

    let signer_member = ms.members.iter().find(|candidate| candidate.key == member);
    match signer_member {
        Some(m) if action == OfflineAction::Execute && !m.permissions.can_execute() => {
            return Err(MsigError::PermissionDenied(
                "signer does not have Execute permission".into(),
            ));
        }
        Some(m) if action != OfflineAction::Execute && !m.permissions.can_vote() => {
            return Err(MsigError::PermissionDenied(
                "signer does not have Vote permission".into(),
            ));
        }
        None => {
            return Err(MsigError::PermissionDenied(
                "signer is not a member of this multisig".into(),
            ));
        }
        _ => {}
    }

    if matches!(action, OfflineAction::Approve | OfflineAction::Reject)
        && index <= ms.stale_transaction_index
    {
        return Err(MsigError::Transaction(format!(
            "proposal #{index} is stale and cannot be {}",
            action.label()
        )));
    }

    let instruction = match action {
        OfflineAction::Approve | OfflineAction::Reject | OfflineAction::Cancel => {
            build_vote_instruction(
                multisig,
                proposal_addr,
                member,
                index,
                action,
                &proposal,
                cfg,
            )?
        }
        OfflineAction::Execute => build_execute_instruction(
            multisig,
            proposal_addr,
            tx_addr,
            tx_account,
            member,
            index,
            &proposal,
            cfg,
        )?,
    };

    let (recent_blockhash, _) = rpc.get_latest_blockhash()?;
    let message_bytes = crate::application::pipeline::compile_legacy_message(
        std::slice::from_ref(&instruction),
        &member,
        &recent_blockhash,
    )?;
    let summary = offline::message_summary(&message_bytes)?;
    let vault_index =
        crate::infra::accounts::vault_tx::VaultTransactionAccount::parse(&tx_account.data)
            .map(|vtx| vtx.vault_index)
            .unwrap_or(cfg.vault_index);

    Ok(OfflineExportBuild {
        threshold: ms.threshold,
        members: ms
            .members
            .iter()
            .map(|m| (m.key, m.permissions.0))
            .collect(),
        create_key: ms.create_key.to_bytes(),
        vault_index,
        message_bytes,
        decoded_instructions: vec![offline::DecodedInstruction {
            program_id: cfg.program_id.to_string(),
            accounts_count: instruction.accounts.len(),
            data_len: instruction.data.len(),
            decoded_name: Some(action.decoded_name().to_string()),
        }],
        recent_blockhash: summary.recent_blockhash,
    })
}

fn build_vote_instruction(
    multisig: &Pubkey,
    proposal: Pubkey,
    member: Pubkey,
    index: u64,
    action: OfflineAction,
    proposal_account: &crate::infra::accounts::proposal::ProposalAccount,
    cfg: &crate::infra::config::Config,
) -> Result<Instruction, MsigError> {
    let vote = action
        .vote()
        .ok_or_else(|| MsigError::Transaction("execute is not a vote action".into()))?;

    match action {
        OfflineAction::Approve | OfflineAction::Reject if !proposal_account.status.is_active() => {
            return Err(MsigError::Transaction(format!(
                "proposal #{index} is '{}', must be 'Active' to {}",
                proposal_account.status.label(),
                action.label()
            )));
        }
        OfflineAction::Cancel if !proposal_account.status.is_approved() => {
            return Err(MsigError::Transaction(format!(
                "proposal #{index} is '{}', must be Approved to cancel",
                proposal_account.status.label()
            )));
        }
        _ => {}
    }

    if action == OfflineAction::Approve && proposal_account.approved.contains(&member) {
        return Err(MsigError::Transaction(format!(
            "signer has already approved proposal #{index}"
        )));
    }
    if action == OfflineAction::Reject && proposal_account.rejected.contains(&member) {
        return Err(MsigError::Transaction(format!(
            "signer has already rejected proposal #{index}"
        )));
    }
    if action == OfflineAction::Cancel && proposal_account.cancelled.contains(&member) {
        return Err(MsigError::Transaction(format!(
            "signer has already cancelled proposal #{index}"
        )));
    }

    let mut data = Vec::with_capacity(9);
    data.extend_from_slice(&vote.discriminator());
    data.push(0x00);

    Ok(Instruction {
        program_id: cfg.program_id,
        accounts: vec![
            AccountMeta::new_readonly(*multisig, false),
            AccountMeta::new(member, true),
            AccountMeta::new(proposal, false),
        ],
        data,
    })
}

#[allow(clippy::too_many_arguments)]
fn build_execute_instruction(
    multisig: &Pubkey,
    proposal: Pubkey,
    transaction: Pubkey,
    tx_account: &crate::infra::rpc::AccountData,
    executor: Pubkey,
    index: u64,
    proposal_account: &crate::infra::accounts::proposal::ProposalAccount,
    cfg: &crate::infra::config::Config,
) -> Result<Instruction, MsigError> {
    if !proposal_account.status.is_approved() {
        return Err(MsigError::Transaction(format!(
            "proposal #{index} is '{}', must be Approved to execute",
            proposal_account.status.label()
        )));
    }

    let is_vault_tx =
        crate::infra::accounts::vault_tx::VaultTransactionAccount::parse(&tx_account.data).is_ok();
    let (data, accounts) = if is_vault_tx {
        let data = vec![0xc2, 0x08, 0xa1, 0x57, 0x99, 0xa4, 0x19, 0xab];
        let mut accts = vec![
            AccountMeta::new_readonly(*multisig, false),
            AccountMeta::new(proposal, false),
            AccountMeta::new_readonly(transaction, false),
            AccountMeta::new_readonly(executor, true),
        ];
        let vtx =
            crate::infra::accounts::vault_tx::VaultTransactionAccount::parse(&tx_account.data)?;
        let num_signers = vtx.message.num_signers as usize;
        let num_writable_signers = vtx.message.num_writable_signers as usize;
        let num_writable_non_signers = vtx.message.num_writable_non_signers as usize;
        for (i, key_bytes) in vtx.message.account_keys.iter().enumerate() {
            let key = Pubkey::from(*key_bytes);
            let is_writable = if i < num_signers {
                i < num_writable_signers
            } else {
                (i - num_signers) < num_writable_non_signers
            };
            if is_writable {
                accts.push(AccountMeta::new(key, false));
            } else {
                accts.push(AccountMeta::new_readonly(key, false));
            }
        }
        (data, accts)
    } else {
        let data = vec![0x72, 0x92, 0xf4, 0xbd, 0xfc, 0x8c, 0x24, 0x28];
        let system_program = solana_pubkey::pubkey!("11111111111111111111111111111111");
        let accts = vec![
            AccountMeta::new(*multisig, false),
            AccountMeta::new_readonly(executor, true),
            AccountMeta::new(proposal, false),
            AccountMeta::new_readonly(transaction, false),
            AccountMeta::new(executor, true),
            AccountMeta::new_readonly(system_program, false),
        ];
        (data, accts)
    };

    Ok(Instruction {
        program_id: cfg.program_id,
        accounts,
        data,
    })
}

// ---------------------------------------------------------------------------
// tx status / combine
// ---------------------------------------------------------------------------

fn cmd_status(globals: GlobalOpts, parser: &mut lexopt::Parser) -> Result<(), MsigError> {
    use lexopt::Arg::*;

    let mut file_path: Option<String> = None;
    while let Some(arg) = parser.next().map_err(|e| MsigError::Usage(e.to_string()))? {
        match arg {
            Value(val) if file_path.is_none() => {
                file_path = Some(
                    val.into_string()
                        .map_err(|_| MsigError::Usage("invalid UTF-8".into()))?,
                );
            }
            Short('h') | Long("help") => {
                println!("Usage: msig tx status <FILE>");
                println!();
                println!("Verify and inspect a .sqds offline transaction file.");
                return Ok(());
            }
            _ => {
                return Err(MsigError::Usage(
                    "unexpected argument for 'tx status'. Run 'msig tx status --help'.".into(),
                ));
            }
        }
    }

    let file_path = file_path.ok_or_else(|| {
        MsigError::Usage("file path is required. Usage: msig tx status <FILE>".into())
    })?;
    let cfg = load_config_only(&globals)?;
    let output_mode = crate::output::detect_output_mode(globals.output.as_deref());
    let path = std::path::Path::new(&file_path);
    let (file, _message_bytes, message_summary) = read_verified_offline_file(path, &cfg)?;

    match output_mode {
        OutputMode::Json => {
            json::print_json(&serde_json::json!({
                "file": file,
                "message": {
                    "recent_blockhash": message_summary.recent_blockhash.clone(),
                    "instruction_count": message_summary.instruction_count,
                    "required_signers": message_summary
                        .required_signers
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>(),
                    "decoded_instructions": message_summary.decoded_instructions.clone(),
                }
            }));
        }
        OutputMode::Text => {
            println!(
                "Envelope signature: VALID (signed by {})",
                file.exporter.pubkey
            );
            display_offline_details(&file, &message_summary);
        }
    }
    Ok(())
}

fn cmd_combine(globals: GlobalOpts, parser: &mut lexopt::Parser) -> Result<(), MsigError> {
    use lexopt::Arg::*;

    let mut out_path: Option<String> = None;
    let mut input_paths = Vec::new();

    while let Some(arg) = parser.next().map_err(|e| MsigError::Usage(e.to_string()))? {
        match arg {
            Long("out") => out_path = Some(parse_value(parser, "--out")?),
            Value(val) => {
                input_paths.push(
                    val.into_string()
                        .map_err(|_| MsigError::Usage("invalid UTF-8".into()))?,
                );
            }
            Short('h') | Long("help") => {
                println!("Usage: msig tx combine --out <FILE> <SIGNED.sqds>...");
                println!();
                println!("Merge signatures from matching .sqds files.");
                return Ok(());
            }
            _ => {
                return Err(MsigError::Usage(
                    "unexpected argument for 'tx combine'. Run 'msig tx combine --help'.".into(),
                ));
            }
        }
    }

    let out_path = out_path.ok_or_else(|| MsigError::Usage("--out <FILE> is required".into()))?;
    if input_paths.is_empty() {
        return Err(MsigError::Usage(
            "at least one .sqds input file is required".into(),
        ));
    }

    let cfg = load_config_only(&globals)?;
    let output_mode = crate::output::detect_output_mode(globals.output.as_deref());
    let mut combined: Option<offline::OfflineFile> = None;

    for input_path in &input_paths {
        let path = std::path::Path::new(input_path);
        let (mut file, _message_bytes, _summary) = read_verified_offline_file(path, &cfg)?;

        if let Some(base) = &mut combined {
            ensure_same_offline_transaction(base, &file)?;
            for signature in file.transaction.existing_signatures.drain(..) {
                merge_existing_signature(base, signature)?;
            }
        } else {
            let mut first = file.clone();
            let signatures = std::mem::take(&mut first.transaction.existing_signatures);
            for signature in signatures {
                merge_existing_signature(&mut first, signature)?;
            }
            combined = Some(first);
        }
    }

    let Some(combined) = combined else {
        return Err(MsigError::Usage(
            "at least one .sqds input file is required".into(),
        ));
    };
    let signature_count = combined.transaction.existing_signatures.len();
    let output_path = std::path::Path::new(&out_path);
    offline::write_offline_file(&combined, output_path)?;

    match output_mode {
        OutputMode::Json => json::print_json(&serde_json::json!({
            "action": "tx_combine",
            "output": out_path,
            "signature_count": signature_count,
            "transaction_index": combined.transaction.index,
            "message_hash": combined.transaction.message_hash,
        })),
        OutputMode::Text => {
            println!(
                "Wrote {} with {} verified signature(s).",
                output_path.display(),
                signature_count
            );
        }
    }

    Ok(())
}

fn read_verified_offline_file(
    path: &std::path::Path,
    cfg: &crate::infra::config::Config,
) -> Result<(offline::OfflineFile, Vec<u8>, offline::MessageSummary), MsigError> {
    let file = offline::read_offline_file(path)?;
    offline::verify_envelope(&file)?;
    if !file.cluster.is_empty() {
        offline::validate_cluster_match(&file.cluster, &cfg.cluster)?;
    }
    let message_bytes = offline::decode_message(&file)?;
    let message_summary = offline::message_summary(&message_bytes)?;
    Ok((file, message_bytes, message_summary))
}

fn ensure_same_offline_transaction(
    base: &offline::OfflineFile,
    candidate: &offline::OfflineFile,
) -> Result<(), MsigError> {
    let same = base.version == candidate.version
        && base.format == candidate.format
        && base.cluster == candidate.cluster
        && base.multisig.address == candidate.multisig.address
        && base.multisig.threshold == candidate.multisig.threshold
        && base.multisig.config_state_hash == candidate.multisig.config_state_hash
        && base.multisig.create_key == candidate.multisig.create_key
        && base.transaction.index == candidate.transaction.index
        && base.transaction.vault_index == candidate.transaction.vault_index
        && base.transaction.message_b64 == candidate.transaction.message_b64
        && base.transaction.message_hash == candidate.transaction.message_hash;

    if same {
        Ok(())
    } else {
        Err(MsigError::Signing(
            "cannot combine .sqds files: transaction metadata or message bytes differ".into(),
        ))
    }
}

fn merge_existing_signature(
    file: &mut offline::OfflineFile,
    signature: offline::ExistingSignature,
) -> Result<(), MsigError> {
    let signer: Pubkey = signature.signer.parse().map_err(|_| {
        MsigError::Signing(format!("invalid signature signer {}", signature.signer))
    })?;
    let signature_bytes = bs58::decode(&signature.signature)
        .into_vec()
        .map_err(|e| MsigError::Signing(format!("invalid signature for {signer}: {e}")))?;
    offline::add_verified_signature(file, signer, &signature_bytes)
}

fn display_offline_details(file: &offline::OfflineFile, message_summary: &offline::MessageSummary) {
    println!();
    println!("=== Transaction Details ===");
    println!("  Multisig:     {}", file.multisig.address);
    println!(
        "  Threshold:    {}/{}",
        file.multisig.threshold,
        file.multisig.members.len()
    );
    println!("  Tx Index:     {}", file.transaction.index);
    println!("  Vault Index:  {}", file.transaction.vault_index);
    if !file.cluster.is_empty() {
        println!("  Cluster:      {}", file.cluster);
    }
    println!("  Blockhash:    {}", message_summary.recent_blockhash);
    println!("  Message Hash: {}", file.transaction.message_hash);
    println!("  Instructions: {}", message_summary.instruction_count);
    println!(
        "  Required Signers: {}",
        message_summary.required_signers.len()
    );
    for signer in &message_summary.required_signers {
        println!("    {signer}");
    }

    if !message_summary.decoded_instructions.is_empty() {
        println!();
        println!("  Instructions:");
        for (i, ix) in message_summary.decoded_instructions.iter().enumerate() {
            println!(
                "    #{}: program {}, {} accounts, {} bytes data",
                i + 1,
                crate::sanitize::sanitize_display(&ix.program_id),
                ix.accounts_count,
                ix.data_len,
            );
        }
    }

    if !file.transaction.existing_signatures.is_empty() {
        println!();
        println!("  Existing signatures:");
        for sig in &file.transaction.existing_signatures {
            let sig_preview = if sig.signature.len() > 16 {
                &sig.signature[..16]
            } else {
                &sig.signature
            };
            println!("    {}: {}...", sig.signer, sig_preview);
        }
    }
    println!("===========================");
}

// ---------------------------------------------------------------------------
// tx import
// ---------------------------------------------------------------------------

fn cmd_import(globals: GlobalOpts, parser: &mut lexopt::Parser) -> Result<(), MsigError> {
    use lexopt::Arg::*;

    let mut file_path: Option<String> = None;
    let mut do_sign = false;
    let mut do_push = false;

    while let Some(arg) = parser.next().map_err(|e| MsigError::Usage(e.to_string()))? {
        match arg {
            Value(val) if file_path.is_none() => {
                file_path = Some(
                    val.into_string()
                        .map_err(|_| MsigError::Usage("invalid UTF-8".into()))?,
                );
            }
            Long("sign") => {
                do_sign = true;
            }
            Long("push") => {
                do_push = true;
            }
            Short('h') | Long("help") => {
                println!("Usage: msig tx import <FILE> [--sign] [--push]");
                println!();
                println!("Import a .sqds offline transaction file.");
                println!();
                println!("Options:");
                println!("  --sign   Sign the transaction with your key");
                println!("  --push   Submit the signed transaction to the network");
                return Ok(());
            }
            _ => {
                return Err(MsigError::Usage(
                    "unexpected argument for 'tx import'. Run 'msig tx import --help'.".into(),
                ));
            }
        }
    }

    let file_path = file_path.ok_or_else(|| {
        MsigError::Usage("file path is required. Usage: msig tx import <FILE>".into())
    })?;

    let cfg = load_config_only(&globals)?;
    let path = std::path::Path::new(&file_path);
    let mut file = offline::read_offline_file(path)?;

    offline::verify_envelope(&file)?;
    if !file.cluster.is_empty() {
        offline::validate_cluster_match(&file.cluster, &cfg.cluster)?;
    }
    eprintln!(
        "Envelope signature: VALID (signed by {})",
        file.exporter.pubkey
    );

    let message_bytes = offline::decode_message(&file)?;
    let message_summary = offline::message_summary(&message_bytes)?;

    // Display transaction details
    eprintln!();
    eprintln!("=== Transaction Details ===");
    eprintln!("  Multisig:     {}", file.multisig.address);
    eprintln!(
        "  Threshold:    {}/{}",
        file.multisig.threshold,
        file.multisig.members.len()
    );
    eprintln!("  Tx Index:     {}", file.transaction.index);
    eprintln!("  Vault Index:  {}", file.transaction.vault_index);
    if !file.cluster.is_empty() {
        eprintln!("  Cluster:      {}", file.cluster);
    }
    eprintln!("  Blockhash:    {}", message_summary.recent_blockhash);
    eprintln!("  Message Hash: {}", file.transaction.message_hash);
    eprintln!("  Instructions: {}", message_summary.instruction_count);
    eprintln!(
        "  Required Signers: {}",
        message_summary.required_signers.len()
    );
    for signer in &message_summary.required_signers {
        eprintln!("    {signer}");
    }

    if !message_summary.decoded_instructions.is_empty() {
        eprintln!();
        eprintln!("  Instructions:");
        for (i, ix) in message_summary.decoded_instructions.iter().enumerate() {
            eprintln!(
                "    #{}: program {}, {} accounts, {} bytes data",
                i + 1,
                crate::sanitize::sanitize_display(&ix.program_id),
                ix.accounts_count,
                ix.data_len,
            );
        }
    }

    if !file.transaction.existing_signatures.is_empty() {
        eprintln!();
        eprintln!("  Existing signatures:");
        for sig in &file.transaction.existing_signatures {
            let sig_preview = if sig.signature.len() > 16 {
                &sig.signature[..16]
            } else {
                &sig.signature
            };
            eprintln!("    {}: {}...", sig.signer, sig_preview);
        }
    }
    eprintln!("===========================");

    if do_sign {
        let signer = crate::infra::signer::resolve_signer(
            globals.ledger.as_deref(),
            globals.keypair.as_deref(),
            cfg.keypair.as_deref(),
        )?;

        let sig_bytes = signer.sign(&message_bytes)?;
        offline::add_verified_signature(&mut file, signer.pubkey(), &sig_bytes)?;

        println!();
        println!("Signed by: {}", signer.pubkey());
        println!("Signature: {}", bs58::encode(sig_bytes).into_string());

        offline::write_offline_file(&file, path)?;
        println!("Updated {file_path} with new signature.");
    }

    if do_push {
        use base64::Engine as _;
        let wire_tx = offline::build_signed_transaction(&file)?;
        let tx_b64 = base64::engine::general_purpose::STANDARD.encode(wire_tx);
        let rpc =
            crate::infra::rpc::SolanaRpcClient::with_commitment(&cfg.cluster, &cfg.commitment);
        let sig = rpc.send_transaction(&tx_b64)?;
        println!();
        println!("Transaction sent: {sig}");
        wait_for_confirmation(&rpc, &sig, &cfg.commitment)?;
    }

    Ok(())
}

fn wait_for_confirmation(
    rpc: &crate::infra::rpc::SolanaRpcClient,
    signature: &str,
    commitment: &str,
) -> Result<(), MsigError> {
    eprintln!("Waiting for confirmation...");
    let start = std::time::Instant::now();
    let timeout = std::time::Duration::from_secs(60);
    let mut delay = std::time::Duration::from_millis(500);
    let max_delay = std::time::Duration::from_secs(4);

    loop {
        let statuses = rpc.get_signature_statuses(&[signature])?;
        if let Some(Some(status)) = statuses.into_iter().next() {
            if status.err.is_some() {
                let err_str = status
                    .err
                    .as_ref()
                    .map(|e| e.to_string())
                    .unwrap_or_else(|| "unknown error".to_string());
                return Err(MsigError::Transaction(format!(
                    "transaction failed: {err_str}"
                )));
            }
            let confirmed = match commitment {
                "finalized" => status
                    .confirmation_status
                    .as_deref()
                    .map(|s| s == "finalized")
                    .unwrap_or(false),
                _ => status
                    .confirmation_status
                    .as_deref()
                    .map(|s| s == "confirmed" || s == "finalized")
                    .unwrap_or(false),
            };
            if confirmed {
                eprintln!("Confirmed.");
                return Ok(());
            }
        }

        if start.elapsed() >= timeout {
            eprintln!("Warning: confirmation timed out after 60s. Transaction may still land.");
            return Ok(());
        }

        std::thread::sleep(delay);
        delay = (delay * 2).min(max_delay);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn parse_create_data_accepts_hex_and_utf8() {
        assert_eq!(
            parse_create_data("0x0200_0000").unwrap_or_else(|e| panic!("{e}")),
            vec![2, 0, 0, 0]
        );
        assert_eq!(
            parse_create_data("utf8:ok").unwrap_or_else(|e| panic!("{e}")),
            b"ok".to_vec()
        );
    }

    #[test]
    fn parse_vault_message_accepts_json_byte_array() {
        assert_eq!(
            parse_vault_message("[1, 2, 3, 5, 5, 6, 7, 8]").unwrap_or_else(|e| panic!("{e}")),
            vec![1, 2, 3, 5, 5, 6, 7, 8]
        );
    }

    #[test]
    fn parse_vault_message_rejects_utf8_prefix() {
        assert!(parse_vault_message("utf8:not-a-vault-message").is_err());
    }

    #[test]
    fn parse_create_account_supports_context_and_flags() {
        let vault = Pubkey::from([1u8; 32]);
        let multisig = Pubkey::from([2u8; 32]);
        let program_id = Pubkey::from([3u8; 32]);
        let labels = HashMap::new();

        let account = parse_create_account(
            "vault:writable:signer",
            &vault,
            &multisig,
            &program_id,
            &labels,
        )
        .unwrap_or_else(|e| panic!("{e}"));

        assert_eq!(account.pubkey, vault);
        assert!(account.is_writable);
        assert!(account.is_signer);
    }

    #[test]
    fn parse_create_account_rejects_unknown_flags() {
        let vault = Pubkey::from([1u8; 32]);
        let multisig = Pubkey::from([2u8; 32]);
        let program_id = Pubkey::from([3u8; 32]);
        let labels = HashMap::new();

        assert!(
            parse_create_account("vault:admin", &vault, &multisig, &program_id, &labels).is_err()
        );
    }
}
