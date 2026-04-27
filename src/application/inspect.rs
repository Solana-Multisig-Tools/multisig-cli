use solana_pubkey::Pubkey;

use crate::domain::multisig::{MultisigInfo, TokenBalance, VaultBalances};
use crate::domain::proposal::{ProposalDetail, ProposalSummary, TransactionType};
use crate::domain::transaction::{AccountRef, ConfigTxInfo, InstructionSummary, VaultTxInfo};
use crate::error::MsigError;
use crate::infra::accounts::config_tx::ConfigTransactionAccount;
use crate::infra::accounts::multisig::MultisigAccount;
use crate::infra::accounts::proposal::ProposalAccount;
use crate::infra::accounts::vault_tx::{TransactionMessage, VaultTransactionAccount};
use crate::infra::address_lookup_table;
use crate::infra::instruction::AccountMeta;
use crate::infra::pda;
use crate::infra::rpc::RpcProvider;

/// Fetch full multisig info including vault balance.
pub fn fetch_multisig_info(
    rpc: &dyn RpcProvider,
    multisig_addr: &Pubkey,
    vault_index: u8,
    program_id: &Pubkey,
) -> Result<MultisigInfo, MsigError> {
    let multisig_str = multisig_addr.to_string();
    let program_id_str = program_id.to_string();

    let (vault_pubkey, _) = pda::vault_pda(multisig_addr, vault_index, program_id);
    let vault_str = vault_pubkey.to_string();

    let mut batch = rpc.get_multiple_accounts(&[&multisig_str, &vault_str])?;
    let vault_data = if batch.len() > 1 {
        batch.remove(1)
    } else {
        None
    };
    let account_data = batch
        .into_iter()
        .next()
        .flatten()
        .ok_or_else(|| MsigError::MultisigNotFound(multisig_str.clone()))?;

    if account_data.owner != program_id_str {
        return Err(MsigError::Transaction(format!(
            "account {} not owned by Squads program (owner: {})",
            multisig_str, account_data.owner
        )));
    }

    let ms = MultisigAccount::parse(&account_data.data)?;
    let vault_balance = vault_data.map(|a| a.lamports).unwrap_or(0);

    Ok(MultisigInfo {
        address: *multisig_addr,
        create_key: ms.create_key,
        config_authority: ms.config_authority,
        threshold: ms.threshold,
        time_lock: ms.time_lock,
        transaction_index: ms.transaction_index,
        stale_transaction_index: ms.stale_transaction_index,
        rent_collector: ms.rent_collector,
        bump: ms.bump,
        members: ms.members,
        vault_address: vault_pubkey,
        vault_balance_lamports: vault_balance,
    })
}

/// List recent proposals for a multisig, returning up to `limit` summaries.
pub fn list_proposals(
    rpc: &dyn RpcProvider,
    multisig_addr: &Pubkey,
    limit: u64,
    program_id: &Pubkey,
) -> Result<Vec<ProposalSummary>, MsigError> {
    list_proposals_paged(rpc, multisig_addr, limit, 0, program_id)
}

/// List proposals with pagination: `offset` skips N from the latest end.
pub fn list_proposals_paged(
    rpc: &dyn RpcProvider,
    multisig_addr: &Pubkey,
    limit: u64,
    offset: u64,
    program_id: &Pubkey,
) -> Result<Vec<ProposalSummary>, MsigError> {
    let multisig_str = multisig_addr.to_string();

    let ms_account = rpc
        .get_account_info(&multisig_str)?
        .ok_or_else(|| MsigError::MultisigNotFound(multisig_str.clone()))?;
    let ms = MultisigAccount::parse(&ms_account.data)?;

    if ms.transaction_index == 0 {
        return Ok(vec![]);
    }

    let end = ms.transaction_index.saturating_sub(offset);
    let start = end.saturating_sub(limit).saturating_add(1);

    let mut proposal_addrs: Vec<String> = Vec::new();
    let mut indexes: Vec<u64> = Vec::new();
    for idx in start..=end {
        let (prop_pubkey, _) = pda::proposal_pda(multisig_addr, idx, program_id);
        proposal_addrs.push(prop_pubkey.to_string());
        indexes.push(idx);
    }

    let addr_refs: Vec<&str> = proposal_addrs.iter().map(|s| s.as_str()).collect();
    let accounts = rpc.get_multiple_accounts(&addr_refs)?;

    let mut summaries = Vec::new();
    for (i, maybe_acct) in accounts.into_iter().enumerate() {
        if let Some(acct) = maybe_acct {
            if let Ok(prop) = ProposalAccount::parse(&acct.data) {
                let (prop_pubkey, _) = pda::proposal_pda(multisig_addr, indexes[i], program_id);
                summaries.push(ProposalSummary {
                    index: indexes[i],
                    address: prop_pubkey,
                    status: prop.status,
                    approved_count: prop.approved.len(),
                    rejected_count: prop.rejected.len(),
                    threshold: ms.threshold,
                });
            }
        }
    }

    summaries.reverse();
    Ok(summaries)
}

/// Fetch detailed information about a single proposal by index.
pub fn get_proposal_detail(
    rpc: &dyn RpcProvider,
    multisig_addr: &Pubkey,
    index: u64,
    program_id: &Pubkey,
) -> Result<ProposalDetail, MsigError> {
    let multisig_str = multisig_addr.to_string();
    let (prop_pubkey, _) = pda::proposal_pda(multisig_addr, index, program_id);
    let prop_str = prop_pubkey.to_string();
    let (tx_pubkey, _) = pda::transaction_pda(multisig_addr, index, program_id);
    let tx_str = tx_pubkey.to_string();

    let batch = rpc.get_multiple_accounts(&[&multisig_str, &prop_str, &tx_str])?;
    let mut batch_iter = batch.into_iter();

    let ms_account = batch_iter
        .next()
        .flatten()
        .ok_or_else(|| MsigError::MultisigNotFound(multisig_str.clone()))?;
    let ms = MultisigAccount::parse(&ms_account.data)?;

    let prop_account = batch_iter
        .next()
        .flatten()
        .ok_or(MsigError::ProposalNotFound(index))?;
    let prop = ProposalAccount::parse(&prop_account.data)?;

    let tx_account = batch_iter.next().flatten();

    let mut vault_tx = None;
    let mut config_tx = None;
    let mut transaction_type = TransactionType::Unknown;

    if let Some(tx_acct) = tx_account {
        if let Ok(vtx) = VaultTransactionAccount::parse(&tx_acct.data) {
            transaction_type = TransactionType::Vault;
            let mut instructions = decode_instructions(rpc, &vtx)?;
            resolve_token_amounts(rpc, &mut instructions, &vtx.message);
            vault_tx = Some(VaultTxInfo {
                creator: vtx.creator,
                vault_index: vtx.vault_index,
                instruction_count: vtx.message.instructions.len(),
                instructions,
            });
        } else if let Ok(ctx) = ConfigTransactionAccount::parse(&tx_acct.data) {
            transaction_type = TransactionType::Config;
            config_tx = Some(ConfigTxInfo {
                creator: ctx.creator,
                action_count: ctx.actions.len(),
                actions: ctx.actions,
            });
        }
    }

    let summary = ProposalSummary {
        index,
        address: prop_pubkey,
        status: prop.status,
        approved_count: prop.approved.len(),
        rejected_count: prop.rejected.len(),
        threshold: ms.threshold,
    };

    Ok(ProposalDetail {
        summary,
        multisig: prop.multisig,
        approved: prop.approved,
        rejected: prop.rejected,
        cancelled: prop.cancelled,
        transaction_type,
        vault_tx,
        config_tx,
    })
}

/// Fetch vault SOL balance and known token balances.
pub fn fetch_vault_balances(
    rpc: &dyn RpcProvider,
    multisig_addr: &Pubkey,
    vault_index: u8,
    program_id: &Pubkey,
    config: &crate::infra::config::Config,
) -> Result<VaultBalances, MsigError> {
    let (vault_pubkey, _) = pda::vault_pda(multisig_addr, vault_index, program_id);
    let vault_str = vault_pubkey.to_string();

    let sol_lamports = match rpc.get_account_info(&vault_str)? {
        Some(acct) => acct.lamports,
        None => 0,
    };

    let known_tokens: Vec<(&str, &str)> = vec![
        ("USDC", "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"),
        ("USDT", "Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB"),
    ];

    let mut token_mints: Vec<(String, String)> = known_tokens
        .iter()
        .map(|(sym, mint)| (sym.to_string(), mint.to_string()))
        .collect();

    for (sym, mint) in &config.tokens {
        if mint != "native" && !token_mints.iter().any(|(_, m)| m == mint) {
            token_mints.push((sym.clone(), mint.clone()));
        }
    }

    let mut ata_addrs: Vec<String> = Vec::new();
    let mut ata_info: Vec<(String, String)> = Vec::new();
    for (sym, mint) in &token_mints {
        if let Ok(mk) = mint.parse::<Pubkey>() {
            let ata = pda::derive_ata(&vault_pubkey, &mk);
            ata_addrs.push(ata.to_string());
            ata_info.push((sym.clone(), mint.clone()));
        }
    }

    let mut token_balances = Vec::new();

    if !ata_addrs.is_empty() {
        let refs: Vec<&str> = ata_addrs.iter().map(|s| s.as_str()).collect();
        let accounts = rpc.get_multiple_accounts(&refs)?;

        for (i, maybe_acct) in accounts.into_iter().enumerate() {
            if let Some(acct) = maybe_acct {
                if acct.data.len() >= 72 {
                    let amount = u64::from_le_bytes([
                        acct.data[64],
                        acct.data[65],
                        acct.data[66],
                        acct.data[67],
                        acct.data[68],
                        acct.data[69],
                        acct.data[70],
                        acct.data[71],
                    ]);
                    let decimals = match ata_info[i].0.to_uppercase().as_str() {
                        "USDC" | "USDT" => 6u8,
                        _ => 0,
                    };
                    let ui_amount = format_token_amount(amount, decimals);
                    let mint: Pubkey = ata_info[i].1.parse().map_err(|_| {
                        MsigError::Usage(format!("invalid mint: '{}'", ata_info[i].1))
                    })?;
                    token_balances.push(TokenBalance {
                        mint,
                        symbol: Some(ata_info[i].0.clone()),
                        raw_amount: amount,
                        ui_amount,
                    });
                }
            }
        }
    }

    Ok(VaultBalances {
        vault_address: vault_pubkey,
        sol_lamports,
        token_balances,
    })
}

fn format_token_amount(amount: u64, decimals: u8) -> String {
    if decimals == 0 {
        return amount.to_string();
    }
    let divisor = 10u64.pow(u32::from(decimals));
    let whole = amount / divisor;
    let frac = amount % divisor;
    format!("{whole}.{frac:0>width$}", width = decimals as usize)
}

/// Well-known SPL token mint addresses and their decimals.
fn decimals_for_mint(mint: &str) -> Option<u8> {
    match mint {
        "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v" => Some(6), // USDC
        "Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB" => Some(6), // USDT
        "So11111111111111111111111111111111111111112" => Some(9),  // Wrapped SOL
        _ => None,
    }
}

/// Post-process decoded instructions to resolve raw token amounts into human-readable
/// values. For SPL Token Transfer (disc 3), the mint is unknown from instruction data
/// alone, so we batch-fetch the source ATA accounts to read the mint, then look up
/// decimals from known mints or fetch the mint account on-chain.
fn resolve_token_amounts(
    rpc: &dyn RpcProvider,
    instructions: &mut [InstructionSummary],
    _msg: &crate::infra::accounts::vault_tx::TransactionMessage,
) {
    let token_programs: &[&str] = &[
        "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA",
        "TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb",
    ];

    // Collect indices of Transfer (disc 3) instructions and their source ATA pubkeys.
    let mut transfer_indices: Vec<usize> = Vec::new();
    let mut source_ata_addrs: Vec<String> = Vec::new();

    for (i, ix) in instructions.iter().enumerate() {
        let prog = ix.program_id.to_string();
        if !token_programs.contains(&prog.as_str()) {
            continue;
        }
        if ix.data_hex.starts_with("03") && ix.accounts.len() >= 2 {
            // SPL Token Transfer: disc=3, accounts[0]=source ATA
            transfer_indices.push(i);
            source_ata_addrs.push(ix.accounts[0].address.to_string());
        }
    }

    if transfer_indices.is_empty() {
        return;
    }

    // Batch-fetch source ATA accounts to read the mint field (offset 0, 32 bytes).
    let ata_refs: Vec<&str> = source_ata_addrs.iter().map(|s| s.as_str()).collect();
    let ata_accounts = match rpc.get_multiple_accounts(&ata_refs) {
        Ok(accts) => accts,
        Err(_) => return, // Non-critical: fall back to raw amounts
    };

    // Extract mint pubkeys from each ATA account.
    let mut mint_strs: Vec<Option<String>> = Vec::with_capacity(transfer_indices.len());
    for maybe_acct in &ata_accounts {
        match maybe_acct {
            Some(acct) if acct.data.len() >= 32 => {
                let mut buf = [0u8; 32];
                buf.copy_from_slice(&acct.data[..32]);
                mint_strs.push(Some(Pubkey::from(buf).to_string()));
            }
            _ => mint_strs.push(None),
        }
    }

    // Collect unknown mints that need on-chain fetch for decimals.
    let mut unknown_mint_addrs: Vec<String> = Vec::new();
    for mint_str in mint_strs.iter().flatten() {
        if decimals_for_mint(mint_str).is_none() && !unknown_mint_addrs.contains(mint_str) {
            unknown_mint_addrs.push(mint_str.clone());
        }
    }

    // Batch-fetch unknown mint accounts (decimals at offset 44, 1 byte in SPL Mint layout).
    let mut fetched_decimals: std::collections::HashMap<String, u8> =
        std::collections::HashMap::new();
    if !unknown_mint_addrs.is_empty() {
        let mint_refs: Vec<&str> = unknown_mint_addrs.iter().map(|s| s.as_str()).collect();
        if let Ok(mint_accounts) = rpc.get_multiple_accounts(&mint_refs) {
            for (i, maybe_acct) in mint_accounts.into_iter().enumerate() {
                if let Some(acct) = maybe_acct {
                    if acct.data.len() >= 45 {
                        fetched_decimals.insert(unknown_mint_addrs[i].clone(), acct.data[44]);
                    }
                }
            }
        }
    }

    // Update decoded strings with human-readable amounts.
    for (idx, &ix_idx) in transfer_indices.iter().enumerate() {
        let mint_str = match &mint_strs[idx] {
            Some(s) => s,
            None => continue,
        };

        let decimals =
            decimals_for_mint(mint_str).or_else(|| fetched_decimals.get(mint_str).copied());

        let decimals = match decimals {
            Some(d) => d,
            None => continue,
        };

        // Re-parse the raw amount from the instruction data hex.
        let ix = &instructions[ix_idx];
        let data_bytes = match hex_decode(&ix.data_hex) {
            Some(b) => b,
            None => continue,
        };
        let amount = match read_u64(&data_bytes, 1) {
            Some(a) => a,
            None => continue,
        };

        let formatted = format_token_amount(amount, decimals);

        // Try to identify the token symbol.
        let symbol = match mint_str.as_str() {
            "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v" => " USDC",
            "Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB" => " USDT",
            "So11111111111111111111111111111111111111112" => " SOL",
            _ => "",
        };

        let dest = acct(&instructions[ix_idx].accounts, 1);
        instructions[ix_idx].decoded = Some(format!(
            "Token Transfer {formatted}{symbol} (dec={decimals}) \u{2192} {dest}"
        ));
    }
}

/// Decode hex string back to bytes.
fn hex_decode(hex: &str) -> Option<Vec<u8>> {
    if !hex.len().is_multiple_of(2) {
        return None;
    }
    let mut bytes = Vec::with_capacity(hex.len() / 2);
    for i in (0..hex.len()).step_by(2) {
        bytes.push(u8::from_str_radix(&hex[i..i + 2], 16).ok()?);
    }
    Some(bytes)
}

fn decode_instructions(
    rpc: &dyn RpcProvider,
    vtx: &VaultTransactionAccount,
) -> Result<Vec<InstructionSummary>, MsigError> {
    let msg = &vtx.message;
    let resolved_keys = resolve_message_account_keys(rpc, msg)?;
    let num_keys = resolved_keys.len();

    Ok(vtx
        .message
        .instructions
        .iter()
        .map(|ix| {
            let program_id = resolved_keys
                .get(ix.program_id_index as usize)
                .map(|meta| meta.pubkey)
                .unwrap_or_default();

            let program_name = identify_program(&program_id);

            let accounts: Vec<AccountRef> = ix
                .account_indexes
                .iter()
                .map(|&idx| {
                    let idx = idx as usize;
                    resolved_keys
                        .get(idx)
                        .map(|meta| AccountRef {
                            address: meta.pubkey,
                            is_signer: meta.is_signer,
                            is_writable: meta.is_writable,
                        })
                        .unwrap_or(AccountRef {
                            address: Pubkey::default(),
                            is_signer: false,
                            is_writable: false,
                        })
                })
                .collect();

            let data_hex = hex_encode(&ix.data);
            let decoded = try_decode_instruction(&program_id, &ix.data, &accounts, num_keys);

            InstructionSummary {
                program_id,
                program_name,
                accounts,
                data_hex,
                decoded,
            }
        })
        .collect())
}

fn resolve_message_account_keys(
    rpc: &dyn RpcProvider,
    msg: &TransactionMessage,
) -> Result<Vec<AccountMeta>, MsigError> {
    let num_signers = msg.num_signers as usize;
    let num_writable_signers = msg.num_writable_signers as usize;
    let num_writable_non_signers = msg.num_writable_non_signers as usize;

    let mut keys = Vec::with_capacity(msg.account_keys.len());
    for (idx, key_bytes) in msg.account_keys.iter().enumerate() {
        let is_signer = idx < num_signers;
        let is_writable = if is_signer {
            idx < num_writable_signers
        } else {
            idx < num_signers + num_writable_non_signers
        };
        let pubkey = Pubkey::from(*key_bytes);
        if is_writable {
            keys.push(AccountMeta::new(pubkey, is_signer));
        } else {
            keys.push(AccountMeta::new_readonly(pubkey, is_signer));
        }
    }

    if msg.address_table_lookups.is_empty() {
        return Ok(keys);
    }

    let lookup_table_keys: Vec<Pubkey> = msg
        .address_table_lookups
        .iter()
        .map(|lookup| Pubkey::from(lookup.account_key))
        .collect();
    let lookup_table_strings: Vec<String> =
        lookup_table_keys.iter().map(ToString::to_string).collect();
    let lookup_table_refs: Vec<&str> = lookup_table_strings.iter().map(String::as_str).collect();
    let lookup_accounts = rpc.get_multiple_accounts(&lookup_table_refs)?;

    let mut loaded_writable = Vec::new();
    let mut loaded_readonly = Vec::new();
    for (lookup_idx, lookup) in msg.address_table_lookups.iter().enumerate() {
        let lookup_table = lookup_table_keys[lookup_idx];
        let account = lookup_accounts
            .get(lookup_idx)
            .and_then(Option::as_ref)
            .ok_or_else(|| {
                MsigError::Transaction(format!("address lookup table {lookup_table} not found"))
            })?;
        let addresses = address_lookup_table::parse_addresses(&lookup_table, account)?;

        for index in &lookup.writable_indexes {
            let address = addresses.get(usize::from(*index)).copied().ok_or_else(|| {
                MsigError::Transaction(format!(
                    "address lookup table {lookup_table} does not contain writable index {index}"
                ))
            })?;
            loaded_writable.push(AccountMeta::new(address, false));
        }
        for index in &lookup.readonly_indexes {
            let address = addresses.get(usize::from(*index)).copied().ok_or_else(|| {
                MsigError::Transaction(format!(
                    "address lookup table {lookup_table} does not contain readonly index {index}"
                ))
            })?;
            loaded_readonly.push(AccountMeta::new_readonly(address, false));
        }
    }

    keys.extend(loaded_writable);
    keys.extend(loaded_readonly);
    Ok(keys)
}

/// Format a pubkey as short form: first4..last4.
fn short_addr(pk: &Pubkey) -> String {
    let s = pk.to_string();
    if s.len() > 8 {
        format!("{}..{}", &s[..4], &s[s.len() - 4..])
    } else {
        s
    }
}

/// Get a short address string from the accounts list by index, or "?" if missing.
fn acct(accounts: &[AccountRef], idx: usize) -> String {
    accounts
        .get(idx)
        .map(|a| short_addr(&a.address))
        .unwrap_or_else(|| "?".to_string())
}

/// Read a u32 LE from data at offset.
fn read_u32(data: &[u8], off: usize) -> Option<u32> {
    data.get(off..off + 4)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

/// Read a u64 LE from data at offset.
fn read_u64(data: &[u8], off: usize) -> Option<u64> {
    data.get(off..off + 8)
        .map(|b| u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]))
}

/// Read a Pubkey (32 bytes) from data at offset.
fn read_pubkey(data: &[u8], off: usize) -> Option<Pubkey> {
    if data.len() < off + 32 {
        return None;
    }
    let mut buf = [0u8; 32];
    buf.copy_from_slice(&data[off..off + 32]);
    Some(Pubkey::from(buf))
}

/// Read a length-prefixed seed: u64 length + bytes. Returns (seed_string, bytes_consumed).
fn read_seed(data: &[u8], off: usize) -> Option<(String, usize)> {
    let len = read_u64(data, off)? as usize;
    let start = off + 8;
    if data.len() < start + len {
        return None;
    }
    let s = String::from_utf8_lossy(&data[start..start + len]).to_string();
    Some((s, 8 + len))
}

/// Read an optional pubkey field: u8 tag + Pubkey if tag != 0.
fn read_option_pubkey(data: &[u8], off: usize) -> Option<(Option<Pubkey>, usize)> {
    let tag = *data.get(off)?;
    if tag == 0 {
        Some((None, 1))
    } else {
        let pk = read_pubkey(data, off + 1)?;
        Some((Some(pk), 33))
    }
}

/// Decode a System Program instruction.
fn decode_system_instruction(data: &[u8], accounts: &[AccountRef]) -> Option<String> {
    let disc = read_u32(data, 0)?;
    match disc {
        0 => {
            // CreateAccount: lamports(u64) + space(u64) + owner(Pubkey)
            let lamports = read_u64(data, 4)?;
            let space = read_u64(data, 12)?;
            let owner = read_pubkey(data, 20)
                .map(|p| short_addr(&p))
                .unwrap_or_default();
            let sol = crate::output::format_sol(lamports);
            Some(format!(
                "CreateAccount {sol} SOL, space={space}, owner={owner}, new={}",
                acct(accounts, 1)
            ))
        }
        1 => {
            // Assign: owner(Pubkey)
            let owner = read_pubkey(data, 4)
                .map(|p| short_addr(&p))
                .unwrap_or_default();
            Some(format!(
                "Assign account={} owner={owner}",
                acct(accounts, 0)
            ))
        }
        2 => {
            // Transfer: lamports(u64)
            let lamports = read_u64(data, 4)?;
            let sol = crate::output::format_sol(lamports);
            Some(format!("Transfer {sol} SOL \u{2192} {}", acct(accounts, 1)))
        }
        3 => {
            // CreateAccountWithSeed — show name + lamports
            let seed_result = read_seed(data, 4 + 32); // skip base pubkey
            if let Some((_seed, consumed)) = seed_result {
                let lam_off = 4 + 32 + consumed;
                let lamports = read_u64(data, lam_off).unwrap_or(0);
                let sol = crate::output::format_sol(lamports);
                Some(format!("CreateAccountWithSeed {sol} SOL"))
            } else {
                Some("CreateAccountWithSeed".to_string())
            }
        }
        4 => Some(format!("AdvanceNonceAccount nonce={}", acct(accounts, 0))),
        5 => {
            // WithdrawNonceAccount: lamports(u64)
            let lamports = read_u64(data, 4)?;
            let sol = crate::output::format_sol(lamports);
            Some(format!(
                "WithdrawNonceAccount {sol} SOL to={}",
                acct(accounts, 1)
            ))
        }
        6 => {
            // InitializeNonceAccount: authority(Pubkey)
            let auth = read_pubkey(data, 4)
                .map(|p| short_addr(&p))
                .unwrap_or_default();
            Some(format!("InitializeNonceAccount authority={auth}"))
        }
        7 => {
            // AuthorizeNonceAccount: new_authority(Pubkey)
            let auth = read_pubkey(data, 4)
                .map(|p| short_addr(&p))
                .unwrap_or_default();
            Some(format!("AuthorizeNonceAccount new_authority={auth}"))
        }
        8 => {
            // Allocate: space(u64)
            let space = read_u64(data, 4)?;
            Some(format!(
                "Allocate space={space}, account={}",
                acct(accounts, 0)
            ))
        }
        9 => Some("AllocateWithSeed".to_string()),
        10 => Some("AssignWithSeed".to_string()),
        11 => {
            // TransferWithSeed: lamports(u64) + from_seed_len + seed + from_owner
            let lamports = read_u64(data, 4).unwrap_or(0);
            let sol = crate::output::format_sol(lamports);
            Some(format!("TransferWithSeed {sol} SOL"))
        }
        12 => Some("UpgradeNonceAccount".to_string()),
        _ => None,
    }
}

/// Decode an SPL Token Program instruction.
fn decode_token_instruction(data: &[u8], accounts: &[AccountRef]) -> Option<String> {
    let disc = *data.first()?;
    match disc {
        0 => {
            // InitializeMint: decimals(u8) + mint_authority(Pubkey) + freeze_authority_option
            let decimals = data.get(1).copied().unwrap_or(0);
            let auth = read_pubkey(data, 2)
                .map(|p| short_addr(&p))
                .unwrap_or_default();
            Some(format!(
                "InitializeMint decimals={decimals}, authority={auth}"
            ))
        }
        1 => Some(format!(
            "InitializeAccount mint={}, owner={}",
            acct(accounts, 1),
            acct(accounts, 2)
        )),
        2 => {
            let m = data.get(1).copied().unwrap_or(0);
            Some(format!("InitializeMultisig m={m}"))
        }
        3 => {
            // Transfer: amount(u64)
            let amount = read_u64(data, 1)?;
            Some(format!(
                "Token Transfer {amount} (raw) \u{2192} {}",
                acct(accounts, 1)
            ))
        }
        4 => {
            // Approve: amount(u64)
            let amount = read_u64(data, 1)?;
            Some(format!(
                "Approve {amount} (raw) delegate={}, authority={}",
                acct(accounts, 1),
                acct(accounts, 2)
            ))
        }
        5 => Some(format!(
            "Revoke source={}, authority={}",
            acct(accounts, 0),
            acct(accounts, 1)
        )),
        6 => {
            // SetAuthority: authority_type(u8) + new_authority_option(u8 + Pubkey?)
            let auth_type = data.get(1).copied().unwrap_or(0);
            let type_name = match auth_type {
                0 => "MintTokens",
                1 => "FreezeAccount",
                2 => "AccountOwner",
                3 => "CloseAccount",
                _ => "Unknown",
            };
            let new_auth =
                read_option_pubkey(data, 2).and_then(|(opt_pk, _)| opt_pk.map(|p| short_addr(&p)));
            match new_auth {
                Some(addr) => Some(format!("SetAuthority type={type_name}, new={addr}")),
                None => Some(format!("SetAuthority type={type_name}, new=None")),
            }
        }
        7 => {
            let amount = read_u64(data, 1)?;
            Some(format!("MintTo {amount} (raw) dest={}", acct(accounts, 1)))
        }
        8 => {
            let amount = read_u64(data, 1)?;
            Some(format!("Burn {amount} (raw) source={}", acct(accounts, 0)))
        }
        9 => Some(format!(
            "CloseAccount account={}, dest={}, authority={}",
            acct(accounts, 0),
            acct(accounts, 1),
            acct(accounts, 2)
        )),
        10 => Some(format!("FreezeAccount account={}", acct(accounts, 0))),
        11 => Some(format!("ThawAccount account={}", acct(accounts, 0))),
        12 => {
            // TransferChecked: amount(u64) + decimals(u8)
            let amount = read_u64(data, 1)?;
            let decimals = data.get(9).copied().unwrap_or(0);
            let formatted = format_token_amount(amount, decimals);
            Some(format!(
                "TransferChecked {formatted} (dec={decimals}) src={}, mint={}, dest={}",
                acct(accounts, 0),
                acct(accounts, 1),
                acct(accounts, 2)
            ))
        }
        13 => {
            // ApproveChecked: amount(u64) + decimals(u8)
            let amount = read_u64(data, 1)?;
            let decimals = data.get(9).copied().unwrap_or(0);
            let formatted = format_token_amount(amount, decimals);
            Some(format!("ApproveChecked {formatted} (dec={decimals})"))
        }
        14 => {
            // MintToChecked: amount(u64) + decimals(u8)
            let amount = read_u64(data, 1)?;
            let decimals = data.get(9).copied().unwrap_or(0);
            let formatted = format_token_amount(amount, decimals);
            Some(format!(
                "MintToChecked {formatted} (dec={decimals}) dest={}",
                acct(accounts, 1)
            ))
        }
        15 => {
            // BurnChecked: amount(u64) + decimals(u8)
            let amount = read_u64(data, 1)?;
            let decimals = data.get(9).copied().unwrap_or(0);
            let formatted = format_token_amount(amount, decimals);
            Some(format!(
                "BurnChecked {formatted} (dec={decimals}) source={}",
                acct(accounts, 0)
            ))
        }
        16 => {
            // InitializeAccount2: owner(Pubkey)
            let owner = read_pubkey(data, 1)
                .map(|p| short_addr(&p))
                .unwrap_or_default();
            Some(format!("InitializeAccount2 owner={owner}"))
        }
        17 => Some("SyncNative".to_string()),
        18 => {
            // InitializeAccount3: owner(Pubkey)
            let owner = read_pubkey(data, 1)
                .map(|p| short_addr(&p))
                .unwrap_or_default();
            Some(format!("InitializeAccount3 owner={owner}"))
        }
        19 => {
            let m = data.get(1).copied().unwrap_or(0);
            Some(format!("InitializeMultisig2 m={m}"))
        }
        20 => {
            // InitializeMint2: decimals(u8) + mint_authority(Pubkey) + freeze_authority_option
            let decimals = data.get(1).copied().unwrap_or(0);
            let auth = read_pubkey(data, 2)
                .map(|p| short_addr(&p))
                .unwrap_or_default();
            Some(format!(
                "InitializeMint2 decimals={decimals}, authority={auth}"
            ))
        }
        25 => {
            // InitializeMintCloseAuthority: close_authority_option(u8 + Pubkey?)
            let close_auth =
                read_option_pubkey(data, 1).and_then(|(opt_pk, _)| opt_pk.map(|p| short_addr(&p)));
            match close_auth {
                Some(addr) => Some(format!("InitializeMintCloseAuthority authority={addr}")),
                None => Some("InitializeMintCloseAuthority authority=None".to_string()),
            }
        }
        _ => None,
    }
}

/// Decode a BPF Upgradeable Loader instruction.
fn decode_bpf_loader_instruction(data: &[u8], accounts: &[AccountRef]) -> Option<String> {
    let disc = read_u32(data, 0)?;
    match disc {
        0 => Some("InitializeBuffer".to_string()),
        1 => {
            let offset = read_u32(data, 4).unwrap_or(0);
            let length = read_u32(data, 8).unwrap_or(0);
            Some(format!("Write offset={offset}, length={length}"))
        }
        2 => {
            let max_data_len = read_u64(data, 4).unwrap_or(0);
            Some(format!("DeployWithMaxDataLen max_data_len={max_data_len}"))
        }
        3 => Some(format!(
            "Upgrade program={}, buffer={}",
            acct(accounts, 1),
            acct(accounts, 2)
        )),
        4 => Some(format!("SetAuthority account={}", acct(accounts, 0))),
        5 => Some(format!(
            "Close account={}, recipient={}",
            acct(accounts, 0),
            acct(accounts, 1)
        )),
        _ => None,
    }
}

/// Decode an Associated Token Account Program instruction.
fn decode_ata_instruction(data: &[u8], accounts: &[AccountRef]) -> Option<String> {
    // ATA program: empty data or disc 0 = Create, disc 1 = CreateIdempotent
    if data.is_empty() {
        return Some(format!(
            "CreateATA wallet={}, mint={}",
            acct(accounts, 1),
            acct(accounts, 3)
        ));
    }
    let disc = data[0];
    if disc == 0 {
        Some(format!(
            "CreateATA wallet={}, mint={}",
            acct(accounts, 1),
            acct(accounts, 3)
        ))
    } else if disc == 1 {
        Some(format!(
            "CreateIdempotentATA wallet={}, mint={}",
            acct(accounts, 1),
            acct(accounts, 3)
        ))
    } else {
        None
    }
}

/// Decode a Compute Budget Program instruction.
fn decode_compute_budget_instruction(data: &[u8], _accounts: &[AccountRef]) -> Option<String> {
    let disc = *data.first()?;
    match disc {
        2 => {
            let units = read_u32(data, 1)?;
            Some(format!("SetComputeUnitLimit units={units}"))
        }
        3 => {
            let microlamports = read_u64(data, 1)?;
            Some(format!("SetComputeUnitPrice {microlamports} microlamports"))
        }
        _ => None,
    }
}

/// Attempt to decode well-known instruction types into a human-readable string.
fn try_decode_instruction(
    program_id: &Pubkey,
    data: &[u8],
    accounts: &[AccountRef],
    _num_keys: usize,
) -> Option<String> {
    let prog = program_id.to_string();
    match prog.as_str() {
        "11111111111111111111111111111111" => decode_system_instruction(data, accounts),
        "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA"
        | "TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb" => decode_token_instruction(data, accounts),
        "BPFLoaderUpgradeab1e11111111111111111111111" => {
            decode_bpf_loader_instruction(data, accounts)
        }
        "ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL" => decode_ata_instruction(data, accounts),
        "ComputeBudget111111111111111111111111111111" => {
            decode_compute_budget_instruction(data, accounts)
        }
        _ => None,
    }
}

fn identify_program(program_id: &Pubkey) -> String {
    let s = program_id.to_string();
    match s.as_str() {
        "11111111111111111111111111111111" => "System".to_string(),
        "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA" => "Token".to_string(),
        "TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb" => "Token-2022".to_string(),
        "ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL" => "ATA".to_string(),
        "BPFLoaderUpgradeab1e11111111111111111111111" => "BPFLoader".to_string(),
        "ComputeBudget111111111111111111111111111111" => "ComputeBudget".to_string(),
        "SQDS4ep65T869zMMBKyuUq6aD6EgTu8psMjkvj52pCf" => "SquadsV4".to_string(),
        _ => {
            if s.len() > 8 {
                format!("{}...{}", &s[..4], &s[s.len() - 4..])
            } else {
                s
            }
        }
    }
}

fn hex_encode(data: &[u8]) -> String {
    let mut s = String::with_capacity(data.len() * 2);
    for b in data {
        use std::fmt::Write;
        let _ = write!(s, "{b:02x}");
    }
    s
}
