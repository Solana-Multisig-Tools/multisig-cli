use solana_pubkey::Pubkey;

use crate::error::MsigError;
use crate::infra::config::Config;
use crate::infra::instruction::{AccountMeta, Instruction};
use crate::infra::pda;
use crate::infra::rpc::RpcProvider;
use crate::infra::signer::Signer;

use super::pipeline::{execute_transaction, execute_transaction_quiet, PreparedTransaction};

const SYSTEM_PROGRAM: Pubkey = solana_pubkey::pubkey!("11111111111111111111111111111111");
const MULTISIG_CREATE_V2_DISC: [u8; 8] = [0x32, 0xdd, 0xc7, 0x5d, 0x28, 0xf5, 0x8b, 0xe9];

fn generate_random_keypair() -> Result<[u8; 64], MsigError> {
    use ed25519_dalek::SigningKey;
    use std::io::Read;
    let mut secret_bytes = zeroize::Zeroizing::new([0u8; 32]);
    std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut *secret_bytes))
        .map_err(|e| MsigError::Signing(format!("failed to get OS entropy: {e}")))?;
    let signing_key = SigningKey::from_bytes(&secret_bytes);
    let public_key = signing_key.verifying_key();
    let mut keypair = [0u8; 64];
    keypair[..32].copy_from_slice(&*secret_bytes);
    keypair[32..].copy_from_slice(public_key.as_bytes());
    drop(signing_key);
    Ok(keypair)
}

/// Derive the programConfig PDA for Squads v4.
/// Seeds: ["multisig", "program_config"]
fn program_config_pda(program_id: &Pubkey) -> Pubkey {
    let (pda, _) = pda::find_program_address(
        &[b"multisig".as_ref(), b"program_config".as_ref()],
        program_id,
    );
    pda
}

/// Fetch the treasury address from the programConfig account.
/// Layout: 8 bytes discriminator, then fields. Treasury is the second
/// field after `config_authority: Pubkey` (32 bytes), so at offset 8+32 = 40.
fn fetch_treasury(rpc: &dyn RpcProvider, program_id: &Pubkey) -> Result<Pubkey, MsigError> {
    let config_pda = program_config_pda(program_id);
    let config_str = config_pda.to_string();

    let account = rpc.get_account_info(&config_str)?.ok_or_else(|| {
        MsigError::Config(
            "Squads programConfig account not found. Is the program deployed on this cluster?"
                .into(),
        )
    })?;

    // ProgramConfig layout: discriminator(8) + authority(32) + multisig_creation_fee(8) + treasury(32) + ...
    if account.data.len() < 80 {
        return Err(MsigError::Transaction(
            "programConfig account data too short to read treasury".into(),
        ));
    }

    let mut treasury_bytes = [0u8; 32];
    treasury_bytes.copy_from_slice(&account.data[48..80]);
    Ok(Pubkey::from(treasury_bytes))
}

#[allow(clippy::too_many_arguments)]
fn build_multisig_create_v2_instruction(
    program_id: Pubkey,
    program_config: Pubkey,
    treasury: Pubkey,
    multisig: Pubkey,
    create_key: Pubkey,
    creator: Pubkey,
    threshold: u16,
    members: &[Pubkey],
    rent_collector: Option<Pubkey>,
) -> Instruction {
    let mut data = Vec::new();
    data.extend_from_slice(&MULTISIG_CREATE_V2_DISC);
    data.push(0x00);
    data.extend_from_slice(&threshold.to_le_bytes());
    data.extend_from_slice(&(members.len() as u32).to_le_bytes());
    for pk in members {
        data.extend_from_slice(pk.as_ref());
        data.push(0b111);
    }
    data.extend_from_slice(&0u32.to_le_bytes());
    match rent_collector {
        Some(rc) => {
            data.push(0x01);
            data.extend_from_slice(rc.as_ref());
        }
        None => data.push(0x00),
    }
    data.push(0x00);

    Instruction {
        program_id,
        accounts: vec![
            AccountMeta::new_readonly(program_config, false),
            AccountMeta::new(treasury, false),
            AccountMeta::new(multisig, false),
            AccountMeta::new_readonly(create_key, true),
            AccountMeta::new(creator, true),
            AccountMeta::new_readonly(SYSTEM_PROGRAM, false),
        ],
        data,
    }
}

#[allow(clippy::too_many_arguments)]
pub fn create_multisig(
    rpc: &dyn RpcProvider,
    signer: &dyn Signer,
    threshold: u16,
    members: &[String],
    rent_collector: Option<&str>,
    config: &Config,
    dry_run: bool,
    skip_confirm: bool,
) -> Result<MultisigCreateResult, MsigError> {
    create_multisig_inner(
        rpc,
        signer,
        threshold,
        members,
        rent_collector,
        config,
        dry_run,
        skip_confirm,
        false,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn create_multisig_quiet(
    rpc: &dyn RpcProvider,
    signer: &dyn Signer,
    threshold: u16,
    members: &[String],
    rent_collector: Option<&str>,
    config: &Config,
    dry_run: bool,
    skip_confirm: bool,
) -> Result<MultisigCreateResult, MsigError> {
    create_multisig_inner(
        rpc,
        signer,
        threshold,
        members,
        rent_collector,
        config,
        dry_run,
        skip_confirm,
        true,
    )
}

#[allow(clippy::too_many_arguments)]
fn create_multisig_inner(
    rpc: &dyn RpcProvider,
    signer: &dyn Signer,
    threshold: u16,
    members: &[String],
    rent_collector: Option<&str>,
    config: &Config,
    dry_run: bool,
    skip_confirm: bool,
    quiet: bool,
) -> Result<MultisigCreateResult, MsigError> {
    let program_id = config.program_id;
    let creator = signer.pubkey();

    // Fetch programConfig and treasury
    let config_pda = program_config_pda(&program_id);
    let treasury = fetch_treasury(rpc, &program_id)?;

    let create_key_bytes = generate_random_keypair()?;
    let create_key_pubkey = Pubkey::from(
        <[u8; 32]>::try_from(&create_key_bytes[32..64])
            .map_err(|_| MsigError::Signing("failed to extract create_key public key".into()))?,
    );
    let (multisig_pda, _) = pda::multisig_pda(&create_key_pubkey, &program_id);

    let member_pubkeys: Vec<Pubkey> = members
        .iter()
        .map(|m| {
            m.parse()
                .map_err(|_| MsigError::Usage(format!("invalid member address: '{m}'")))
        })
        .collect::<Result<_, _>>()?;

    let rent_collector_pubkey: Option<Pubkey> = rent_collector
        .map(|rc| {
            rc.parse()
                .map_err(|_| MsigError::Usage(format!("invalid rent-collector address: '{rc}'")))
        })
        .transpose()?;

    let instruction = build_multisig_create_v2_instruction(
        program_id,
        config_pda,
        treasury,
        multisig_pda,
        create_key_pubkey,
        creator,
        threshold,
        &member_pubkeys,
        rent_collector_pubkey,
    );
    let prepared = PreparedTransaction {
        instructions: vec![instruction],
        description: format!(
            "Create multisig with {} members, threshold {}/{}",
            members.len(),
            threshold,
            members.len()
        ),
        review_instructions: vec![],
        extra_signers: vec![zeroize::Zeroizing::new(create_key_bytes)],
    };

    let result = if quiet {
        execute_transaction_quiet(rpc, signer, prepared, config, dry_run, skip_confirm)?
    } else {
        execute_transaction(rpc, signer, prepared, config, dry_run, skip_confirm)?
    };

    if !quiet && result.is_some() {
        println!("Multisig created: {multisig_pda}");
        println!("Create key: {create_key_pubkey}");
        println!();
        println!("Save this multisig address in your config:");
        println!("  msig config set multisig {multisig_pda}");
    } else if !quiet && dry_run {
        println!("Multisig would be created at: {multisig_pda}");
        println!("Create key: {create_key_pubkey}");
    }

    Ok(MultisigCreateResult {
        signature: result,
        multisig_address: multisig_pda.to_string(),
        create_key: create_key_pubkey.to_string(),
    })
}

/// Result of a multisig creation, including derived addresses.
pub struct MultisigCreateResult {
    pub signature: Option<String>,
    pub multisig_address: String,
    pub create_key: String,
}
