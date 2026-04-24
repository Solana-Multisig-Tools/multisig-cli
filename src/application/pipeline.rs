use base64::Engine;
use sha2::{Digest, Sha256};
use solana_pubkey::Pubkey;
use zeroize::Zeroizing;

use crate::error::MsigError;
use crate::infra::config::Config;
use crate::infra::instruction::Instruction;
use crate::infra::rpc::RpcProvider;
use crate::infra::signer::Signer;

const COMPUTE_BUDGET_PROGRAM: Pubkey =
    solana_pubkey::pubkey!("ComputeBudget111111111111111111111111111111");
const MAX_ACCOUNT_KEYS: usize = 256;
const MAX_SIGNATURES: usize = u8::MAX as usize;
const MAX_COMPILED_INSTRUCTION_ACCOUNTS: usize = 256;
const MAX_SHORT_VEC_ITEMS: usize = u16::MAX as usize;
const MAX_VAULT_MESSAGE_ITEMS: usize = u8::MAX as usize;
const MAX_U16_BYTES: usize = u16::MAX as usize;

pub struct PreparedTransaction {
    pub instructions: Vec<Instruction>,
    pub description: String,
    pub review_instructions: Vec<Instruction>,
    pub extra_signers: Vec<Zeroizing<[u8; 64]>>,
}

pub struct TransactionSummary {
    pub description: String,
    pub instructions: Vec<InstructionDisplay>,
    pub review_instructions: Vec<InstructionDisplay>,
    pub estimated_fee: u64,
}

pub struct InstructionDisplay {
    pub program: String,
    pub accounts_count: usize,
    pub data_len: usize,
    pub decoded: Option<String>,
}

pub fn message_hash_base58(message: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(message);
    bs58::encode(hasher.finalize()).into_string()
}

/// Returns a stack-allocated compact-u16 encoding (1-3 bytes).
fn encode_compact_u16(value: u16) -> ([u8; 3], usize) {
    let mut buf = [0u8; 3];
    let mut val = value;
    let mut len = 0;
    loop {
        let mut elem = (val & 0x7f) as u8;
        val >>= 7;
        if val != 0 {
            elem |= 0x80;
        }
        buf[len] = elem;
        len += 1;
        if val == 0 {
            break;
        }
    }
    (buf, len)
}

fn checked_u16_len(len: usize, what: &str) -> Result<u16, MsigError> {
    u16::try_from(len)
        .map_err(|_| MsigError::Transaction(format!("{what} length {len} exceeds u16::MAX")))
}

fn checked_u8_len(len: usize, what: &str) -> Result<u8, MsigError> {
    u8::try_from(len)
        .map_err(|_| MsigError::Transaction(format!("{what} length {len} exceeds u8::MAX")))
}

fn checked_account_key_count(len: usize) -> Result<u16, MsigError> {
    if len > MAX_ACCOUNT_KEYS {
        return Err(MsigError::Transaction(format!(
            "transaction references {len} account keys; max supported is {MAX_ACCOUNT_KEYS}"
        )));
    }
    checked_u16_len(len, "account keys")
}

#[derive(Clone, Copy)]
struct AccountEntry {
    key: Pubkey,
    is_signer: bool,
    is_writable: bool,
}

fn upsert_account(
    accounts: &mut Vec<AccountEntry>,
    key: Pubkey,
    is_signer: bool,
    is_writable: bool,
) {
    if let Some(existing) = accounts.iter_mut().find(|entry| entry.key == key) {
        existing.is_signer |= is_signer;
        existing.is_writable |= is_writable;
    } else {
        accounts.push(AccountEntry {
            key,
            is_signer,
            is_writable,
        });
    }
}

fn push_compact_len(buf: &mut Vec<u8>, len: usize, what: &str) -> Result<(), MsigError> {
    let len = checked_u16_len(len, what)?;
    let (b, l) = encode_compact_u16(len);
    buf.extend_from_slice(&b[..l]);
    Ok(())
}

fn read_compact_len(buf: &[u8], offset: &mut usize, what: &str) -> Result<usize, MsigError> {
    let mut len = 0usize;
    let mut shift = 0usize;
    for _ in 0..3 {
        let byte = *buf
            .get(*offset)
            .ok_or_else(|| MsigError::Transaction(format!("{what} compact length is truncated")))?;
        *offset += 1;
        len |= ((byte & 0x7f) as usize) << shift;
        if byte & 0x80 == 0 {
            return Ok(len);
        }
        shift += 7;
    }
    Err(MsigError::Transaction(format!(
        "{what} compact length exceeds u16::MAX"
    )))
}

fn required_signers_from_message(message: &[u8]) -> Result<Vec<Pubkey>, MsigError> {
    if message.len() < 3 {
        return Err(MsigError::Transaction(
            "message is too short to contain a Solana header".into(),
        ));
    }
    if message[0] & 0x80 != 0 {
        return Err(MsigError::Transaction(
            "versioned messages are not supported by this transaction pipeline".into(),
        ));
    }

    let required_signatures = message[0] as usize;
    let mut offset = 3usize;
    let account_count = read_compact_len(message, &mut offset, "account keys")?;
    if account_count < required_signatures {
        return Err(MsigError::Transaction(format!(
            "message requires {required_signatures} signatures but only has {account_count} account keys"
        )));
    }

    let keys_len = account_count
        .checked_mul(32)
        .ok_or_else(|| MsigError::Transaction("message account key length overflow".into()))?;
    if message.len().saturating_sub(offset) < keys_len {
        return Err(MsigError::Transaction(
            "message is truncated before account keys".into(),
        ));
    }

    let mut signers = Vec::with_capacity(required_signatures);
    for i in 0..required_signatures {
        let start = offset + i * 32;
        let mut bytes = [0u8; 32];
        bytes.copy_from_slice(&message[start..start + 32]);
        signers.push(Pubkey::from(bytes));
    }
    Ok(signers)
}

fn extra_signer_pubkey(keypair_bytes: &[u8; 64]) -> Result<Pubkey, MsigError> {
    let secret_arr = <&[u8; 32]>::try_from(&keypair_bytes[..32])
        .map_err(|_| MsigError::Signing("invalid extra signer secret key".into()))?;
    let stored_pubkey = <[u8; 32]>::try_from(&keypair_bytes[32..64])
        .map_err(|_| MsigError::Signing("invalid extra signer public key".into()))?;
    let signing_key = ed25519_dalek::SigningKey::from_bytes(secret_arr);
    let derived_pubkey = signing_key.verifying_key().to_bytes();
    if derived_pubkey != stored_pubkey {
        return Err(MsigError::Signing(
            "extra signer public key does not match secret key".into(),
        ));
    }
    Ok(Pubkey::from(derived_pubkey))
}

fn validate_signer_order(
    message: &[u8],
    payer: &Pubkey,
    extra_signers: &[Zeroizing<[u8; 64]>],
) -> Result<Vec<Pubkey>, MsigError> {
    let required_signers = required_signers_from_message(message)?;
    let extra_pubkeys = extra_signers
        .iter()
        .map(|keypair| extra_signer_pubkey(keypair))
        .collect::<Result<Vec<_>, _>>()?;

    let expected = 1 + extra_pubkeys.len();
    if required_signers.len() != expected {
        return Err(MsigError::Signing(format!(
            "message requires {} signatures but {} signer(s) were prepared",
            required_signers.len(),
            expected
        )));
    }

    match required_signers.first() {
        Some(first) if first == payer => {}
        Some(first) => {
            return Err(MsigError::Signing(format!(
                "fee payer signer mismatch: message expects {first}, signer is {payer}"
            )));
        }
        None => {
            return Err(MsigError::Signing(
                "message does not require a fee payer signature".into(),
            ));
        }
    }

    for (idx, expected_pubkey) in extra_pubkeys.iter().enumerate() {
        let signer_idx = idx + 1;
        match required_signers.get(signer_idx) {
            Some(actual) if actual == expected_pubkey => {}
            Some(actual) => {
                return Err(MsigError::Signing(format!(
                    "extra signer #{idx} mismatch: message expects {actual}, prepared {expected_pubkey}"
                )));
            }
            None => {
                return Err(MsigError::Signing(format!(
                    "message is missing prepared extra signer #{idx}"
                )));
            }
        }
    }

    Ok(required_signers)
}

fn build_message(
    instructions: &[Instruction],
    payer: &Pubkey,
    blockhash: &[u8; 32],
) -> Result<Vec<u8>, MsigError> {
    use std::collections::HashMap;

    if instructions.len() > MAX_SHORT_VEC_ITEMS {
        return Err(MsigError::Transaction(format!(
            "transaction has {} instructions; max supported is {MAX_SHORT_VEC_ITEMS}",
            instructions.len()
        )));
    }

    let mut accounts = Vec::new();
    upsert_account(&mut accounts, *payer, true, true);

    for (ix_idx, ix) in instructions.iter().enumerate() {
        if ix.accounts.len() > MAX_COMPILED_INSTRUCTION_ACCOUNTS {
            return Err(MsigError::Transaction(format!(
                "instruction #{ix_idx} references {} accounts; max supported is {MAX_COMPILED_INSTRUCTION_ACCOUNTS}",
                ix.accounts.len()
            )));
        }
        if ix.data.len() > MAX_SHORT_VEC_ITEMS {
            return Err(MsigError::Transaction(format!(
                "instruction #{ix_idx} data is {} bytes; max supported is {MAX_SHORT_VEC_ITEMS}",
                ix.data.len()
            )));
        }
        for meta in &ix.accounts {
            upsert_account(&mut accounts, meta.pubkey, meta.is_signer, meta.is_writable);
        }
        upsert_account(&mut accounts, ix.program_id, false, false);
    }

    let mut signer_writable: Vec<Pubkey> = Vec::new();
    let mut signer_readonly: Vec<Pubkey> = Vec::new();
    let mut nonsigner_writable: Vec<Pubkey> = Vec::new();
    let mut nonsigner_readonly: Vec<Pubkey> = Vec::new();

    for entry in &accounts {
        if entry.key == *payer {
            continue;
        }

        if entry.is_signer && entry.is_writable {
            signer_writable.push(entry.key);
        } else if entry.is_signer {
            signer_readonly.push(entry.key);
        } else if entry.is_writable {
            nonsigner_writable.push(entry.key);
        } else {
            nonsigner_readonly.push(entry.key);
        }
    }

    signer_writable.sort_unstable();
    signer_readonly.sort_unstable();
    nonsigner_writable.sort_unstable();
    nonsigner_readonly.sort_unstable();

    let required_signatures = 1 + signer_writable.len() + signer_readonly.len();
    if required_signatures > MAX_SIGNATURES {
        return Err(MsigError::Transaction(format!(
            "transaction requires {required_signatures} signatures; max supported is {MAX_SIGNATURES}"
        )));
    }

    let num_required_signatures = checked_u8_len(required_signatures, "required signatures")?;
    let num_readonly_signed = checked_u8_len(signer_readonly.len(), "readonly signer count")?;
    let num_readonly_unsigned = checked_u8_len(nonsigner_readonly.len(), "readonly account count")?;

    let mut ordered_keys: Vec<Pubkey> = Vec::new();
    ordered_keys.push(*payer);
    ordered_keys.extend_from_slice(&signer_writable);
    ordered_keys.extend_from_slice(&signer_readonly);
    ordered_keys.extend_from_slice(&nonsigner_writable);
    ordered_keys.extend_from_slice(&nonsigner_readonly);

    checked_account_key_count(ordered_keys.len())?;

    let key_map: HashMap<Pubkey, u8> = ordered_keys
        .iter()
        .enumerate()
        .map(|(i, k)| (*k, i as u8))
        .collect();

    let mut msg = Vec::new();
    msg.push(num_required_signatures);
    msg.push(num_readonly_signed);
    msg.push(num_readonly_unsigned);

    push_compact_len(&mut msg, ordered_keys.len(), "account keys")?;
    for key in &ordered_keys {
        msg.extend_from_slice(key.as_ref());
    }
    msg.extend_from_slice(blockhash);

    push_compact_len(&mut msg, instructions.len(), "instructions")?;
    for ix in instructions {
        let program_index = key_map
            .get(&ix.program_id)
            .copied()
            .ok_or_else(|| MsigError::Transaction("program id missing from account keys".into()))?;
        msg.push(program_index);
        push_compact_len(&mut msg, ix.accounts.len(), "instruction accounts")?;
        for meta in &ix.accounts {
            let account_index = key_map.get(&meta.pubkey).copied().ok_or_else(|| {
                MsigError::Transaction("instruction account missing from account keys".into())
            })?;
            msg.push(account_index);
        }
        push_compact_len(&mut msg, ix.data.len(), "instruction data")?;
        msg.extend_from_slice(&ix.data);
    }

    Ok(msg)
}

fn padded_compute_unit_limit(units_consumed: u64) -> u32 {
    let padded = units_consumed.saturating_mul(11).saturating_add(9) / 10;
    let bounded = padded.max(10_000).min(u64::from(u32::MAX));
    bounded as u32
}

/// Compile a deterministic legacy Solana message for external signing.
pub fn compile_legacy_message(
    instructions: &[Instruction],
    payer: &Pubkey,
    recent_blockhash: &str,
) -> Result<Vec<u8>, MsigError> {
    let blockhash_bytes: [u8; 32] = bs58::decode(recent_blockhash)
        .into_vec()
        .map_err(|e| MsigError::Transaction(format!("invalid blockhash: {e}")))?
        .try_into()
        .map_err(|_| MsigError::Transaction("blockhash not 32 bytes".into()))?;
    build_message(instructions, payer, &blockhash_bytes)
}

pub fn compile_unsigned_legacy_transaction_base64(
    instructions: &[Instruction],
    payer: &Pubkey,
    recent_blockhash: &str,
) -> Result<String, MsigError> {
    let message = compile_legacy_message(instructions, payer, recent_blockhash)?;
    let required_signers = required_signers_from_message(&message)?;
    let wire = build_unsigned_wire_tx(&message, required_signers.len())?;
    Ok(base64::engine::general_purpose::STANDARD.encode(wire))
}

fn build_unsigned_wire_tx(message: &[u8], num_signers: usize) -> Result<Vec<u8>, MsigError> {
    if num_signers > MAX_SIGNATURES {
        return Err(MsigError::Transaction(format!(
            "transaction has {num_signers} signers; max supported is {MAX_SIGNATURES}"
        )));
    }
    let mut wire = Vec::new();
    push_compact_len(&mut wire, num_signers, "signatures")?;
    for _ in 0..num_signers {
        wire.extend_from_slice(&[0u8; 64]);
    }
    wire.extend_from_slice(message);
    Ok(wire)
}

/// The pipeline every write operation goes through.
/// Returns `Some(signature)` on success, `None` on dry-run.
/// Execute a prepared transaction through the full pipeline.
/// When `quiet` is true, suppresses interactive output (for JSON mode).
pub fn execute_transaction(
    rpc: &dyn RpcProvider,
    signer: &dyn Signer,
    prepared: PreparedTransaction,
    config: &Config,
    dry_run: bool,
    skip_confirm: bool,
) -> Result<Option<String>, MsigError> {
    execute_transaction_inner(rpc, signer, prepared, config, dry_run, skip_confirm, false)
}

/// Same as execute_transaction but with quiet mode for JSON output.
pub fn execute_transaction_quiet(
    rpc: &dyn RpcProvider,
    signer: &dyn Signer,
    prepared: PreparedTransaction,
    config: &Config,
    dry_run: bool,
    skip_confirm: bool,
) -> Result<Option<String>, MsigError> {
    execute_transaction_inner(rpc, signer, prepared, config, dry_run, skip_confirm, true)
}

fn execute_transaction_inner(
    rpc: &dyn RpcProvider,
    signer: &dyn Signer,
    prepared: PreparedTransaction,
    config: &Config,
    dry_run: bool,
    skip_confirm: bool,
    quiet: bool,
) -> Result<Option<String>, MsigError> {
    let description = prepared.description;
    let payer = signer.pubkey();

    // Phase 1: Simulate
    let (blockhash_b58, _) = rpc.get_latest_blockhash()?;
    let blockhash_bytes: [u8; 32] = bs58::decode(&blockhash_b58)
        .into_vec()
        .map_err(|e| MsigError::Transaction(format!("invalid blockhash: {e}")))?
        .try_into()
        .map_err(|_| MsigError::Transaction("blockhash not 32 bytes".into()))?;

    let sim_message = build_message(&prepared.instructions, &payer, &blockhash_bytes)?;
    let sim_required_signers = required_signers_from_message(&sim_message)?;
    let sim_wire = build_unsigned_wire_tx(&sim_message, sim_required_signers.len())?;
    let sim_b64 = base64::engine::general_purpose::STANDARD.encode(&sim_wire);

    let sim_result = rpc.simulate_transaction(&sim_b64)?;
    if let Some(ref err) = sim_result.err {
        if !sim_result.logs.is_empty() {
            eprintln!("Simulation logs:");
            for log in &sim_result.logs {
                eprintln!("  {log}");
            }
        }
        return Err(MsigError::SimulationFailed(format!(
            "simulation error: {err}"
        )));
    }

    let units_consumed = sim_result.units_consumed;

    // Phase 2: Rebuild with ComputeBudget
    let mut final_instructions = Vec::new();
    if config.priority_fee > 0 {
        let cu_limit = padded_compute_unit_limit(units_consumed);
        let mut cu_limit_data = Vec::with_capacity(5);
        cu_limit_data.push(2u8);
        cu_limit_data.extend_from_slice(&cu_limit.to_le_bytes());
        final_instructions.push(Instruction {
            program_id: COMPUTE_BUDGET_PROGRAM,
            accounts: vec![],
            data: cu_limit_data,
        });

        let mut cu_price_data = Vec::with_capacity(9);
        cu_price_data.push(3u8);
        cu_price_data.extend_from_slice(&config.priority_fee.to_le_bytes());
        final_instructions.push(Instruction {
            program_id: COMPUTE_BUDGET_PROGRAM,
            accounts: vec![],
            data: cu_price_data,
        });
    }
    final_instructions.extend(prepared.instructions);

    let (blockhash_b58, _) = rpc.get_latest_blockhash()?;
    let blockhash_bytes: [u8; 32] = bs58::decode(&blockhash_b58)
        .into_vec()
        .map_err(|e| MsigError::Transaction(format!("invalid blockhash: {e}")))?
        .try_into()
        .map_err(|_| MsigError::Transaction("blockhash not 32 bytes".into()))?;

    let message_bytes = build_message(&final_instructions, &payer, &blockhash_bytes)?;
    let required_signers = validate_signer_order(&message_bytes, &payer, &prepared.extra_signers)?;

    // Phase 3: Display and confirm before any signer approval.
    if !quiet || signer.requires_device_review() {
        let summary = build_summary(
            &description,
            &final_instructions,
            &prepared.review_instructions,
            units_consumed,
        );
        display_summary(&summary);
        display_message_hash(&message_bytes);
    }

    if dry_run {
        if !quiet || signer.requires_device_review() {
            eprintln!("Dry run: transaction simulated successfully");
        }
        return Ok(None);
    }

    if !skip_confirm && (!quiet || signer.requires_device_review()) {
        eprint!("Confirm? [y/N] ");
        let mut input = String::new();
        std::io::stdin()
            .read_line(&mut input)
            .map_err(MsigError::Io)?;
        let trimmed = input.trim().to_lowercase();
        if trimmed != "y" && trimmed != "yes" {
            return Err(MsigError::Cancelled);
        }
    }

    // Phase 4: Sign
    let sig_bytes = signer.sign(&message_bytes)?;
    let sig_bytes_arr: [u8; 64] = sig_bytes
        .try_into()
        .map_err(|_| MsigError::Signing("signature is not 64 bytes".into()))?;

    let mut extra_sigs: Vec<[u8; 64]> = Vec::new();
    for keypair_bytes in &prepared.extra_signers {
        let secret_arr = <&[u8; 32]>::try_from(&keypair_bytes[..32])
            .map_err(|_| MsigError::Signing("invalid extra signer key".into()))?;
        let secret = ed25519_dalek::SigningKey::from_bytes(secret_arr);
        let sig: ed25519_dalek::Signature = ed25519_dalek::Signer::sign(&secret, &message_bytes);
        extra_sigs.push(sig.to_bytes());
    }

    // Phase 5: Wire format
    let total_sigs = required_signers.len();
    let mut wire_tx = Vec::new();
    {
        push_compact_len(&mut wire_tx, total_sigs, "signatures")?;
    }
    wire_tx.extend_from_slice(&sig_bytes_arr);
    for sig in &extra_sigs {
        wire_tx.extend_from_slice(sig);
    }
    wire_tx.extend_from_slice(&message_bytes);

    let tx_base64 = base64::engine::general_purpose::STANDARD.encode(&wire_tx);

    // Phase 6: Send and confirm
    let signature = rpc.send_transaction(&tx_base64)?;
    if !quiet {
        println!("Transaction sent: {signature}");
    }

    if !quiet {
        eprintln!("Waiting for confirmation...");
    }
    let commitment = &config.commitment;
    let start = std::time::Instant::now();
    let timeout = std::time::Duration::from_secs(60);
    let mut delay = std::time::Duration::from_millis(500);
    let max_delay = std::time::Duration::from_secs(4);

    loop {
        let statuses = rpc.get_signature_statuses(&[&signature])?;
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
            let confirmed = match commitment.as_str() {
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
                if !quiet {
                    eprintln!("Confirmed.");
                }
                return Ok(Some(signature));
            }
        }

        if start.elapsed() >= timeout {
            if !quiet {
                eprintln!("Warning: confirmation timed out after 60s. Transaction may still land.");
            }
            return Ok(Some(signature));
        }

        std::thread::sleep(delay);
        delay = (delay * 2).min(max_delay);
    }
}

fn build_summary(
    description: &str,
    ixs: &[Instruction],
    review_ixs: &[Instruction],
    units_consumed: u64,
) -> TransactionSummary {
    let instructions = ixs.iter().map(build_instruction_display).collect();
    let review_instructions = review_ixs.iter().map(build_instruction_display).collect();

    let estimated_fee = 5000 + units_consumed / 1000;
    TransactionSummary {
        description: description.to_string(),
        instructions,
        review_instructions,
        estimated_fee,
    }
}

fn display_summary(summary: &TransactionSummary) {
    eprintln!();
    eprintln!("  {}", summary.description);
    eprintln!();

    if summary.review_instructions.is_empty() {
        for (i, ix) in summary.instructions.iter().enumerate() {
            display_instruction("Instruction", i, ix);
        }
    } else {
        eprintln!("  Vault instructions:");
        for (i, ix) in summary.review_instructions.iter().enumerate() {
            display_instruction("Inner instruction", i, ix);
        }
        eprintln!(
            "  Squads wrapper: {} setup instruction(s)",
            summary.instructions.len()
        );
    }
    eprintln!("  Estimated fee: {} lamports", summary.estimated_fee);
    eprintln!();
}

fn display_instruction(prefix: &str, index: usize, ix: &InstructionDisplay) {
    let decoded_str = ix.decoded.as_deref().unwrap_or("unknown");
    eprintln!(
        "    {} {}: {} (program {}, {} accounts, {} bytes data)",
        prefix,
        index + 1,
        decoded_str,
        ix.program,
        ix.accounts_count,
        ix.data_len
    );
}

fn display_message_hash(message: &[u8]) {
    eprintln!("  Message Hash: {}", message_hash_base58(message));
    eprintln!("  Ledger blind signing should show this same Message Hash.");
    eprintln!();
}

fn build_instruction_display(ix: &Instruction) -> InstructionDisplay {
    InstructionDisplay {
        program: identify_program(&ix.program_id),
        accounts_count: ix.accounts.len(),
        data_len: ix.data.len(),
        decoded: decode_instruction_name(ix),
    }
}

fn decode_instruction_name(ix: &Instruction) -> Option<String> {
    decode_known_program_instruction(ix).or_else(|| decode_squads_instruction_name(&ix.data))
}

fn decode_squads_instruction_name(data: &[u8]) -> Option<String> {
    if data.len() < 8 {
        return None;
    }
    let mut disc = [0u8; 8];
    disc.copy_from_slice(&data[..8]);
    match disc {
        [0x90, 0x25, 0xa4, 0x88, 0xbc, 0xd8, 0x2a, 0xf8] => Some("proposal_approve".into()),
        [0xf3, 0x3e, 0x86, 0x9c, 0xe6, 0x6a, 0xf6, 0x87] => Some("proposal_reject".into()),
        [0xdc, 0x3c, 0x49, 0xe0, 0x1e, 0x6c, 0x4f, 0x9f] => Some("proposal_create".into()),
        [0x0b, 0x22, 0x5c, 0xf8, 0x9a, 0x1b, 0x33, 0x6a] => Some("proposal_activate".into()),
        [0x30, 0xfa, 0x4e, 0xa8, 0xd0, 0xe2, 0xda, 0xd3] => Some("vault_transaction_create".into()),
        [0x9b, 0xec, 0x57, 0xe4, 0x89, 0x4b, 0x51, 0x27] => {
            Some("config_transaction_create".into())
        }
        [0xc2, 0x08, 0xa1, 0x57, 0x99, 0xa4, 0x19, 0xab] => {
            Some("vault_transaction_execute".into())
        }
        [0x72, 0x92, 0xf4, 0xbd, 0xfc, 0x8c, 0x24, 0x28] => {
            Some("config_transaction_execute".into())
        }
        _ => None,
    }
}

fn decode_known_program_instruction(ix: &Instruction) -> Option<String> {
    match ix.program_id.to_string().as_str() {
        "11111111111111111111111111111111" => decode_system_instruction(ix),
        "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA"
        | "TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb" => decode_token_instruction(ix),
        "ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL" => decode_ata_instruction(ix),
        "MemoSq4gqABAXKb96qnH8TysNcWxMyWCqXgDLGmfcHr"
        | "Memo1UhkJRfHyvLMcVucJwxXeuD728EqVDDwQDxFMNo" => decode_memo_instruction(ix),
        _ => None,
    }
}

fn decode_system_instruction(ix: &Instruction) -> Option<String> {
    if ix.data.len() < 4 {
        return None;
    }
    let tag = u32::from_le_bytes(ix.data[0..4].try_into().ok()?);
    match tag {
        2 if ix.data.len() >= 12 => {
            let lamports = u64::from_le_bytes(ix.data[4..12].try_into().ok()?);
            let recipient = ix
                .accounts
                .get(1)
                .map(|meta| meta.pubkey.to_string())
                .unwrap_or_else(|| "(missing recipient)".to_string());
            Some(format!(
                "System transfer {} SOL -> {}",
                crate::output::table::format_sol(lamports),
                recipient
            ))
        }
        0 => Some("System create account".to_string()),
        1 => Some("System assign".to_string()),
        3 => Some("System create account with seed".to_string()),
        4 => Some("System advance nonce account".to_string()),
        8 => Some("System allocate".to_string()),
        11 => Some("System transfer with seed".to_string()),
        _ => None,
    }
}

fn decode_token_instruction(ix: &Instruction) -> Option<String> {
    let tag = *ix.data.first()?;
    match tag {
        3 if ix.data.len() >= 9 => {
            let amount = u64::from_le_bytes(ix.data[1..9].try_into().ok()?);
            let destination = ix
                .accounts
                .get(1)
                .map(|meta| meta.pubkey.to_string())
                .unwrap_or_else(|| "(missing destination)".to_string());
            Some(format!(
                "Token transfer {amount} raw units -> {destination}"
            ))
        }
        12 if ix.data.len() >= 10 => {
            let amount = u64::from_le_bytes(ix.data[1..9].try_into().ok()?);
            let decimals = ix.data[9];
            Some(format!(
                "Token transfer checked {amount} raw units (decimals {decimals})"
            ))
        }
        4 => Some("Token approve".to_string()),
        7 => Some("Token mint to".to_string()),
        8 => Some("Token burn".to_string()),
        9 => Some("Token close account".to_string()),
        _ => None,
    }
}

fn decode_ata_instruction(ix: &Instruction) -> Option<String> {
    match ix.data.first().copied() {
        None => Some("Create associated token account".to_string()),
        Some(1) => Some("Create associated token account idempotent".to_string()),
        Some(2) => Some("Recover nested associated token account".to_string()),
        _ => None,
    }
}

fn decode_memo_instruction(ix: &Instruction) -> Option<String> {
    match std::str::from_utf8(&ix.data) {
        Ok(memo) if memo.len() <= 80 => Some(format!("Memo \"{memo}\"")),
        Ok(_) => Some(format!("Memo ({} bytes)", ix.data.len())),
        Err(_) => Some(format!("Memo ({} bytes)", ix.data.len())),
    }
}

fn identify_program(program_id: &Pubkey) -> String {
    match program_id.to_string().as_str() {
        "11111111111111111111111111111111" => "System".to_string(),
        "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA" => "Token".to_string(),
        "TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb" => "Token-2022".to_string(),
        "ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL" => "ATA".to_string(),
        "MemoSq4gqABAXKb96qnH8TysNcWxMyWCqXgDLGmfcHr" => "Memo".to_string(),
        "Memo1UhkJRfHyvLMcVucJwxXeuD728EqVDDwQDxFMNo" => "Memo".to_string(),
        "ComputeBudget111111111111111111111111111111" => "ComputeBudget".to_string(),
        other => short_address(other),
    }
}

fn short_address(address: &str) -> String {
    if address.len() <= 8 {
        address.to_string()
    } else {
        format!("{}...{}", &address[..4], &address[address.len() - 4..])
    }
}

/// Serialize inner instructions for vault transaction create.
pub fn serialize_vault_transaction_message(
    ixs: &[Instruction],
    vault: &Pubkey,
) -> Result<Vec<u8>, MsigError> {
    if ixs.len() > MAX_VAULT_MESSAGE_ITEMS {
        return Err(MsigError::Transaction(format!(
            "vault transaction has {} instructions; max supported is {MAX_VAULT_MESSAGE_ITEMS}",
            ixs.len()
        )));
    }

    let mut accounts = Vec::new();
    upsert_account(&mut accounts, *vault, true, true);

    for ix in ixs {
        if ix.accounts.len() > MAX_VAULT_MESSAGE_ITEMS {
            return Err(MsigError::Transaction(format!(
                "vault instruction references {} accounts; max supported is {MAX_VAULT_MESSAGE_ITEMS}",
                ix.accounts.len()
            )));
        }
        if ix.data.len() > MAX_U16_BYTES {
            return Err(MsigError::Transaction(format!(
                "vault instruction data is {} bytes; max supported is {MAX_U16_BYTES}",
                ix.data.len()
            )));
        }
        for meta in &ix.accounts {
            upsert_account(&mut accounts, meta.pubkey, meta.is_signer, meta.is_writable);
        }
        upsert_account(&mut accounts, ix.program_id, false, false);
    }

    let mut signer_writable: Vec<Pubkey> = Vec::new();
    let mut signer_readonly: Vec<Pubkey> = Vec::new();
    let mut nonsigner_writable: Vec<Pubkey> = Vec::new();
    let mut nonsigner_readonly: Vec<Pubkey> = Vec::new();

    for entry in &accounts {
        if entry.key == *vault {
            continue;
        }

        if entry.is_signer && entry.is_writable {
            signer_writable.push(entry.key);
        } else if entry.is_signer {
            signer_readonly.push(entry.key);
        } else if entry.is_writable {
            nonsigner_writable.push(entry.key);
        } else {
            nonsigner_readonly.push(entry.key);
        }
    }

    signer_writable.sort_unstable();
    signer_readonly.sort_unstable();
    nonsigner_writable.sort_unstable();
    nonsigner_readonly.sort_unstable();

    let mut keys: Vec<Pubkey> = Vec::new();
    keys.push(*vault);
    keys.extend_from_slice(&signer_writable);
    keys.extend_from_slice(&signer_readonly);
    keys.extend_from_slice(&nonsigner_writable);
    keys.extend_from_slice(&nonsigner_readonly);

    if keys.len() > MAX_VAULT_MESSAGE_ITEMS {
        return Err(MsigError::Transaction(format!(
            "vault transaction references {} account keys; max supported is {MAX_VAULT_MESSAGE_ITEMS}",
            keys.len()
        )));
    }

    let num_signers = checked_u8_len(
        1 + signer_writable.len() + signer_readonly.len(),
        "vault signer count",
    )?;
    let num_writable_signers =
        checked_u8_len(1 + signer_writable.len(), "vault writable signer count")?;
    let num_writable_non_signers =
        checked_u8_len(nonsigner_writable.len(), "vault writable non-signer count")?;

    let mut buf = vec![num_signers, num_writable_signers, num_writable_non_signers];

    // Account keys: u8 length prefix (not u32 — custom Squads format, not standard Borsh)
    buf.push(checked_u8_len(keys.len(), "vault account keys")?);
    for key in &keys {
        buf.extend_from_slice(key.as_ref());
    }

    let key_map: std::collections::HashMap<Pubkey, u8> = keys
        .iter()
        .enumerate()
        .map(|(i, k)| (*k, i as u8))
        .collect();

    // Instructions: u8 length prefix
    buf.push(checked_u8_len(ixs.len(), "vault instructions")?);
    for ix in ixs {
        let program_id_index = key_map
            .get(&ix.program_id)
            .copied()
            .ok_or_else(|| MsigError::Transaction("vault program id missing from keys".into()))?;
        buf.push(program_id_index);
        let account_indexes: Vec<u8> = ix
            .accounts
            .iter()
            .map(|meta| {
                key_map.get(&meta.pubkey).copied().ok_or_else(|| {
                    MsigError::Transaction("vault instruction account missing from keys".into())
                })
            })
            .collect::<Result<_, _>>()?;
        // account_indexes: u8 length prefix
        buf.push(checked_u8_len(
            account_indexes.len(),
            "vault instruction accounts",
        )?);
        buf.extend_from_slice(&account_indexes);
        // data: u16 LE length prefix
        buf.extend_from_slice(
            &checked_u16_len(ix.data.len(), "vault instruction data")?.to_le_bytes(),
        );
        buf.extend_from_slice(&ix.data);
    }

    // address_table_lookups: u8 length prefix (always 0 for now)
    buf.push(0u8);
    Ok(buf)
}

// Re-export for tests
#[cfg(test)]
mod tests {
    use super::*;
    use crate::infra::instruction::AccountMeta;

    fn compact_to_vec(value: u16) -> Vec<u8> {
        let (buf, len) = encode_compact_u16(value);
        buf[..len].to_vec()
    }

    #[test]
    fn compact_u16_zero() {
        assert_eq!(compact_to_vec(0), vec![0]);
    }

    #[test]
    fn compact_u16_small() {
        assert_eq!(compact_to_vec(127), vec![127]);
    }

    #[test]
    fn compact_u16_two_bytes() {
        assert_eq!(compact_to_vec(128), vec![0x80, 0x01]);
    }

    #[test]
    fn decode_instruction_names() {
        assert_eq!(
            decode_squads_instruction_name(
                &[0x90, 0x25, 0xa4, 0x88, 0xbc, 0xd8, 0x2a, 0xf8, 0x00,]
            ),
            Some("proposal_approve".into())
        );
        assert_eq!(decode_squads_instruction_name(&[0x00; 4]), None);
    }

    #[test]
    fn build_summary_prefers_review_instructions() {
        let payer = Pubkey::new_unique();
        let recipient = Pubkey::new_unique();
        let system_program = solana_pubkey::pubkey!("11111111111111111111111111111111");
        let mut transfer_data = Vec::new();
        transfer_data.extend_from_slice(&2u32.to_le_bytes());
        transfer_data.extend_from_slice(&1_000_000_000u64.to_le_bytes());
        let inner = Instruction {
            program_id: system_program,
            accounts: vec![
                AccountMeta::new(payer, true),
                AccountMeta::new(recipient, false),
            ],
            data: transfer_data,
        };
        let outer = Instruction {
            program_id: Pubkey::new_unique(),
            accounts: vec![],
            data: vec![0x30, 0xfa, 0x4e, 0xa8, 0xd0, 0xe2, 0xda, 0xd3],
        };
        let summary = build_summary("review", &[outer], &[inner], 1_000);

        assert_eq!(summary.instructions.len(), 1);
        assert_eq!(summary.review_instructions.len(), 1);
        let expected = format!("System transfer 1 SOL -> {recipient}");
        assert_eq!(
            summary.review_instructions[0].decoded.as_deref(),
            Some(expected.as_str())
        );
    }

    #[test]
    fn build_message_basic() {
        let payer = Pubkey::new_unique();
        let program_id = Pubkey::new_unique();
        let blockhash = [0u8; 32];
        let ix = Instruction {
            program_id,
            accounts: vec![],
            data: vec![1, 2, 3],
        };
        let msg = build_message(&[ix], &payer, &blockhash).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(msg[0], 1); // 1 required signature
        assert_eq!(msg[3], 2); // 2 accounts
    }

    #[test]
    fn build_message_matches_simple_reference_vector() {
        let payer = Pubkey::from([1u8; 32]);
        let writable_account = Pubkey::from([2u8; 32]);
        let readonly_account = Pubkey::from([3u8; 32]);
        let program_id = Pubkey::from([4u8; 32]);
        let blockhash = [9u8; 32];
        let ix = Instruction {
            program_id,
            accounts: vec![
                crate::infra::instruction::AccountMeta::new(writable_account, false),
                crate::infra::instruction::AccountMeta::new_readonly(readonly_account, false),
            ],
            data: vec![0xaa, 0xbb],
        };

        let msg = build_message(&[ix], &payer, &blockhash).unwrap_or_else(|e| panic!("{e}"));
        let mut expected = vec![1, 0, 2, 4];
        expected.extend_from_slice(&[1u8; 32]);
        expected.extend_from_slice(&[2u8; 32]);
        expected.extend_from_slice(&[3u8; 32]);
        expected.extend_from_slice(&[4u8; 32]);
        expected.extend_from_slice(&[9u8; 32]);
        expected.extend_from_slice(&[1, 3, 2, 1, 2, 2, 0xaa, 0xbb]);

        assert_eq!(msg, expected);
    }

    #[test]
    fn build_message_is_stable_with_duplicate_account_flag_upgrades() {
        let payer = Pubkey::from([1u8; 32]);
        let account = Pubkey::from([2u8; 32]);
        let program_id = Pubkey::from([3u8; 32]);
        let blockhash = [4u8; 32];
        let ix = Instruction {
            program_id,
            accounts: vec![
                crate::infra::instruction::AccountMeta::new_readonly(account, false),
                crate::infra::instruction::AccountMeta::new(account, false),
            ],
            data: vec![],
        };

        let first = build_message(std::slice::from_ref(&ix), &payer, &blockhash)
            .unwrap_or_else(|e| panic!("{e}"));
        let second = build_message(&[ix], &payer, &blockhash).unwrap_or_else(|e| panic!("{e}"));

        assert_eq!(first, second);
        assert_eq!(first[0], 1);
        assert_eq!(first[2], 1);
    }

    #[test]
    fn required_signers_follow_message_header_order() {
        let payer = Pubkey::from([1u8; 32]);
        let extra_signer = Pubkey::from([2u8; 32]);
        let program_id = Pubkey::from([3u8; 32]);
        let blockhash = [4u8; 32];
        let ix = Instruction {
            program_id,
            accounts: vec![crate::infra::instruction::AccountMeta::new_readonly(
                extra_signer,
                true,
            )],
            data: vec![],
        };

        let msg = build_message(&[ix], &payer, &blockhash).unwrap_or_else(|e| panic!("{e}"));
        let signers = required_signers_from_message(&msg).unwrap_or_else(|e| panic!("{e}"));

        assert_eq!(signers, vec![payer, extra_signer]);
    }

    #[test]
    fn compute_unit_padding_uses_integer_ceiling() {
        assert_eq!(padded_compute_unit_limit(0), 10_000);
        assert_eq!(padded_compute_unit_limit(10_001), 11_002);
        assert_eq!(padded_compute_unit_limit(u64::MAX), u32::MAX);
    }

    #[test]
    fn vault_message_rejects_too_many_keys_before_truncation() {
        let vault = Pubkey::from([1u8; 32]);
        let program_id = Pubkey::from([2u8; 32]);
        let accounts = (0..=MAX_VAULT_MESSAGE_ITEMS)
            .map(|i| {
                let mut bytes = [0u8; 32];
                bytes[0] = 3;
                bytes[31] = i as u8;
                crate::infra::instruction::AccountMeta::new(Pubkey::from(bytes), false)
            })
            .collect();
        let ix = Instruction {
            program_id,
            accounts,
            data: vec![],
        };

        let result = serialize_vault_transaction_message(&[ix], &vault);
        assert!(matches!(result, Err(MsigError::Transaction(_))));
    }
}
