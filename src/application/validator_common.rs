//! Helpers shared by the validator commands (`vote-account`, `stake-account`):
//! the multisig's vault set, the standalone simulation of inner instructions,
//! and the small byte readers used by the review decoders.

use std::collections::HashMap;

use solana_pubkey::Pubkey;

use crate::error::MsigError;
use crate::infra::instruction::Instruction;
use crate::infra::pda;
use crate::infra::rpc::RpcProvider;
use crate::sanitize::sanitize_display;

use super::pipeline::compile_unsigned_legacy_transaction_base64;

pub const SYSVAR_CLOCK: Pubkey =
    solana_pubkey::pubkey!("SysvarC1ock11111111111111111111111111111111");

/// All 256 vault PDAs of a multisig, keyed by address.
pub fn vault_addresses(multisig: &Pubkey, program_id: &Pubkey) -> HashMap<Pubkey, u8> {
    (0..=u8::MAX)
        .map(|index| (pda::vault_pda(multisig, index, program_id).0, index))
        .collect()
}

/// A new withdrawer outside the multisig hands the account to someone else.
pub fn is_external_withdrawer(new_withdrawer: &Pubkey, vaults: &HashMap<Pubkey, u8>) -> bool {
    !vaults.contains_key(new_withdrawer)
}

/// Simulate the inner instructions on their own, fee payer = proposer, before
/// anything is proposed. The Squads pipeline only simulates the outer
/// proposal-creation transaction, which succeeds whatever the inner ops do.
///
/// `allow_failure` exists because some operations legitimately fail now but
/// succeed at execution time (a commission change outside the allowed part of
/// the epoch, an authority change that is already queued, a withdrawal after
/// the stake cooldown).
pub fn simulate_inner_instructions(
    rpc: &dyn RpcProvider,
    payer: &Pubkey,
    instructions: &[Instruction],
    allow_failure: bool,
) -> Result<(), MsigError> {
    let (blockhash, _) = rpc.get_latest_blockhash()?;
    let tx = compile_unsigned_legacy_transaction_base64(instructions, payer, &blockhash)?;
    let result = rpc.simulate_transaction(&tx)?;
    let Some(err) = result.err else {
        return Ok(());
    };

    let err = sanitize_display(&err.to_string());
    eprintln!("Inner instruction simulation failed: {err}");
    if !result.logs.is_empty() {
        eprintln!("Simulation logs:");
        for log in &result.logs {
            eprintln!("  {}", sanitize_display(log));
        }
    }
    if allow_failure {
        eprintln!(
            "Warning: proposing anyway because --allow-simulation-failure was passed. \
             The proposal may fail at execution."
        );
        return Ok(());
    }
    Err(MsigError::SimulationFailed(format!(
        "inner instructions failed: {err}. Nothing was proposed. Pass \
         --allow-simulation-failure only if the operation is expected to succeed at execution time."
    )))
}

/// Human-readable decoding of one instruction plus review warnings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedInstruction {
    pub text: String,
    pub warnings: Vec<String>,
}

pub(crate) fn read_u32(data: &[u8], off: usize) -> Option<u32> {
    data.get(off..off.checked_add(4)?)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

pub(crate) fn read_u64(data: &[u8], off: usize) -> Option<u64> {
    data.get(off..off.checked_add(8)?)
        .map(|b| u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]))
}

pub(crate) fn read_i64(data: &[u8], off: usize) -> Option<i64> {
    read_u64(data, off).map(|v| i64::from_le_bytes(v.to_le_bytes()))
}

pub(crate) fn read_pubkey(data: &[u8], off: usize) -> Option<Pubkey> {
    let bytes: [u8; 32] = data.get(off..off.checked_add(32)?)?.try_into().ok()?;
    Some(Pubkey::from(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readers_are_bounds_checked() {
        let data = [1u8, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0];
        assert_eq!(read_u32(&data, 0), Some(1));
        assert_eq!(read_u64(&data, 4), Some(2));
        assert_eq!(read_u64(&data, 5), None);
        assert_eq!(read_u32(&data, usize::MAX), None);
        assert_eq!(read_pubkey(&data, 0), None);
        assert_eq!(read_i64(&[0xff; 8], 0), Some(-1));
    }
}
