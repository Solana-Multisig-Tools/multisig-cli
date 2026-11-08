use solana_pubkey::Pubkey;

use super::{
    build_context, load_config_only, next_verb, parse_value, require_explicit_vault_index,
    resolve_labeled_pubkey as resolve_pubkey, GlobalOpts,
};
use crate::application::stake_account::{
    self, StakeOperation, StakeProposalRequest, StakeWithdrawAmount,
};
use crate::application::validator_common;
use crate::error::{MsigError, OutputMode};
use crate::infra::config::tokens;
use crate::output::{format_sol, json};

const PROPOSAL_VERBS: &[&str] = &[
    "create",
    "delegate",
    "deactivate",
    "withdraw",
    "split",
    "merge",
    "set-staker",
    "set-withdrawer",
];

pub fn run(globals: GlobalOpts, mut parser: lexopt::Parser) -> Result<(), MsigError> {
    let verb = next_verb(&mut parser, "stake-account")?;
    match verb.as_str() {
        "show" => cmd_show(globals, &mut parser),
        v if PROPOSAL_VERBS.contains(&v) => match parse_proposal_args(&verb, &mut parser)? {
            Some(args) => cmd_propose(globals, args),
            None => Ok(()),
        },
        other => Err(MsigError::Usage(format!(
            "unknown stake-account command: '{other}'"
        ))),
    }
}

// ---------------------------------------------------------------------------
// stake-account show
// ---------------------------------------------------------------------------

fn cmd_show(globals: GlobalOpts, parser: &mut lexopt::Parser) -> Result<(), MsigError> {
    use lexopt::Arg::*;
    let mut stake: Option<String> = None;

    while let Some(arg) = parser.next().map_err(|e| MsigError::Usage(e.to_string()))? {
        match arg {
            Value(val) if stake.is_none() => {
                stake = Some(
                    val.into_string()
                        .map_err(|_| MsigError::Usage("invalid UTF-8".into()))?,
                );
            }
            Short('h') | Long("help") => {
                super::help::print_resource_help("stake-account");
                return Ok(());
            }
            _ => return Err(MsigError::Usage(
                "unexpected argument for 'stake-account show'. Run 'msig stake-account --help'."
                    .into(),
            )),
        }
    }
    let stake =
        stake.ok_or_else(|| MsigError::Usage("stake account address is required".into()))?;

    let cfg = load_config_only(&globals)?;
    let output_mode = crate::output::detect_output_mode(globals.output.as_deref());
    let rpc = crate::infra::rpc::SolanaRpcClient::with_commitment(&cfg.cluster, &cfg.commitment);
    let stake = resolve_pubkey(&stake, &cfg.labels, "stake account")?;

    // Label the authorities with their vault index when a multisig is known.
    let vaults = cfg
        .multisig
        .as_deref()
        .map(|addr| {
            resolve_pubkey(addr, &cfg.labels, "multisig")
                .map(|ms| validator_common::vault_addresses(&ms, &cfg.program_id))
        })
        .transpose()?;

    let info = stake_account::fetch_stake_account_info(&rpc, &stake, vaults.as_ref())?;

    match output_mode {
        OutputMode::Json => json::print_json(&info),
        OutputMode::Text => {
            let tag_of = |pk: &Pubkey| -> String {
                match vaults.as_ref().and_then(|v| v.get(pk)) {
                    Some(index) => format!(" (vault {index})"),
                    None => String::new(),
                }
            };
            println!("Stake account: {}", info.stake_account);
            println!("  State: {}", info.state);
            println!("  Balance: {} SOL", format_sol(info.balance_lamports));
            if let Some(reserve) = info.rent_exempt_reserve_lamports {
                println!("  Rent-exempt reserve: {} SOL", format_sol(reserve));
            }
            if let Some(ref staker) = info.staker {
                println!("  Staker: {staker}{}", tag_of(staker));
            }
            if let Some(ref withdrawer) = info.withdrawer {
                println!("  Withdrawer: {withdrawer}{}", tag_of(withdrawer));
            }
            if let Some(ref lockup) = info.lockup {
                if lockup.in_force {
                    println!(
                        "  Lockup: IN FORCE until epoch {} / unix time {} (custodian {})",
                        lockup.epoch, lockup.unix_timestamp, lockup.custodian
                    );
                } else if lockup.epoch != 0 || lockup.unix_timestamp != 0 {
                    println!(
                        "  Lockup: expired (epoch {}, unix time {})",
                        lockup.epoch, lockup.unix_timestamp
                    );
                } else {
                    println!("  Lockup: none");
                }
            }
            if let Some(ref delegation) = info.delegation {
                println!("  Delegated to vote account: {}", delegation.vote_account);
                println!(
                    "  Delegated stake: {} SOL",
                    format_sol(delegation.stake_lamports)
                );
                println!("  Activation epoch: {}", delegation.activation_epoch);
                match delegation.deactivation_epoch {
                    Some(epoch) => println!("  Deactivation epoch: {epoch}"),
                    None => println!("  Deactivation epoch: none (not deactivating)"),
                }
            }
            println!("  Current epoch: {}", info.current_epoch);
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Proposal verbs
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
enum VerbArgs {
    Create {
        lamports: u64,
        vote_account: Option<String>,
    },
    Delegate {
        stake: String,
        vote_account: String,
    },
    Deactivate {
        stake: String,
    },
    Withdraw {
        stake: String,
        recipient: String,
        amount: StakeWithdrawAmount,
    },
    Split {
        stake: String,
        lamports: u64,
    },
    Merge {
        destination: String,
        source: String,
    },
    SetStaker {
        stake: String,
        new_staker: String,
    },
    SetWithdrawer {
        stake: String,
        new_withdrawer: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProposalArgs {
    verb: String,
    args: VerbArgs,
    allow_external_authority: bool,
    allow_simulation_failure: bool,
}

fn parse_sol(raw: &str, what: &str) -> Result<u64, MsigError> {
    let lamports = tokens::parse_human_amount(raw, 9)?;
    if lamports == 0 {
        return Err(MsigError::Usage(format!(
            "{what} must be greater than zero"
        )));
    }
    Ok(lamports)
}

/// Parse a proposal verb's arguments. Returns `None` when help was printed.
fn parse_proposal_args(
    verb: &str,
    parser: &mut lexopt::Parser,
) -> Result<Option<ProposalArgs>, MsigError> {
    use lexopt::Arg::*;

    let mut positionals: Vec<String> = Vec::new();
    let mut amount: Option<String> = None;
    let mut all = false;
    let mut vote_account: Option<String> = None;
    let mut allow_external_authority = false;
    let mut allow_simulation_failure = false;
    let is_withdraw = verb == "withdraw";
    let is_create = verb == "create";
    let accepts_external = verb == "set-withdrawer";

    while let Some(arg) = parser.next().map_err(|e| MsigError::Usage(e.to_string()))? {
        match arg {
            Value(val) => positionals.push(
                val.into_string()
                    .map_err(|_| MsigError::Usage("invalid UTF-8".into()))?,
            ),
            Long("amount") if is_withdraw => amount = Some(parse_value(parser, "--amount")?),
            Long("all") if is_withdraw => all = true,
            Long("vote-account") if is_create => {
                vote_account = Some(parse_value(parser, "--vote-account")?)
            }
            Long("allow-external-authority") if accepts_external => allow_external_authority = true,
            Long("allow-simulation-failure") => allow_simulation_failure = true,
            Short('h') | Long("help") => {
                super::help::print_resource_help("stake-account");
                return Ok(None);
            }
            _ => {
                return Err(MsigError::Usage(format!(
                    "unexpected argument for 'stake-account {verb}'. \
                     Run 'msig stake-account --help'."
                )))
            }
        }
    }

    let expected = match verb {
        "create" | "deactivate" => 1,
        _ => 2,
    };
    if positionals.len() != expected {
        return Err(MsigError::Usage(format!(
            "'stake-account {verb}' takes {expected} argument{}, got {}. Run 'msig stake-account --help'.",
            if expected == 1 { "" } else { "s" },
            positionals.len()
        )));
    }
    let mut positionals = positionals.into_iter();
    let mut next = || positionals.next().unwrap_or_default();

    let args = match verb {
        "create" => VerbArgs::Create {
            lamports: parse_sol(&next(), "the amount")?,
            vote_account,
        },
        "delegate" => VerbArgs::Delegate {
            stake: next(),
            vote_account: next(),
        },
        "deactivate" => VerbArgs::Deactivate { stake: next() },
        "withdraw" => {
            let stake = next();
            let recipient = next();
            let amount = match (amount, all) {
                (Some(sol), false) => StakeWithdrawAmount::Lamports(parse_sol(&sol, "--amount")?),
                (None, true) => StakeWithdrawAmount::All,
                _ => {
                    return Err(MsigError::Usage(
                        "withdraw needs exactly one of --amount <SOL> or --all".into(),
                    ))
                }
            };
            VerbArgs::Withdraw {
                stake,
                recipient,
                amount,
            }
        }
        "split" => {
            let stake = next();
            VerbArgs::Split {
                stake,
                lamports: parse_sol(&next(), "the split amount")?,
            }
        }
        "merge" => VerbArgs::Merge {
            destination: next(),
            source: next(),
        },
        "set-staker" => VerbArgs::SetStaker {
            stake: next(),
            new_staker: next(),
        },
        "set-withdrawer" => VerbArgs::SetWithdrawer {
            stake: next(),
            new_withdrawer: next(),
        },
        other => {
            return Err(MsigError::Usage(format!(
                "unknown stake-account command: '{other}'"
            )))
        }
    };

    Ok(Some(ProposalArgs {
        verb: verb.to_string(),
        args,
        allow_external_authority,
        allow_simulation_failure,
    }))
}

fn require_vault_index(globals: &GlobalOpts) -> Result<u8, MsigError> {
    require_explicit_vault_index(
        globals,
        "stake-account",
        "the vault that is the stake account's staker or withdrawer, or funds a new one",
    )
}

fn cmd_propose(globals: GlobalOpts, args: ProposalArgs) -> Result<(), MsigError> {
    let vault_index = require_vault_index(&globals)?;
    let output_mode = crate::output::detect_output_mode(globals.output.as_deref());
    let is_json = matches!(output_mode, OutputMode::Json);

    let ctx = build_context(&globals)?;
    let multisig = ctx.resolve_multisig(None)?;
    let labels = &ctx.config.labels;
    let stake = |input: &str| resolve_pubkey(input, labels, "stake account");

    let operation = match args.args {
        VerbArgs::Create {
            lamports,
            vote_account,
        } => StakeOperation::Create {
            lamports,
            vote_account: vote_account
                .map(|vote| resolve_pubkey(&vote, labels, "vote account"))
                .transpose()?,
        },
        VerbArgs::Delegate {
            stake: s,
            vote_account,
        } => StakeOperation::Delegate {
            stake: stake(&s)?,
            vote_account: resolve_pubkey(&vote_account, labels, "vote account")?,
        },
        VerbArgs::Deactivate { stake: s } => StakeOperation::Deactivate { stake: stake(&s)? },
        VerbArgs::Withdraw {
            stake: s,
            recipient,
            amount,
        } => StakeOperation::Withdraw {
            stake: stake(&s)?,
            recipient: resolve_pubkey(&recipient, labels, "recipient")?,
            amount,
        },
        VerbArgs::Split { stake: s, lamports } => StakeOperation::Split {
            stake: stake(&s)?,
            lamports,
        },
        VerbArgs::Merge {
            destination,
            source,
        } => StakeOperation::Merge {
            destination: stake(&destination)?,
            source: stake(&source)?,
        },
        VerbArgs::SetStaker {
            stake: s,
            new_staker,
        } => StakeOperation::SetStaker {
            stake: stake(&s)?,
            new_staker: resolve_pubkey(&new_staker, labels, "staker")?,
        },
        VerbArgs::SetWithdrawer {
            stake: s,
            new_withdrawer,
        } => StakeOperation::SetWithdrawer {
            stake: stake(&s)?,
            new_withdrawer: resolve_pubkey(&new_withdrawer, labels, "withdrawer")?,
        },
    };

    let request = StakeProposalRequest {
        multisig,
        vault_index,
        operation,
        allow_external_authority: args.allow_external_authority,
        allow_simulation_failure: args.allow_simulation_failure,
        memo: globals.memo.as_deref(),
        dry_run: globals.dry_run,
        skip_confirm: globals.yes,
        quiet: is_json,
    };
    let result = stake_account::create_stake_account_proposal(
        ctx.rpc(),
        ctx.signer.as_ref(),
        &ctx.config,
        &request,
    )?;

    if let (Some(new_stake), false) = (result.new_stake_account, is_json) {
        println!("New stake account: {new_stake}");
    }

    if is_json {
        let stake_account = match operation {
            StakeOperation::Create { .. } => result.new_stake_account,
            StakeOperation::Delegate { stake, .. }
            | StakeOperation::Deactivate { stake }
            | StakeOperation::Withdraw { stake, .. }
            | StakeOperation::Split { stake, .. }
            | StakeOperation::SetStaker { stake, .. }
            | StakeOperation::SetWithdrawer { stake, .. } => Some(stake),
            StakeOperation::Merge { destination, .. } => Some(destination),
        };
        let proposal = &result.proposal;
        let mut out = serde_json::json!({
            "action": format!("stake_account_{}", args.verb.replace('-', "_")),
            "stake_account": stake_account.map(|pk| pk.to_string()),
            "transaction_index": proposal.transaction_index,
            "vault_index": vault_index,
            "vault": proposal.vault.to_string(),
            "transaction": proposal.transaction.to_string(),
            "proposal": proposal.proposal.to_string(),
            "signature": proposal.signature,
        });
        if let Some(new_stake) = result.new_stake_account {
            out["new_stake_account"] = serde_json::Value::String(new_stake.to_string());
        }
        if let StakeOperation::Merge { source, .. } = operation {
            out["source_stake_account"] = serde_json::Value::String(source.to_string());
        }
        json::print_json(&out);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(verb: &str, args: &[&str]) -> Result<Option<ProposalArgs>, MsigError> {
        let mut parser = lexopt::Parser::from_args(args.iter().map(|a| a.to_string()));
        parse_proposal_args(verb, &mut parser)
    }

    fn parsed(verb: &str, args: &[&str]) -> ProposalArgs {
        parse(verb, args)
            .unwrap_or_else(|e| panic!("{e}"))
            .unwrap_or_else(|| panic!("help displayed"))
    }

    fn usage_err(verb: &str, args: &[&str]) -> String {
        match parse(verb, args) {
            Err(MsigError::Usage(msg)) => msg,
            Err(MsigError::Config(msg)) => msg,
            other => panic!("expected usage error, got {other:?}"),
        }
    }

    #[test]
    fn create_parses_amount_and_optional_vote_account() {
        let args = parsed("create", &["2"]);
        assert_eq!(
            args.args,
            VerbArgs::Create {
                lamports: 2_000_000_000,
                vote_account: None,
            }
        );
        let args = parsed(
            "create",
            &[
                "--vote-account",
                "VOTE",
                "1.5",
                "--allow-simulation-failure",
            ],
        );
        assert_eq!(
            args.args,
            VerbArgs::Create {
                lamports: 1_500_000_000,
                vote_account: Some("VOTE".into()),
            }
        );
        assert!(args.allow_simulation_failure);
        assert!(usage_err("create", &["0"]).contains("greater than zero"));
        usage_err("create", &["abc"]);
        assert!(usage_err("create", &[]).contains("takes 1 argument"));
        usage_err("delegate", &["S", "V", "--vote-account", "X"]);
    }

    #[test]
    fn withdraw_parses_each_amount_mode() {
        let args = parsed("withdraw", &["STAKE", "R", "--amount", "1.5"]);
        assert_eq!(
            args.args,
            VerbArgs::Withdraw {
                stake: "STAKE".into(),
                recipient: "R".into(),
                amount: StakeWithdrawAmount::Lamports(1_500_000_000),
            }
        );
        let args = parsed("withdraw", &["--all", "STAKE", "R"]);
        assert!(matches!(
            args.args,
            VerbArgs::Withdraw {
                amount: StakeWithdrawAmount::All,
                ..
            }
        ));
        assert!(usage_err("withdraw", &["STAKE", "R"]).contains("exactly one"));
        assert!(
            usage_err("withdraw", &["STAKE", "R", "--all", "--amount", "1"])
                .contains("exactly one")
        );
        assert!(
            usage_err("withdraw", &["STAKE", "R", "--amount", "0"]).contains("greater than zero")
        );
        // --close is a vote-account flag, not a stake-account one.
        usage_err("withdraw", &["STAKE", "R", "--close"]);
        assert!(usage_err("withdraw", &["STAKE", "--all"]).contains("takes 2 arguments"));
    }

    #[test]
    fn positional_verbs() {
        assert_eq!(
            parsed("delegate", &["S", "V"]).args,
            VerbArgs::Delegate {
                stake: "S".into(),
                vote_account: "V".into(),
            }
        );
        assert_eq!(
            parsed("deactivate", &["S"]).args,
            VerbArgs::Deactivate { stake: "S".into() }
        );
        assert_eq!(
            parsed("split", &["S", "0.5"]).args,
            VerbArgs::Split {
                stake: "S".into(),
                lamports: 500_000_000,
            }
        );
        assert_eq!(
            parsed("merge", &["DEST", "SRC"]).args,
            VerbArgs::Merge {
                destination: "DEST".into(),
                source: "SRC".into(),
            }
        );
        assert_eq!(
            parsed("set-staker", &["S", "NEW"]).args,
            VerbArgs::SetStaker {
                stake: "S".into(),
                new_staker: "NEW".into(),
            }
        );
        usage_err("deactivate", &["S", "EXTRA"]);
        usage_err("split", &["S", "0"]);
        usage_err("merge", &["DEST"]);
    }

    #[test]
    fn external_authority_flag_only_on_set_withdrawer() {
        let args = parsed(
            "set-withdrawer",
            &["S", "NEW", "--allow-external-authority"],
        );
        assert!(args.allow_external_authority);
        assert_eq!(
            args.args,
            VerbArgs::SetWithdrawer {
                stake: "S".into(),
                new_withdrawer: "NEW".into(),
            }
        );
        usage_err("set-staker", &["S", "NEW", "--allow-external-authority"]);
        usage_err(
            "withdraw",
            &["S", "R", "--all", "--allow-external-authority"],
        );
    }

    #[test]
    fn vault_index_must_be_explicit() {
        match require_vault_index(&GlobalOpts::default()) {
            Err(MsigError::Usage(msg)) => {
                assert!(msg.contains("--vault-index") && msg.starts_with("stake-account"))
            }
            other => panic!("expected usage error, got {other:?}"),
        }
        let globals = GlobalOpts {
            vault_index: Some(3),
            ..GlobalOpts::default()
        };
        assert_eq!(require_vault_index(&globals).ok(), Some(3));
    }
}
