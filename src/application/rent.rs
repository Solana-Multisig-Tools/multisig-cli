use solana_pubkey::Pubkey;

use crate::error::MsigError;
use crate::infra::accounts::multisig::MultisigAccount;
use crate::infra::accounts::proposal::ProposalAccount;
use crate::infra::config::Config;
use crate::infra::instruction::{AccountMeta, Instruction};
use crate::infra::pda;
use crate::infra::rpc::RpcProvider;
use crate::infra::signer::Signer;
use crate::output::format_sol;

use super::pipeline::{execute_transaction, PreparedTransaction};

use crate::infra::accounts::discriminators;

const SYSTEM_PROGRAM: Pubkey = solana_pubkey::pubkey!("11111111111111111111111111111111");
const VAULT_TX_ACCOUNTS_CLOSE_DISC: [u8; 8] = [0xc4, 0x47, 0xbb, 0xb0, 0x02, 0x23, 0xaa, 0xa5];
const CONFIG_TX_ACCOUNTS_CLOSE_DISC: [u8; 8] = [0x50, 0xcb, 0x54, 0x35, 0x97, 0x70, 0xbb, 0xba];

#[derive(Clone, Copy)]
pub enum TxKind {
    Vault,
    Config,
}

pub(crate) fn build_transaction_accounts_close_instruction(
    program_id: Pubkey,
    multisig: Pubkey,
    proposal: Pubkey,
    transaction: Pubkey,
    rent_collector: Pubkey,
    kind: TxKind,
) -> Instruction {
    let disc = match kind {
        TxKind::Vault => VAULT_TX_ACCOUNTS_CLOSE_DISC,
        TxKind::Config => CONFIG_TX_ACCOUNTS_CLOSE_DISC,
    };
    Instruction {
        program_id,
        accounts: vec![
            AccountMeta::new_readonly(multisig, false),
            AccountMeta::new(proposal, false),
            AccountMeta::new(transaction, false),
            AccountMeta::new(rent_collector, false),
            AccountMeta::new_readonly(SYSTEM_PROGRAM, false),
        ],
        data: disc.to_vec(),
    }
}

#[allow(clippy::too_many_arguments)]
pub fn claim_rent(
    rpc: &dyn RpcProvider,
    signer: &dyn Signer,
    multisig_addr: &Pubkey,
    last_n: u64,
    dry_run: bool,
    config: &Config,
    skip_confirm: bool,
    program_id: &Pubkey,
) -> Result<(), MsigError> {
    let multisig_str = multisig_addr.to_string();

    let ms_account = rpc
        .get_account_info(&multisig_str)?
        .ok_or_else(|| MsigError::MultisigNotFound(multisig_str.clone()))?;
    let ms = MultisigAccount::parse(&ms_account.data)?;

    if ms.transaction_index == 0 {
        println!("No transactions found.");
        return Ok(());
    }

    let rent_collector: Pubkey = *ms.rent_collector.as_ref()
        .ok_or_else(|| MsigError::Config("multisig has no rent_collector set. Set one with a config transaction before reclaiming rent.".into()))?;

    let start = ms
        .transaction_index
        .saturating_sub(last_n)
        .saturating_add(1);
    let end = ms.transaction_index;
    let mut closable_proposals: Vec<(u64, Pubkey, Pubkey, TxKind)> = Vec::new();
    let mut scanned = 0u64;

    let mut batch_start = start;
    while batch_start <= end {
        let batch_end = (batch_start + 99).min(end);
        let mut proposal_addrs: Vec<String> = Vec::new();
        let mut tx_addrs: Vec<String> = Vec::new();
        let mut indexes: Vec<u64> = Vec::new();

        for idx in batch_start..=batch_end {
            let (prop_pubkey, _) = pda::proposal_pda(multisig_addr, idx, program_id);
            let (tx_pubkey, _) = pda::transaction_pda(multisig_addr, idx, program_id);
            proposal_addrs.push(prop_pubkey.to_string());
            tx_addrs.push(tx_pubkey.to_string());
            indexes.push(idx);
        }

        let prop_refs: Vec<&str> = proposal_addrs.iter().map(|s| s.as_str()).collect();
        let tx_refs: Vec<&str> = tx_addrs.iter().map(|s| s.as_str()).collect();
        let prop_accounts = rpc.get_multiple_accounts(&prop_refs)?;
        let tx_accounts = rpc.get_multiple_accounts(&tx_refs)?;

        for (i, maybe_acct) in prop_accounts.into_iter().enumerate() {
            scanned += 1;
            if let Some(acct) = maybe_acct {
                if let Ok(prop) = ProposalAccount::parse(&acct.data) {
                    if prop.status.is_terminal() {
                        let (prop_pubkey, _) =
                            pda::proposal_pda(multisig_addr, indexes[i], program_id);
                        let (tx_pubkey, _) =
                            pda::transaction_pda(multisig_addr, indexes[i], program_id);

                        // Determine tx kind from the transaction account discriminator.
                        let kind = match &tx_accounts[i] {
                            Some(tx_acct)
                                if tx_acct.data.len() >= 8
                                    && tx_acct.data[..8] == discriminators::CONFIG_TRANSACTION =>
                            {
                                TxKind::Config
                            }
                            _ => TxKind::Vault,
                        };
                        closable_proposals.push((indexes[i], prop_pubkey, tx_pubkey, kind));
                    }
                }
            }
        }
        batch_start = batch_end + 1;
    }

    println!(
        "Scanned {} proposals, found {} reclaimable.",
        scanned,
        closable_proposals.len()
    );

    if closable_proposals.is_empty() {
        println!("No reclaimable rent found.");
        return Ok(());
    }

    for (idx, prop, _, _) in &closable_proposals {
        println!("  Proposal #{idx}: {prop}");
    }

    let batch_size = 5;
    let mut total_reclaimed = 0u64;
    let mut batch_num = 0u64;

    for chunk in closable_proposals.chunks(batch_size) {
        batch_num += 1;
        let mut close_ixs = Vec::new();

        for (_idx, prop_pubkey, tx_pubkey, kind) in chunk {
            close_ixs.push(build_transaction_accounts_close_instruction(
                *program_id,
                *multisig_addr,
                *prop_pubkey,
                *tx_pubkey,
                rent_collector,
                *kind,
            ));
        }

        let prepared = PreparedTransaction {
            instructions: close_ixs,
            description: format!(
                "Close {} proposal accounts (batch {batch_num})",
                chunk.len()
            ),
            review_instructions: vec![],
            extra_signers: vec![],
        };

        match execute_transaction(rpc, signer, prepared, config, dry_run, skip_confirm) {
            Ok(Some(_)) => {
                total_reclaimed += chunk.len() as u64 * 4_000_000;
                eprintln!("Batch {batch_num}: closed {} accounts", chunk.len());
            }
            Ok(None) => {
                total_reclaimed += chunk.len() as u64 * 4_000_000;
            }
            Err(e) => {
                eprintln!("Batch {batch_num} failed: {e}");
            }
        }
    }

    println!();
    let sol_est = format_sol(total_reclaimed);
    if dry_run {
        println!(
            "Dry run: would reclaim ~{sol_est} SOL from {} proposals.",
            closable_proposals.len()
        );
    } else {
        println!(
            "Reclaimed ~{sol_est} SOL from {} proposals.",
            closable_proposals.len()
        );
    }
    Ok(())
}
