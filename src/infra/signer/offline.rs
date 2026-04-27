//! Offline signing: export and import `.sqds` transaction files.
//!
//! The `.sqds` format is a JSON envelope that contains:
//! - Multisig metadata (address, threshold, members, config state hash)
//! - Serialized Solana message bytes (base64)
//! - Optional decoded instruction advisory info for exporter-side UX
//! - An Ed25519 envelope signature over canonical data
//!
//! This allows air-gapped signing workflows where a transaction is
//! exported on a hot machine, signed on a cold machine with a Ledger,
//! and imported back.

use ed25519_dalek::Verifier;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::Signer;
use crate::error::MsigError;

/// Domain separator for the canonical signed data hash.
const DOMAIN_SEPARATOR: &[u8] = b"squads-offline-v1";

// ---------------------------------------------------------------------------
// .sqds file format types
// ---------------------------------------------------------------------------

/// Top-level .sqds file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OfflineFile {
    pub version: u8,
    pub format: String,
    pub created_at: String,
    #[serde(default)]
    pub cluster: String,
    pub exporter: ExporterInfo,
    pub multisig: MultisigInfo,
    pub transaction: TransactionInfo,
    pub envelope_signature: EnvelopeSignature,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExporterInfo {
    pub pubkey: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MultisigInfo {
    pub address: String,
    pub threshold: u16,
    pub members: Vec<MemberInfo>,
    pub config_state_hash: String,
    /// The create_key used to derive the multisig PDA. Stored so that
    /// `verify_envelope` can recompute the canonical signed_hash from
    /// file fields alone, preventing metadata tampering.
    #[serde(default)]
    pub create_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemberInfo {
    pub pubkey: String,
    pub permissions: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransactionInfo {
    pub index: u64,
    pub vault_index: u8,
    pub message_b64: String,
    pub message_hash: String,
    #[serde(default)]
    pub decoded_instructions: Vec<DecodedInstruction>,
    #[serde(default)]
    pub existing_signatures: Vec<ExistingSignature>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecodedInstruction {
    pub program_id: String,
    pub accounts_count: usize,
    pub data_len: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decoded_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExistingSignature {
    pub signer: String,
    pub signature: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvelopeSignature {
    pub signer: String,
    pub signed_hash: String,
    pub signature: String,
}

// ---------------------------------------------------------------------------
// Canonical data hash
// ---------------------------------------------------------------------------

pub struct SignedDataPayload<'a> {
    pub version: u8,
    pub cluster: &'a str,
    pub multisig_address: &'a [u8; 32],
    pub transaction_index: u64,
    pub vault_index: u8,
    pub message_bytes: &'a [u8],
    pub config_state_hash: &'a [u8; 32],
}

fn hash_len_prefixed(hasher: &mut Sha256, bytes: &[u8]) {
    hasher.update((bytes.len() as u64).to_le_bytes());
    hasher.update(bytes);
}

/// Compute the canonical hash over the critical transaction data.
///
/// This is the value that the envelope is signed over, providing
/// integrity verification of the .sqds file.
///
/// Layout: SHA-256 of the domain separator plus explicit, length-prefixed fields.
pub fn compute_signed_data(payload: &SignedDataPayload<'_>) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(DOMAIN_SEPARATOR);
    hasher.update([payload.version]);
    hash_len_prefixed(&mut hasher, payload.cluster.as_bytes());
    hasher.update(payload.multisig_address);
    hasher.update(payload.transaction_index.to_le_bytes());
    hasher.update([payload.vault_index]);
    hasher.update(payload.config_state_hash);
    hash_len_prefixed(&mut hasher, payload.message_bytes);
    let result = hasher.finalize();
    let mut out = [0u8; 32];
    out.copy_from_slice(&result);
    out
}

/// Compute a config state hash from multisig parameters.
///
/// This binds the offline file to a specific multisig configuration,
/// preventing replays after membership changes.
///
/// Layout: SHA-256 of (create_key + threshold_le + (member_key + permissions) * N)
pub fn compute_config_state_hash(
    create_key: &[u8; 32],
    threshold: u16,
    members: &[(&[u8; 32], u8)],
) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(create_key);
    hasher.update(threshold.to_le_bytes());
    for (key, permissions) in members {
        hasher.update(*key);
        hasher.update([*permissions]);
    }
    let result = hasher.finalize();
    let mut out = [0u8; 32];
    out.copy_from_slice(&result);
    out
}

// ---------------------------------------------------------------------------
// Export
// ---------------------------------------------------------------------------

/// Parameters for exporting a transaction.
pub struct ExportParams {
    pub version: u8,
    pub exporter_pubkey: solana_pubkey::Pubkey,
    pub exporter_label: Option<String>,
    pub multisig_address: solana_pubkey::Pubkey,
    pub threshold: u16,
    pub members: Vec<(solana_pubkey::Pubkey, u8)>,
    pub create_key: [u8; 32],
    pub transaction_index: u64,
    pub vault_index: u8,
    pub message_bytes: Vec<u8>,
    pub decoded_instructions: Vec<DecodedInstruction>,
    pub existing_signatures: Vec<ExistingSignature>,
    pub cluster: String,
}

/// Build an `OfflineFile` and sign its envelope with the given secret key.
pub fn export_transaction(
    params: &ExportParams,
    envelope_signer: &dyn Signer,
) -> Result<OfflineFile, MsigError> {
    if envelope_signer.pubkey() != params.exporter_pubkey {
        return Err(MsigError::Signing(format!(
            "envelope signer {} does not match exporter {}",
            envelope_signer.pubkey(),
            params.exporter_pubkey
        )));
    }

    // Config state hash
    let member_keys: Vec<([u8; 32], u8)> = params
        .members
        .iter()
        .map(|(pk, perm)| {
            let bytes: [u8; 32] = pk.to_bytes();
            (bytes, *perm)
        })
        .collect();
    let member_refs: Vec<(&[u8; 32], u8)> = member_keys.iter().map(|(k, p)| (k, *p)).collect();
    let config_hash = compute_config_state_hash(&params.create_key, params.threshold, &member_refs);
    let config_hash_b58 = bs58::encode(&config_hash).into_string();

    // Message hash
    let msg_hash = {
        let mut h = Sha256::new();
        h.update(&params.message_bytes);
        let r = h.finalize();
        let mut out = [0u8; 32];
        out.copy_from_slice(&r);
        out
    };
    let msg_hash_b58 = bs58::encode(&msg_hash).into_string();

    // Message base64
    let message_b64 = base64::Engine::encode(
        &base64::engine::general_purpose::STANDARD,
        &params.message_bytes,
    );

    // Canonical signed data
    let multisig_bytes: [u8; 32] = params.multisig_address.to_bytes();
    let signed_data = compute_signed_data(&SignedDataPayload {
        version: params.version,
        cluster: &params.cluster,
        multisig_address: &multisig_bytes,
        transaction_index: params.transaction_index,
        vault_index: params.vault_index,
        message_bytes: &params.message_bytes,
        config_state_hash: &config_hash,
    });
    let signed_hash_b58 = bs58::encode(&signed_data).into_string();

    // Sign the hash through the configured signer, so keypair and Ledger
    // exporters use the same path and raw key material is not reloaded here.
    let signature = envelope_signer.sign(&signed_data)?;
    if signature.len() != 64 {
        return Err(MsigError::Signing(format!(
            "envelope signature is {} bytes, expected 64",
            signature.len()
        )));
    }
    let sig_b58 = bs58::encode(signature).into_string();

    // ISO 8601 timestamp (basic — no chrono dependency)
    let created_at = format_timestamp_now();

    let file = OfflineFile {
        version: params.version,
        format: "squads-offline-v1".to_string(),
        created_at,
        cluster: params.cluster.clone(),
        exporter: ExporterInfo {
            pubkey: params.exporter_pubkey.to_string(),
            label: params.exporter_label.clone(),
        },
        multisig: MultisigInfo {
            address: params.multisig_address.to_string(),
            threshold: params.threshold,
            members: params
                .members
                .iter()
                .map(|(pk, perm)| MemberInfo {
                    pubkey: pk.to_string(),
                    permissions: *perm,
                })
                .collect(),
            config_state_hash: config_hash_b58,
            create_key: bs58::encode(&params.create_key).into_string(),
        },
        transaction: TransactionInfo {
            index: params.transaction_index,
            vault_index: params.vault_index,
            message_b64,
            message_hash: msg_hash_b58,
            decoded_instructions: params.decoded_instructions.clone(),
            existing_signatures: params.existing_signatures.clone(),
        },
        envelope_signature: EnvelopeSignature {
            signer: params.exporter_pubkey.to_string(),
            signed_hash: signed_hash_b58,
            signature: sig_b58,
        },
    };

    Ok(file)
}

/// Serialize the `OfflineFile` and write it to disk.
pub fn write_offline_file(file: &OfflineFile, path: &std::path::Path) -> Result<(), MsigError> {
    let json = serde_json::to_string_pretty(file)
        .map_err(|e| MsigError::Signing(format!("failed to serialize .sqds file: {e}")))?;

    std::fs::write(path, json).map_err(MsigError::Io)?;

    // Set restrictive permissions — .sqds files contain signed transaction
    // data and sensitive multisig metadata.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        std::fs::set_permissions(path, perms).map_err(|e| {
            MsigError::Signing(format!(
                "failed to set permissions on {}: {e}",
                path.display()
            ))
        })?;
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Import
// ---------------------------------------------------------------------------

/// Read and parse a .sqds file from disk.
pub fn read_offline_file(path: &std::path::Path) -> Result<OfflineFile, MsigError> {
    if !path.exists() {
        return Err(MsigError::Signing(format!(
            "offline file not found: {}",
            path.display()
        )));
    }

    let contents = std::fs::read_to_string(path).map_err(|e| {
        MsigError::Signing(format!(
            "failed to read offline file {}: {e}",
            path.display()
        ))
    })?;

    let file: OfflineFile = serde_json::from_str(&contents).map_err(|e| {
        MsigError::Signing(format!(
            "failed to parse .sqds file {}: {e}",
            path.display()
        ))
    })?;

    if file.format != "squads-offline-v1" {
        return Err(MsigError::Signing(format!(
            "unsupported .sqds format: '{}'. Expected 'squads-offline-v1'.",
            file.format
        )));
    }

    if file.version != 1 {
        return Err(MsigError::Signing(format!(
            "unsupported .sqds version: {}. Expected 1.",
            file.version
        )));
    }

    Ok(file)
}

/// Verify the envelope signature on an `OfflineFile`.
///
/// Recomputes the canonical hash from the file contents and verifies
/// the Ed25519 signature against the exporter's public key.
pub fn verify_envelope(file: &OfflineFile) -> Result<(), MsigError> {
    // Decode the exporter public key
    let signer_bytes = bs58::decode(&file.envelope_signature.signer)
        .into_vec()
        .map_err(|e| MsigError::Signing(format!("invalid envelope signer pubkey: {e}")))?;

    if signer_bytes.len() != 32 {
        return Err(MsigError::Signing(
            "envelope signer pubkey is not 32 bytes".to_string(),
        ));
    }

    let mut signer_arr = [0u8; 32];
    signer_arr.copy_from_slice(&signer_bytes);
    let verifying_key = ed25519_dalek::VerifyingKey::from_bytes(&signer_arr)
        .map_err(|e| MsigError::Signing(format!("invalid envelope signer key: {e}")))?;

    // Decode the message bytes from base64
    let message_bytes = base64::Engine::decode(
        &base64::engine::general_purpose::STANDARD,
        &file.transaction.message_b64,
    )
    .map_err(|e| MsigError::Signing(format!("invalid message_b64 in .sqds file: {e}")))?;

    // Recompute the canonical signed_hash from file fields to prevent
    // metadata tampering (e.g. modified multisig.address or transaction.index).
    if file.multisig.create_key.is_empty() {
        return Err(MsigError::Signing(
            "missing create_key in .sqds file — cannot verify envelope integrity".into(),
        ));
    }

    let create_key_bytes: Vec<u8> = bs58::decode(&file.multisig.create_key)
        .into_vec()
        .map_err(|e| MsigError::Signing(format!("invalid create_key: {e}")))?;
    if create_key_bytes.len() != 32 {
        return Err(MsigError::Signing("create_key is not 32 bytes".into()));
    }
    let mut ck = [0u8; 32];
    ck.copy_from_slice(&create_key_bytes);

    let member_keys: Vec<([u8; 32], u8)> = file
        .multisig
        .members
        .iter()
        .map(|m| {
            let bytes = bs58::decode(&m.pubkey)
                .into_vec()
                .map_err(|e| MsigError::Signing(format!("invalid member pubkey: {e}")))?;
            if bytes.len() != 32 {
                return Err(MsigError::Signing("member pubkey is not 32 bytes".into()));
            }
            let mut arr = [0u8; 32];
            arr.copy_from_slice(&bytes);
            Ok((arr, m.permissions))
        })
        .collect::<Result<_, MsigError>>()?;
    let member_refs: Vec<(&[u8; 32], u8)> = member_keys.iter().map(|(k, p)| (k, *p)).collect();
    let config_hash = compute_config_state_hash(&ck, file.multisig.threshold, &member_refs);

    // Verify config_state_hash matches what's in the file
    let expected_config_hash_b58 = bs58::encode(&config_hash).into_string();
    if expected_config_hash_b58 != file.multisig.config_state_hash {
        return Err(MsigError::Signing(
            "config_state_hash mismatch: multisig metadata may have been tampered with."
                .to_string(),
        ));
    }

    let multisig_addr_bytes: Vec<u8> = bs58::decode(&file.multisig.address)
        .into_vec()
        .map_err(|e| MsigError::Signing(format!("invalid multisig address: {e}")))?;
    if multisig_addr_bytes.len() != 32 {
        return Err(MsigError::Signing(
            "multisig address is not 32 bytes".into(),
        ));
    }
    let mut ms_arr = [0u8; 32];
    ms_arr.copy_from_slice(&multisig_addr_bytes);

    let parsed_message = message_summary(&message_bytes)?;
    verify_advisory_instructions(&file.transaction.decoded_instructions, &parsed_message)?;

    let signed_hash_bytes = compute_signed_data(&SignedDataPayload {
        version: file.version,
        cluster: &file.cluster,
        multisig_address: &ms_arr,
        transaction_index: file.transaction.index,
        vault_index: file.transaction.vault_index,
        message_bytes: &message_bytes,
        config_state_hash: &config_hash,
    });
    let signed_hash_b58 = bs58::encode(&signed_hash_bytes).into_string();
    if signed_hash_b58 != file.envelope_signature.signed_hash {
        return Err(MsigError::Signing(
            "signed_hash mismatch: envelope metadata may have been tampered with.".to_string(),
        ));
    }

    let sig_bytes = bs58::decode(&file.envelope_signature.signature)
        .into_vec()
        .map_err(|e| MsigError::Signing(format!("invalid signature in envelope: {e}")))?;

    if sig_bytes.len() != 64 {
        return Err(MsigError::Signing(format!(
            "envelope signature is {} bytes, expected 64",
            sig_bytes.len()
        )));
    }

    let mut sig_arr = [0u8; 64];
    sig_arr.copy_from_slice(&sig_bytes);
    let signature = ed25519_dalek::Signature::from_bytes(&sig_arr);

    // Verify the Ed25519 signature over the signed_hash
    verifying_key
        .verify(&signed_hash_bytes, &signature)
        .map_err(|_| {
            MsigError::Signing(
                "envelope signature verification FAILED. \
                 The .sqds file may have been tampered with."
                    .to_string(),
            )
        })?;

    // Additionally verify the message hash matches
    let expected_msg_hash = {
        let mut h = Sha256::new();
        h.update(&message_bytes);
        let r = h.finalize();
        bs58::encode(r).into_string()
    };

    if expected_msg_hash != file.transaction.message_hash {
        return Err(MsigError::Signing(
            "message_hash mismatch: the stored message bytes don't match the hash. \
             The .sqds file may have been tampered with."
                .to_string(),
        ));
    }

    // Verify exporter pubkey matches envelope signer
    if file.exporter.pubkey != file.envelope_signature.signer {
        return Err(MsigError::Signing(
            "exporter pubkey does not match envelope signer".to_string(),
        ));
    }

    Ok(())
}

/// Decode the transaction message stored in a `.sqds` file.
pub fn decode_message(file: &OfflineFile) -> Result<Vec<u8>, MsigError> {
    base64::Engine::decode(
        &base64::engine::general_purpose::STANDARD,
        &file.transaction.message_b64,
    )
    .map_err(|e| MsigError::Signing(format!("invalid message_b64 in .sqds file: {e}")))
}

/// Return the fee-payer/signing account keys required by a legacy Solana message.
pub fn required_signers(message: &[u8]) -> Result<Vec<solana_pubkey::Pubkey>, MsigError> {
    let parsed = parse_legacy_message(message)?;
    Ok(parsed.required_signers)
}

/// Add or replace a signature in the file after verifying it signs the stored message.
pub fn add_verified_signature(
    file: &mut OfflineFile,
    signer: solana_pubkey::Pubkey,
    signature: &[u8],
) -> Result<(), MsigError> {
    let message = decode_message(file)?;
    let required = required_signers(&message)?;
    if !required.contains(&signer) {
        return Err(MsigError::Signing(format!(
            "signer {signer} is not required by this transaction message"
        )));
    }
    verify_message_signature(&signer, &message, signature)?;

    let signature_b58 = bs58::encode(signature).into_string();
    let signer_str = signer.to_string();
    if let Some(existing) = file
        .transaction
        .existing_signatures
        .iter_mut()
        .find(|sig| sig.signer == signer_str)
    {
        existing.signature = signature_b58;
    } else {
        file.transaction
            .existing_signatures
            .push(ExistingSignature {
                signer: signer_str,
                signature: signature_b58,
            });
    }
    Ok(())
}

/// Build a signed legacy transaction in Solana wire format from stored signatures.
pub fn build_signed_transaction(file: &OfflineFile) -> Result<Vec<u8>, MsigError> {
    let message = decode_message(file)?;
    let required = required_signers(&message)?;

    let mut signatures = Vec::with_capacity(required.len());
    let need = u16::try_from(required.len()).unwrap_or(u16::MAX);
    for signer in required {
        let signer_str = signer.to_string();
        let sig = file
            .transaction
            .existing_signatures
            .iter()
            .find(|sig| sig.signer == signer_str)
            .ok_or(MsigError::InsufficientSignatures {
                have: file.transaction.existing_signatures.len() as u16,
                need,
            })?;
        let sig_bytes = bs58::decode(&sig.signature)
            .into_vec()
            .map_err(|e| MsigError::Signing(format!("invalid signature for {signer}: {e}")))?;
        verify_message_signature(&signer, &message, &sig_bytes)?;
        signatures.push(sig_bytes);
    }

    let mut wire = Vec::new();
    encode_short_vec_len(signatures.len(), &mut wire)?;
    for sig in signatures {
        wire.extend_from_slice(&sig);
    }
    wire.extend_from_slice(&message);
    Ok(wire)
}

pub fn message_summary(message: &[u8]) -> Result<MessageSummary, MsigError> {
    parse_legacy_message(message)
}

#[derive(Debug, Clone)]
pub struct MessageSummary {
    pub required_signers: Vec<solana_pubkey::Pubkey>,
    pub recent_blockhash: String,
    pub instruction_count: usize,
    pub decoded_instructions: Vec<DecodedInstruction>,
}

fn verify_advisory_instructions(
    advisory: &[DecodedInstruction],
    parsed: &MessageSummary,
) -> Result<(), MsigError> {
    if advisory.is_empty() {
        return Ok(());
    }

    if advisory.len() != parsed.decoded_instructions.len() {
        return Err(MsigError::Signing(format!(
            "decoded instruction advisory mismatch: file has {} instructions, message has {}",
            advisory.len(),
            parsed.decoded_instructions.len()
        )));
    }

    for (idx, (claimed, actual)) in advisory
        .iter()
        .zip(parsed.decoded_instructions.iter())
        .enumerate()
    {
        if claimed.program_id != actual.program_id
            || claimed.accounts_count != actual.accounts_count
            || claimed.data_len != actual.data_len
        {
            return Err(MsigError::Signing(format!(
                "decoded instruction advisory mismatch at #{}: message has program {}, {} accounts, {} bytes data",
                idx + 1,
                actual.program_id,
                actual.accounts_count,
                actual.data_len
            )));
        }
    }

    Ok(())
}

fn verify_message_signature(
    signer: &solana_pubkey::Pubkey,
    message: &[u8],
    signature: &[u8],
) -> Result<(), MsigError> {
    if signature.len() != 64 {
        return Err(MsigError::Signing(format!(
            "signature for {signer} is {} bytes, expected 64",
            signature.len()
        )));
    }
    let signer_bytes = signer.to_bytes();
    let verifying_key = ed25519_dalek::VerifyingKey::from_bytes(&signer_bytes)
        .map_err(|e| MsigError::Signing(format!("invalid signer key {signer}: {e}")))?;
    let mut sig_arr = [0u8; 64];
    sig_arr.copy_from_slice(signature);
    let signature = ed25519_dalek::Signature::from_bytes(&sig_arr);
    verifying_key.verify(message, &signature).map_err(|_| {
        MsigError::Signing(format!(
            "signature for {signer} does not verify against this transaction message"
        ))
    })
}

fn parse_legacy_message(message: &[u8]) -> Result<MessageSummary, MsigError> {
    if message.len() < 3 {
        return Err(MsigError::Signing(
            "transaction message is too short to contain a Solana header".into(),
        ));
    }
    if message[0] & 0x80 != 0 {
        return Err(MsigError::Signing(
            "versioned transaction messages are not supported in .sqds files yet".into(),
        ));
    }

    let required_signatures = message[0] as usize;
    let mut offset = 3usize;
    let account_count = read_short_vec_len(message, &mut offset)?;
    if account_count < required_signatures {
        return Err(MsigError::Signing(format!(
            "message requires {required_signatures} signatures but only has {account_count} account keys"
        )));
    }
    if account_count > 256 {
        return Err(MsigError::Signing(format!(
            "message has {account_count} account keys; legacy instruction indexes support at most 256"
        )));
    }

    let keys_len = account_count
        .checked_mul(32)
        .ok_or_else(|| MsigError::Signing("message account key length overflow".into()))?;
    if message.len().saturating_sub(offset) < keys_len + 32 {
        return Err(MsigError::Signing(
            "transaction message is truncated before account keys or blockhash".into(),
        ));
    }

    let mut account_keys = Vec::with_capacity(account_count);
    let mut required_signers = Vec::with_capacity(required_signatures);
    for i in 0..account_count {
        let start = offset + i * 32;
        let mut bytes = [0u8; 32];
        bytes.copy_from_slice(&message[start..start + 32]);
        let pubkey = solana_pubkey::Pubkey::from(bytes);
        if i < required_signatures {
            required_signers.push(pubkey);
        }
        account_keys.push(pubkey);
    }
    offset += keys_len;

    let blockhash = &message[offset..offset + 32];
    offset += 32;

    let instruction_count = read_short_vec_len(message, &mut offset)?;
    let mut decoded_instructions = Vec::with_capacity(instruction_count);
    for ix_idx in 0..instruction_count {
        if offset >= message.len() {
            return Err(MsigError::Signing(
                "transaction message is truncated in instruction program index".into(),
            ));
        }
        let program_id_index = message[offset] as usize;
        let program_id = account_keys.get(program_id_index).ok_or_else(|| {
            MsigError::Signing(format!(
                "instruction #{} references program index {program_id_index} but message has {account_count} account keys",
                ix_idx + 1
            ))
        })?;
        offset += 1;
        let account_indexes = read_short_vec_len(message, &mut offset)?;
        if message.len().saturating_sub(offset) < account_indexes {
            return Err(MsigError::Signing(
                "transaction message is truncated in instruction accounts".into(),
            ));
        }
        for account_offset in 0..account_indexes {
            let account_index = message[offset + account_offset] as usize;
            if account_index >= account_count {
                return Err(MsigError::Signing(format!(
                    "instruction #{} references account index {account_index} but message has {account_count} account keys",
                    ix_idx + 1
                )));
            }
        }
        offset += account_indexes;
        let data_len = read_short_vec_len(message, &mut offset)?;
        if message.len().saturating_sub(offset) < data_len {
            return Err(MsigError::Signing(
                "transaction message is truncated in instruction data".into(),
            ));
        }
        offset += data_len;
        decoded_instructions.push(DecodedInstruction {
            program_id: program_id.to_string(),
            accounts_count: account_indexes,
            data_len,
            decoded_name: None,
        });
    }

    if offset != message.len() {
        return Err(MsigError::Signing(format!(
            "transaction message has {} trailing bytes",
            message.len() - offset
        )));
    }

    Ok(MessageSummary {
        required_signers,
        recent_blockhash: bs58::encode(blockhash).into_string(),
        instruction_count,
        decoded_instructions,
    })
}

fn read_short_vec_len(message: &[u8], offset: &mut usize) -> Result<usize, MsigError> {
    let mut len = 0usize;
    let mut shift = 0usize;
    for _ in 0..3 {
        let byte = *message.get(*offset).ok_or_else(|| {
            MsigError::Signing("transaction message shortvec is truncated".into())
        })?;
        *offset += 1;
        len |= ((byte & 0x7f) as usize) << shift;
        if byte & 0x80 == 0 {
            if len > u16::MAX as usize {
                return Err(MsigError::Signing(format!(
                    "transaction message shortvec length {len} exceeds u16::MAX"
                )));
            }
            return Ok(len);
        }
        shift += 7;
    }
    Err(MsigError::Signing(
        "transaction message shortvec exceeds u16 length".into(),
    ))
}

fn encode_short_vec_len(len: usize, out: &mut Vec<u8>) -> Result<(), MsigError> {
    let len = u16::try_from(len)
        .map_err(|_| MsigError::Signing(format!("shortvec length {len} exceeds u16::MAX")))?;
    let mut value = len;
    loop {
        let mut byte = (value & 0x7f) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        out.push(byte);
        if value == 0 {
            break;
        }
    }
    Ok(())
}

/// Validate that the .sqds file targets the expected cluster.
///
/// This is a best-effort check using the RPC URL prefix.
pub fn validate_cluster_match(file_cluster: &str, current_cluster: &str) -> Result<(), MsigError> {
    // Normalize for comparison
    let normalize = |s: &str| -> String {
        if s.contains("mainnet") {
            "mainnet".to_string()
        } else if s.contains("devnet") {
            "devnet".to_string()
        } else if s.contains("testnet") {
            "testnet".to_string()
        } else if s.contains("localhost") || s.contains("127.0.0.1") {
            "localhost".to_string()
        } else {
            s.to_string()
        }
    };

    let file_norm = normalize(file_cluster);
    let current_norm = normalize(current_cluster);

    if file_norm != current_norm {
        return Err(MsigError::Signing(format!(
            "cluster mismatch: .sqds file was exported for '{file_cluster}' \
             but current cluster is '{current_cluster}'"
        )));
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Simple UTC timestamp without external crate.
fn format_timestamp_now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_secs()).ok())
        .unwrap_or(0);

    // Convert epoch seconds to UTC components without pulling in chrono/time.
    let days = secs / 86400;
    let time_of_day = secs % 86400;
    let hours = time_of_day / 3600;
    let minutes = (time_of_day % 3600) / 60;
    let seconds = time_of_day % 60;
    let (year, month, day) = days_to_ymd(days);

    format!("{year:04}-{month:02}-{day:02}T{hours:02}:{minutes:02}:{seconds:02}Z")
}

/// Convert days since Unix epoch to (year, month, day).
fn days_to_ymd(days: i64) -> (i64, u32, u32) {
    // Civil calendar algorithm from Howard Hinnant
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u32;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };
    (year, m, d)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    struct TestSigner {
        signing_key: ed25519_dalek::SigningKey,
        pubkey: solana_pubkey::Pubkey,
    }

    impl TestSigner {
        fn new(secret_bytes: [u8; 32]) -> Self {
            let signing_key = ed25519_dalek::SigningKey::from_bytes(&secret_bytes);
            let pubkey = solana_pubkey::Pubkey::from(signing_key.verifying_key().to_bytes());
            Self {
                signing_key,
                pubkey,
            }
        }
    }

    impl crate::infra::signer::Signer for TestSigner {
        fn pubkey(&self) -> solana_pubkey::Pubkey {
            self.pubkey
        }

        fn sign(&self, message: &[u8]) -> Result<Vec<u8>, MsigError> {
            let signature: ed25519_dalek::Signature =
                ed25519_dalek::Signer::sign(&self.signing_key, message);
            Ok(signature.to_bytes().to_vec())
        }
    }

    fn legacy_message(
        payer: solana_pubkey::Pubkey,
        program_id: solana_pubkey::Pubkey,
        accounts: usize,
        data: &[u8],
    ) -> Vec<u8> {
        let metas = (0..accounts)
            .map(|i| crate::infra::instruction::AccountMeta {
                pubkey: solana_pubkey::Pubkey::from([20u8 + i as u8; 32]),
                is_signer: false,
                is_writable: false,
            })
            .collect();
        let ix = crate::infra::instruction::Instruction {
            program_id,
            accounts: metas,
            data: data.to_vec(),
        };
        let blockhash = bs58::encode([9u8; 32]).into_string();
        crate::application::pipeline::compile_legacy_message(&[ix], &payer, &blockhash)
            .unwrap_or_else(|e| panic!("{e}"))
    }

    fn decoded_from_message(message: &[u8]) -> Vec<DecodedInstruction> {
        message_summary(message)
            .unwrap_or_else(|e| panic!("{e}"))
            .decoded_instructions
    }

    #[test]
    fn test_compute_config_state_hash_deterministic() {
        let create_key = [1u8; 32];
        let threshold = 2u16;
        let member1 = ([10u8; 32], 7u8);
        let member2 = ([20u8; 32], 3u8);

        let h1 = compute_config_state_hash(
            &create_key,
            threshold,
            &[(&member1.0, member1.1), (&member2.0, member2.1)],
        );
        let h2 = compute_config_state_hash(
            &create_key,
            threshold,
            &[(&member1.0, member1.1), (&member2.0, member2.1)],
        );

        assert_eq!(h1, h2);
    }

    #[test]
    fn test_compute_config_state_hash_varies_with_threshold() {
        let create_key = [1u8; 32];
        let member = ([10u8; 32], 7u8);

        let h1 = compute_config_state_hash(&create_key, 2, &[(&member.0, member.1)]);
        let h2 = compute_config_state_hash(&create_key, 3, &[(&member.0, member.1)]);

        assert_ne!(h1, h2);
    }

    #[test]
    fn test_compute_signed_data_deterministic() {
        let multisig = [1u8; 32];
        let config_hash = [2u8; 32];
        let message = b"hello";

        let payload = SignedDataPayload {
            version: 1,
            cluster: "devnet",
            multisig_address: &multisig,
            transaction_index: 42,
            vault_index: 0,
            message_bytes: message,
            config_state_hash: &config_hash,
        };
        let h1 = compute_signed_data(&payload);
        let h2 = compute_signed_data(&payload);

        assert_eq!(h1, h2);
    }

    #[test]
    fn test_compute_signed_data_varies_with_index() {
        let multisig = [1u8; 32];
        let config_hash = [2u8; 32];
        let message = b"hello";

        let h1 = compute_signed_data(&SignedDataPayload {
            version: 1,
            cluster: "devnet",
            multisig_address: &multisig,
            transaction_index: 42,
            vault_index: 0,
            message_bytes: message,
            config_state_hash: &config_hash,
        });
        let h2 = compute_signed_data(&SignedDataPayload {
            version: 1,
            cluster: "devnet",
            multisig_address: &multisig,
            transaction_index: 43,
            vault_index: 0,
            message_bytes: message,
            config_state_hash: &config_hash,
        });

        assert_ne!(h1, h2);
    }

    #[test]
    fn test_compute_signed_data_varies_with_cluster() {
        let multisig = [1u8; 32];
        let config_hash = [2u8; 32];
        let message = b"hello";

        let h1 = compute_signed_data(&SignedDataPayload {
            version: 1,
            cluster: "devnet",
            multisig_address: &multisig,
            transaction_index: 42,
            vault_index: 0,
            message_bytes: message,
            config_state_hash: &config_hash,
        });
        let h2 = compute_signed_data(&SignedDataPayload {
            version: 1,
            cluster: "mainnet",
            multisig_address: &multisig,
            transaction_index: 42,
            vault_index: 0,
            message_bytes: message,
            config_state_hash: &config_hash,
        });

        assert_ne!(h1, h2);
    }

    #[test]
    fn test_export_and_verify_roundtrip() {
        let signer = TestSigner::new([42u8; 32]);

        let multisig = solana_pubkey::Pubkey::from([1u8; 32]);
        let member_pk = solana_pubkey::Pubkey::from([10u8; 32]);
        let program_id = solana_pubkey::Pubkey::from([2u8; 32]);
        let message_bytes = legacy_message(signer.pubkey, program_id, 1, &[1, 2, 3, 4]);
        let mut decoded_instructions = decoded_from_message(&message_bytes);
        decoded_instructions[0].decoded_name = Some("display-only-name".to_string());

        let params = ExportParams {
            version: 1,
            exporter_pubkey: signer.pubkey,
            exporter_label: Some("test".to_string()),
            multisig_address: multisig,
            threshold: 2,
            members: vec![(member_pk, 7)],
            create_key: [0u8; 32],
            transaction_index: 42,
            vault_index: 0,
            message_bytes,
            decoded_instructions,
            existing_signatures: vec![],
            cluster: "https://api.mainnet-beta.solana.com".to_string(),
        };

        let file = export_transaction(&params, &signer).unwrap_or_else(|e| panic!("{e}"));

        // Verify the envelope
        assert!(verify_envelope(&file).is_ok());

        // Verify fields
        assert_eq!(file.version, 1);
        assert_eq!(file.format, "squads-offline-v1");
        assert_eq!(file.transaction.index, 42);
        assert_eq!(file.multisig.threshold, 2);
        assert_eq!(file.multisig.members.len(), 1);
    }

    #[test]
    fn test_verify_envelope_detects_tampering() {
        let signer = TestSigner::new([42u8; 32]);

        let multisig = solana_pubkey::Pubkey::from([1u8; 32]);
        let program_id = solana_pubkey::Pubkey::from([2u8; 32]);
        let message_bytes = legacy_message(signer.pubkey, program_id, 0, &[1, 2, 3]);

        let params = ExportParams {
            version: 1,
            exporter_pubkey: signer.pubkey,
            exporter_label: None,
            multisig_address: multisig,
            threshold: 2,
            members: vec![],
            create_key: [0u8; 32],
            transaction_index: 1,
            vault_index: 0,
            message_bytes,
            decoded_instructions: vec![],
            existing_signatures: vec![],
            cluster: "devnet".to_string(),
        };

        let mut file = export_transaction(&params, &signer).unwrap_or_else(|e| panic!("{e}"));

        // Tamper with the message
        file.transaction.message_b64 =
            base64::Engine::encode(&base64::engine::general_purpose::STANDARD, b"tampered");

        let result = verify_envelope(&file);
        assert!(result.is_err());
    }

    #[test]
    fn test_verify_envelope_detects_decoded_instruction_tampering() {
        let signer = TestSigner::new([42u8; 32]);
        let multisig = solana_pubkey::Pubkey::from([1u8; 32]);
        let program_id = solana_pubkey::Pubkey::from([2u8; 32]);
        let message_bytes = legacy_message(signer.pubkey, program_id, 2, &[1, 2, 3]);
        let decoded_instructions = decoded_from_message(&message_bytes);

        let params = ExportParams {
            version: 1,
            exporter_pubkey: signer.pubkey,
            exporter_label: None,
            multisig_address: multisig,
            threshold: 1,
            members: vec![(signer.pubkey, 7)],
            create_key: [0u8; 32],
            transaction_index: 1,
            vault_index: 0,
            message_bytes,
            decoded_instructions,
            existing_signatures: vec![],
            cluster: "devnet".to_string(),
        };

        let mut file = export_transaction(&params, &signer).unwrap_or_else(|e| panic!("{e}"));
        file.transaction.decoded_instructions[0].data_len += 1;

        assert!(verify_envelope(&file).is_err());
    }

    #[test]
    fn test_verify_envelope_does_not_trust_advisory_names() {
        let signer = TestSigner::new([42u8; 32]);
        let multisig = solana_pubkey::Pubkey::from([1u8; 32]);
        let program_id = solana_pubkey::Pubkey::from([2u8; 32]);
        let message_bytes = legacy_message(signer.pubkey, program_id, 1, &[1, 2, 3]);
        let mut decoded_instructions = decoded_from_message(&message_bytes);
        decoded_instructions[0].decoded_name = Some("exported-name".to_string());

        let params = ExportParams {
            version: 1,
            exporter_pubkey: signer.pubkey,
            exporter_label: None,
            multisig_address: multisig,
            threshold: 1,
            members: vec![(signer.pubkey, 7)],
            create_key: [0u8; 32],
            transaction_index: 1,
            vault_index: 0,
            message_bytes: message_bytes.clone(),
            decoded_instructions,
            existing_signatures: vec![],
            cluster: "devnet".to_string(),
        };

        let mut file = export_transaction(&params, &signer).unwrap_or_else(|e| panic!("{e}"));
        file.transaction.decoded_instructions[0].decoded_name = Some("tampered-name".to_string());

        assert!(verify_envelope(&file).is_ok());
        assert_eq!(
            message_summary(&message_bytes)
                .unwrap_or_else(|e| panic!("{e}"))
                .decoded_instructions[0]
                .decoded_name,
            None
        );
    }

    #[test]
    fn test_verify_envelope_allows_missing_advisory_metadata() {
        let signer = TestSigner::new([42u8; 32]);
        let multisig = solana_pubkey::Pubkey::from([1u8; 32]);
        let program_id = solana_pubkey::Pubkey::from([2u8; 32]);
        let message_bytes = legacy_message(signer.pubkey, program_id, 1, &[1, 2, 3]);

        let params = ExportParams {
            version: 1,
            exporter_pubkey: signer.pubkey,
            exporter_label: None,
            multisig_address: multisig,
            threshold: 1,
            members: vec![(signer.pubkey, 7)],
            create_key: [0u8; 32],
            transaction_index: 1,
            vault_index: 0,
            message_bytes,
            decoded_instructions: vec![],
            existing_signatures: vec![],
            cluster: "devnet".to_string(),
        };

        let file = export_transaction(&params, &signer).unwrap_or_else(|e| panic!("{e}"));
        assert!(verify_envelope(&file).is_ok());
    }

    #[test]
    fn test_validate_cluster_match() {
        assert!(validate_cluster_match(
            "https://api.mainnet-beta.solana.com",
            "https://api.mainnet-beta.solana.com"
        )
        .is_ok());

        assert!(validate_cluster_match(
            "https://custom-mainnet.rpc.com",
            "https://api.mainnet-beta.solana.com"
        )
        .is_ok());

        assert!(validate_cluster_match(
            "https://api.devnet.solana.com",
            "https://api.mainnet-beta.solana.com"
        )
        .is_err());
    }

    #[test]
    fn test_read_offline_file_not_found() {
        let result = read_offline_file(std::path::Path::new("/nonexistent/file.sqds"));
        assert!(result.is_err());
        let msg = match result {
            Ok(_) => panic!("expected missing offline file to fail"),
            Err(e) => format!("{e}"),
        };
        assert!(msg.contains("not found"));
    }

    #[test]
    fn test_write_and_read_roundtrip() {
        let signer = TestSigner::new([42u8; 32]);
        let multisig = solana_pubkey::Pubkey::from([1u8; 32]);
        let program_id = solana_pubkey::Pubkey::from([2u8; 32]);
        let message_bytes = legacy_message(signer.pubkey, program_id, 0, &[1, 2, 3]);

        let params = ExportParams {
            version: 1,
            exporter_pubkey: signer.pubkey,
            exporter_label: None,
            multisig_address: multisig,
            threshold: 2,
            members: vec![],
            create_key: [0u8; 32],
            transaction_index: 1,
            vault_index: 0,
            message_bytes,
            decoded_instructions: vec![],
            existing_signatures: vec![],
            cluster: "devnet".to_string(),
        };

        let file = export_transaction(&params, &signer).unwrap_or_else(|e| panic!("{e}"));

        // Write to a temp file
        let dir = std::env::temp_dir();
        let path = dir.join("msig-test-roundtrip.sqds");
        write_offline_file(&file, &path).unwrap_or_else(|e| panic!("{e}"));

        // Read it back
        let loaded = read_offline_file(&path).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(loaded.version, 1);
        assert_eq!(loaded.transaction.index, 1);

        // Verify envelope still valid
        assert!(verify_envelope(&loaded).is_ok());

        // Cleanup
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_add_signature_and_build_wire_transaction() {
        let signer = TestSigner::new([7u8; 32]);
        let program_id = solana_pubkey::Pubkey::from([2u8; 32]);
        let blockhash = bs58::encode([9u8; 32]).into_string();
        let ix = crate::infra::instruction::Instruction {
            program_id,
            accounts: vec![],
            data: vec![1, 2, 3],
        };
        let message =
            crate::application::pipeline::compile_legacy_message(&[ix], &signer.pubkey, &blockhash)
                .unwrap_or_else(|e| panic!("{e}"));

        let params = ExportParams {
            version: 1,
            exporter_pubkey: signer.pubkey,
            exporter_label: None,
            multisig_address: solana_pubkey::Pubkey::from([1u8; 32]),
            threshold: 1,
            members: vec![(signer.pubkey, 7)],
            create_key: [0u8; 32],
            transaction_index: 1,
            vault_index: 0,
            message_bytes: message.clone(),
            decoded_instructions: vec![],
            existing_signatures: vec![],
            cluster: "devnet".to_string(),
        };

        let mut file = export_transaction(&params, &signer).unwrap_or_else(|e| panic!("{e}"));
        let signature = signer.sign(&message).unwrap_or_else(|e| panic!("{e}"));
        add_verified_signature(&mut file, signer.pubkey, &signature)
            .unwrap_or_else(|e| panic!("{e}"));

        let summary = message_summary(&message).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(summary.required_signers, vec![signer.pubkey]);
        assert_eq!(file.transaction.existing_signatures.len(), 1);

        let wire = build_signed_transaction(&file).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(wire[0], 1);
        assert_eq!(wire.len(), 1 + 64 + message.len());
    }
}
