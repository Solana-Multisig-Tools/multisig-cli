use super::{load_config_only, next_verb, parse_value, GlobalOpts};
use crate::application::inspect;
use crate::error::{MsigError, OutputMode};
use crate::output::{json, table};

pub fn run(globals: GlobalOpts, mut parser: lexopt::Parser) -> Result<(), MsigError> {
    let verb = next_verb(&mut parser, "vault")?;
    match verb.as_str() {
        "balance" => cmd_balance(globals, &mut parser),
        other => Err(MsigError::Usage(format!(
            "unknown vault command: '{other}'"
        ))),
    }
}

fn cmd_balance(globals: GlobalOpts, parser: &mut lexopt::Parser) -> Result<(), MsigError> {
    use lexopt::Arg::*;
    let mut multisig_override: Option<String> = None;
    while let Some(arg) = parser.next().map_err(|e| MsigError::Usage(e.to_string()))? {
        match arg {
            Long("multisig") => multisig_override = Some(parse_value(parser, "--multisig")?),
            Short('h') | Long("help") => {
                super::help::print_resource_help("vault");
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

    let balances =
        inspect::fetch_vault_balances(&rpc, &multisig_pubkey, vault_index, &program_id, &cfg)?;

    match output_mode {
        OutputMode::Json => json::print_json(&balances),
        OutputMode::Text => {
            println!("Vault [{}]: {}", vault_index, balances.vault_address);
            println!("SOL: {}", table::format_sol(balances.sol_lamports));
            for tb in &balances.token_balances {
                let sym = tb.symbol.as_deref().unwrap_or("???");
                println!("{sym}: {}", tb.ui_amount);
            }
        }
    }
    Ok(())
}
