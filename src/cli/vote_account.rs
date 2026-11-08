use solana_pubkey::Pubkey;

use super::{
    build_context, load_config_only, next_verb, parse_value, require_explicit_vault_index,
    resolve_labeled_pubkey as resolve_pubkey, GlobalOpts,
};
use crate::application::validator_common;
use crate::application::vote_account::{
    self, CollectorKind, VoteOperation, VoteProposalRequest, WithdrawAmount,
};
use crate::error::{MsigError, OutputMode};
use crate::infra::config::tokens;
use crate::output::{format_sol, json};

pub fn run(globals: GlobalOpts, mut parser: lexopt::Parser) -> Result<(), MsigError> {
    let verb = next_verb(&mut parser, "vote-account")?;
    match verb.as_str() {
        "show" => cmd_show(globals, &mut parser),
        "withdraw" | "set-commission" | "set-voter" | "set-withdrawer" | "set-collector"
        | "set-identity" => match parse_proposal_args(&verb, &mut parser)? {
            Some(args) => cmd_propose(globals, args),
            None => Ok(()),
        },
        other => Err(MsigError::Usage(format!(
            "unknown vote-account command: '{other}'"
        ))),
    }
}

// ---------------------------------------------------------------------------
// vote-account show
// ---------------------------------------------------------------------------

fn cmd_show(globals: GlobalOpts, parser: &mut lexopt::Parser) -> Result<(), MsigError> {
    use lexopt::Arg::*;
    let mut vote: Option<String> = None;

    while let Some(arg) = parser.next().map_err(|e| MsigError::Usage(e.to_string()))? {
        match arg {
            Value(val) if vote.is_none() => {
                vote = Some(
                    val.into_string()
                        .map_err(|_| MsigError::Usage("invalid UTF-8".into()))?,
                );
            }
            Short('h') | Long("help") => {
                super::help::print_resource_help("vote-account");
                return Ok(());
            }
            _ => {
                return Err(MsigError::Usage(
                    "unexpected argument for 'vote-account show'. Run 'msig vote-account --help'."
                        .into(),
                ))
            }
        }
    }
    let vote = vote.ok_or_else(|| MsigError::Usage("vote account address is required".into()))?;

    let cfg = load_config_only(&globals)?;
    let output_mode = crate::output::detect_output_mode(globals.output.as_deref());
    let rpc = crate::infra::rpc::SolanaRpcClient::with_commitment(&cfg.cluster, &cfg.commitment);
    let vote = resolve_pubkey(&vote, &cfg.labels, "vote account")?;

    // Label the withdrawer with its vault index when a multisig is known.
    let vaults = cfg
        .multisig
        .as_deref()
        .map(|addr| {
            resolve_pubkey(addr, &cfg.labels, "multisig")
                .map(|ms| validator_common::vault_addresses(&ms, &cfg.program_id))
        })
        .transpose()?;

    let info = vote_account::fetch_vote_account_info(&rpc, &vote, vaults.as_ref())?;

    match output_mode {
        OutputMode::Json => json::print_json(&info),
        OutputMode::Text => {
            let tag_of = |pk: &Pubkey| -> String {
                if *pk == info.vote_account {
                    " (vote account)".into()
                } else {
                    match vaults.as_ref().and_then(|v| v.get(pk)) {
                        Some(index) => format!(" (vault {index})"),
                        None => String::new(),
                    }
                }
            };
            println!("Vote account: {}", info.vote_account);
            println!("  Version: {}", info.version);
            println!("  Balance: {} SOL", format_sol(info.balance_lamports));
            println!(
                "  Rent-exempt minimum: {} SOL",
                format_sol(info.rent_exempt_minimum_lamports)
            );
            println!("  Validator identity: {}", info.validator_identity);
            println!(
                "  Authorized withdrawer: {}{}",
                info.authorized_withdrawer,
                tag_of(&info.authorized_withdrawer)
            );
            if let Some(percent) = info.commission_percent {
                println!("  Commission: {percent}%");
            }
            if let Some(bps) = info.inflation_rewards_commission_bps {
                println!("  Inflation rewards commission: {}", format_bps(bps));
            }
            if let Some(bps) = info.block_revenue_commission_bps {
                println!("  Block revenue commission: {}", format_bps(bps));
            }
            if let Some(ref c) = info.inflation_rewards_collector {
                println!("  Inflation rewards collector: {c}{}", tag_of(c));
            }
            if let Some(ref c) = info.block_revenue_collector {
                println!("  Block revenue collector: {c}{}", tag_of(c));
            }
            println!(
                "  BLS key registered: {}",
                if info.bls_key_registered { "yes" } else { "no" }
            );
        }
    }
    Ok(())
}

fn format_bps(bps: u16) -> String {
    format!("{bps} bps ({}.{:02}%)", bps / 100, bps % 100)
}

// ---------------------------------------------------------------------------
// Proposal verbs
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
enum VerbArgs {
    Withdraw {
        recipient: String,
        amount: WithdrawAmount,
    },
    SetCommission {
        percent: u8,
    },
    SetVoter {
        new_voter: String,
    },
    SetWithdrawer {
        new_withdrawer: String,
    },
    SetCollector {
        kind: CollectorKind,
        collector: String,
    },
    SetIdentity {
        new_identity: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProposalArgs {
    verb: String,
    vote_account: String,
    args: VerbArgs,
    allow_external_authority: bool,
    allow_simulation_failure: bool,
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
    let mut close = false;
    let mut allow_external_authority = false;
    let mut allow_simulation_failure = false;
    let is_withdraw = verb == "withdraw";
    let accepts_external = matches!(verb, "set-withdrawer" | "set-collector");

    while let Some(arg) = parser.next().map_err(|e| MsigError::Usage(e.to_string()))? {
        match arg {
            Value(val) => positionals.push(
                val.into_string()
                    .map_err(|_| MsigError::Usage("invalid UTF-8".into()))?,
            ),
            Long("amount") if is_withdraw => amount = Some(parse_value(parser, "--amount")?),
            Long("all") if is_withdraw => all = true,
            Long("close") if is_withdraw => close = true,
            Long("allow-external-authority") if accepts_external => allow_external_authority = true,
            Long("allow-simulation-failure") => allow_simulation_failure = true,
            Short('h') | Long("help") => {
                super::help::print_resource_help("vote-account");
                return Ok(None);
            }
            _ => {
                return Err(MsigError::Usage(format!(
                    "unexpected argument for 'vote-account {verb}'. Run 'msig vote-account --help'."
                )))
            }
        }
    }

    let expected = if verb == "set-collector" { 3 } else { 2 };
    if positionals.len() != expected {
        return Err(MsigError::Usage(format!(
            "'vote-account {verb}' takes {expected} arguments, got {}. Run 'msig vote-account --help'.",
            positionals.len()
        )));
    }
    let mut positionals = positionals.into_iter();
    let mut next = || positionals.next().unwrap_or_default();
    let vote_account = next();

    let args = match verb {
        "withdraw" => {
            let recipient = next();
            let chosen = usize::from(amount.is_some()) + usize::from(all) + usize::from(close);
            if chosen != 1 {
                return Err(MsigError::Usage(
                    "withdraw needs exactly one of --amount <SOL>, --all or --close".into(),
                ));
            }
            let amount = match amount {
                Some(sol) => {
                    let lamports = tokens::parse_human_amount(&sol, 9)?;
                    if lamports == 0 {
                        return Err(MsigError::Usage(
                            "--amount must be greater than zero".into(),
                        ));
                    }
                    WithdrawAmount::Lamports(lamports)
                }
                None if all => WithdrawAmount::All,
                None => WithdrawAmount::Close,
            };
            VerbArgs::Withdraw { recipient, amount }
        }
        "set-commission" => {
            let raw = next();
            let percent = raw
                .parse::<u8>()
                .ok()
                .filter(|p| *p <= 100)
                .ok_or_else(|| {
                    MsigError::Usage(format!(
                        "invalid commission '{raw}': expected a whole percent from 0 to 100"
                    ))
                })?;
            VerbArgs::SetCommission { percent }
        }
        "set-voter" => VerbArgs::SetVoter { new_voter: next() },
        "set-withdrawer" => VerbArgs::SetWithdrawer {
            new_withdrawer: next(),
        },
        "set-collector" => {
            let kind = CollectorKind::parse(&next())?;
            VerbArgs::SetCollector {
                kind,
                collector: next(),
            }
        }
        "set-identity" => VerbArgs::SetIdentity {
            new_identity: next(),
        },
        other => {
            return Err(MsigError::Usage(format!(
                "unknown vote-account command: '{other}'"
            )))
        }
    };

    Ok(Some(ProposalArgs {
        verb: verb.to_string(),
        vote_account,
        args,
        allow_external_authority,
        allow_simulation_failure,
    }))
}

fn require_vault_index(globals: &GlobalOpts) -> Result<u8, MsigError> {
    require_explicit_vault_index(
        globals,
        "vote-account",
        "the vault that is the vote account's authorized withdrawer",
    )
}

fn cmd_propose(globals: GlobalOpts, args: ProposalArgs) -> Result<(), MsigError> {
    let vault_index = require_vault_index(&globals)?;
    let output_mode = crate::output::detect_output_mode(globals.output.as_deref());
    let is_json = matches!(output_mode, OutputMode::Json);

    let ctx = build_context(&globals)?;
    let multisig = ctx.resolve_multisig(None)?;
    let labels = &ctx.config.labels;
    let vote = resolve_pubkey(&args.vote_account, labels, "vote account")?;

    let operation = match args.args {
        VerbArgs::Withdraw { recipient, amount } => VoteOperation::Withdraw {
            recipient: resolve_pubkey(&recipient, labels, "recipient")?,
            amount,
        },
        VerbArgs::SetCommission { percent } => VoteOperation::SetCommission { percent },
        VerbArgs::SetVoter { new_voter } => VoteOperation::SetVoter {
            new_voter: resolve_pubkey(&new_voter, labels, "voter")?,
        },
        VerbArgs::SetWithdrawer { new_withdrawer } => VoteOperation::SetWithdrawer {
            new_withdrawer: resolve_pubkey(&new_withdrawer, labels, "withdrawer")?,
        },
        VerbArgs::SetCollector { kind, collector } => VoteOperation::SetCollector {
            kind,
            collector: resolve_pubkey(&collector, labels, "collector")?,
        },
        VerbArgs::SetIdentity { new_identity } => VoteOperation::SetIdentity {
            new_identity: resolve_pubkey(&new_identity, labels, "validator identity")?,
        },
    };

    let request = VoteProposalRequest {
        multisig,
        vault_index,
        vote_account: vote,
        operation,
        allow_external_authority: args.allow_external_authority,
        allow_simulation_failure: args.allow_simulation_failure,
        memo: globals.memo.as_deref(),
        dry_run: globals.dry_run,
        skip_confirm: globals.yes,
        quiet: is_json,
    };
    let result = vote_account::create_vote_account_proposal(
        ctx.rpc(),
        ctx.signer.as_ref(),
        &ctx.config,
        &request,
    )?;

    if is_json {
        let mut out = serde_json::json!({
            "action": format!("vote_account_{}", args.verb.replace('-', "_")),
            "vote_account": vote.to_string(),
            "transaction_index": result.transaction_index,
            "vault_index": vault_index,
            "vault": result.vault.to_string(),
            "transaction": result.transaction.to_string(),
            "proposal": result.proposal.to_string(),
            "signature": result.signature,
        });
        if let VoteOperation::SetIdentity { new_identity } = operation {
            // The new identity must co-sign `proposal execute --extra-signer`.
            out["execute_extra_signers"] = serde_json::json!([new_identity.to_string()]);
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
            other => panic!("expected usage error, got {other:?}"),
        }
    }

    #[test]
    fn withdraw_parses_each_amount_mode() {
        let args = parsed("withdraw", &["VOTE", "RECIPIENT", "--amount", "1.5"]);
        assert_eq!(args.vote_account, "VOTE");
        assert_eq!(
            args.args,
            VerbArgs::Withdraw {
                recipient: "RECIPIENT".into(),
                amount: WithdrawAmount::Lamports(1_500_000_000),
            }
        );
        assert!(!args.allow_simulation_failure);

        let args = parsed("withdraw", &["--all", "VOTE", "RECIPIENT"]);
        assert!(matches!(
            args.args,
            VerbArgs::Withdraw {
                amount: WithdrawAmount::All,
                ..
            }
        ));

        let args = parsed(
            "withdraw",
            &["VOTE", "RECIPIENT", "--close", "--allow-simulation-failure"],
        );
        assert!(matches!(
            args.args,
            VerbArgs::Withdraw {
                amount: WithdrawAmount::Close,
                ..
            }
        ));
        assert!(args.allow_simulation_failure);
    }

    #[test]
    fn withdraw_requires_exactly_one_amount_mode() {
        assert!(usage_err("withdraw", &["VOTE", "RECIPIENT"]).contains("exactly one"));
        assert!(
            usage_err("withdraw", &["VOTE", "RECIPIENT", "--all", "--close"])
                .contains("exactly one")
        );
        assert!(
            usage_err("withdraw", &["VOTE", "RECIPIENT", "--amount", "1", "--all"])
                .contains("exactly one")
        );
        assert!(
            usage_err("withdraw", &["VOTE", "RECIPIENT", "--amount", "0"])
                .contains("greater than zero")
        );
        assert!(usage_err("withdraw", &["VOTE", "--all"]).contains("takes 2 arguments"));
    }

    #[test]
    fn external_authority_flag_only_on_authority_verbs() {
        let args = parsed(
            "set-withdrawer",
            &["VOTE", "NEW", "--allow-external-authority"],
        );
        assert!(args.allow_external_authority);
        usage_err(
            "withdraw",
            &["VOTE", "R", "--all", "--allow-external-authority"],
        );
        usage_err("set-commission", &["VOTE", "5", "--all"]);
    }

    #[test]
    fn set_commission_bounds() {
        let args = parsed("set-commission", &["VOTE", "100"]);
        assert_eq!(args.args, VerbArgs::SetCommission { percent: 100 });
        usage_err("set-commission", &["VOTE", "101"]);
        usage_err("set-commission", &["VOTE", "-1"]);
        usage_err("set-commission", &["VOTE", "5.5"]);
    }

    #[test]
    fn set_collector_parses_kind() {
        let args = parsed("set-collector", &["VOTE", "block-revenue", "C"]);
        assert_eq!(
            args.args,
            VerbArgs::SetCollector {
                kind: CollectorKind::BlockRevenue,
                collector: "C".into(),
            }
        );
        usage_err("set-collector", &["VOTE", "fees", "C"]);
        usage_err("set-collector", &["VOTE", "C"]);
    }

    #[test]
    fn set_identity_parses_two_positionals() {
        let args = parsed("set-identity", &["VOTE", "NEW_ID"]);
        assert_eq!(args.vote_account, "VOTE");
        assert_eq!(
            args.args,
            VerbArgs::SetIdentity {
                new_identity: "NEW_ID".into()
            }
        );
        let args = parsed(
            "set-identity",
            &["VOTE", "NEW_ID", "--allow-simulation-failure"],
        );
        assert!(args.allow_simulation_failure);
        usage_err("set-identity", &["VOTE"]);
        usage_err(
            "set-identity",
            &["VOTE", "NEW_ID", "--allow-external-authority"],
        );
    }

    #[test]
    fn vault_index_must_be_explicit() {
        let globals = GlobalOpts::default();
        match require_vault_index(&globals) {
            Err(MsigError::Usage(msg)) => assert!(msg.contains("--vault-index")),
            other => panic!("expected usage error, got {other:?}"),
        }
        let globals = GlobalOpts {
            vault_index: Some(0),
            ..GlobalOpts::default()
        };
        assert_eq!(require_vault_index(&globals).ok(), Some(0));
    }

    #[test]
    fn formats_bps_without_floats() {
        assert_eq!(format_bps(300), "300 bps (3.00%)");
        assert_eq!(format_bps(10_000), "10000 bps (100.00%)");
        assert_eq!(format_bps(5), "5 bps (0.05%)");
    }
}
