use solana_pubkey::Pubkey;

const MAX_SEEDS: usize = 16;
const MAX_SEED_LEN: usize = 32;
const PDA_MARKER: &[u8] = b"ProgramDerivedAddress";

/// Squads Multisig Program v4 program ID.
pub const PROGRAM_ID: Pubkey =
    solana_pubkey::pubkey!("SQDS4ep65T869zMMBKyuUq6aD6EgTu8psMjkvj52pCf");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PdaError {
    MaxSeedLengthExceeded,
    InvalidSeeds,
}

fn bytes_are_curve_point(bytes: [u8; 32]) -> bool {
    curve25519_dalek::edwards::CompressedEdwardsY(bytes)
        .decompress()
        .is_some()
}

fn create_program_address(seeds: &[&[u8]], program_id: &Pubkey) -> Result<Pubkey, PdaError> {
    if seeds.len() > MAX_SEEDS || seeds.iter().any(|seed| seed.len() > MAX_SEED_LEN) {
        return Err(PdaError::MaxSeedLengthExceeded);
    }

    let mut hasher = <sha2::Sha256 as sha2::Digest>::new();
    for seed in seeds {
        sha2::Digest::update(&mut hasher, seed);
    }
    sha2::Digest::update(&mut hasher, program_id.as_ref());
    sha2::Digest::update(&mut hasher, PDA_MARKER);
    let hash: [u8; 32] = sha2::Digest::finalize(hasher).into();

    if bytes_are_curve_point(hash) {
        return Err(PdaError::InvalidSeeds);
    }

    Ok(Pubkey::new_from_array(hash))
}

pub(crate) fn find_program_address(seeds: &[&[u8]], program_id: &Pubkey) -> (Pubkey, u8) {
    let mut bump_seed = [u8::MAX];
    for _ in 0..u8::MAX {
        let mut seeds_with_bump = seeds.to_vec();
        seeds_with_bump.push(&bump_seed);
        match create_program_address(&seeds_with_bump, program_id) {
            Ok(address) => return (address, bump_seed[0]),
            Err(PdaError::InvalidSeeds) => bump_seed[0] -= 1,
            Err(PdaError::MaxSeedLengthExceeded) => break,
        }
    }
    panic!("unable to find a viable program address bump seed")
}

/// Seeds: ["multisig", "multisig", create_key]
pub fn multisig_pda(create_key: &Pubkey, program_id: &Pubkey) -> (Pubkey, u8) {
    find_program_address(
        &[
            b"multisig".as_ref(),
            b"multisig".as_ref(),
            create_key.as_ref(),
        ],
        program_id,
    )
}

pub fn vault_pda(multisig: &Pubkey, vault_index: u8, program_id: &Pubkey) -> (Pubkey, u8) {
    find_program_address(
        &[
            b"multisig".as_ref(),
            multisig.as_ref(),
            b"vault".as_ref(),
            &[vault_index],
        ],
        program_id,
    )
}

pub fn transaction_pda(multisig: &Pubkey, index: u64, program_id: &Pubkey) -> (Pubkey, u8) {
    find_program_address(
        &[
            b"multisig".as_ref(),
            multisig.as_ref(),
            b"transaction".as_ref(),
            &index.to_le_bytes(),
        ],
        program_id,
    )
}

pub fn proposal_pda(multisig: &Pubkey, index: u64, program_id: &Pubkey) -> (Pubkey, u8) {
    find_program_address(
        &[
            b"multisig".as_ref(),
            multisig.as_ref(),
            b"transaction".as_ref(),
            &index.to_le_bytes(),
            b"proposal".as_ref(),
        ],
        program_id,
    )
}

pub fn spending_limit_pda(
    multisig: &Pubkey,
    create_key: &Pubkey,
    program_id: &Pubkey,
) -> (Pubkey, u8) {
    find_program_address(
        &[
            b"multisig".as_ref(),
            multisig.as_ref(),
            b"spending_limit".as_ref(),
            create_key.as_ref(),
        ],
        program_id,
    )
}

const TOKEN_PROGRAM: Pubkey = solana_pubkey::pubkey!("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA");

const ATA_PROGRAM: Pubkey = solana_pubkey::pubkey!("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL");

pub fn derive_ata(wallet: &Pubkey, mint: &Pubkey) -> Pubkey {
    derive_ata_with_token_program(wallet, mint, &TOKEN_PROGRAM)
}

pub fn derive_ata_with_token_program(
    wallet: &Pubkey,
    mint: &Pubkey,
    token_program: &Pubkey,
) -> Pubkey {
    let (ata, _) = find_program_address(
        &[wallet.as_ref(), token_program.as_ref(), mint.as_ref()],
        &ATA_PROGRAM,
    );
    ata
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn create_program_address_matches_solana_vectors() {
        let program_id = Pubkey::from_str("BPFLoaderUpgradeab1e11111111111111111111111")
            .unwrap_or_else(|e| panic!("{e}"));
        let public_key = Pubkey::from_str("SeedPubey1111111111111111111111111111111111")
            .unwrap_or_else(|e| panic!("{e}"));

        assert_eq!(
            create_program_address(&[b"", &[1]], &program_id),
            Ok(
                Pubkey::from_str("BwqrghZA2htAcqq8dzP1WDAhTXYTYWj7CHxF5j7TDBAe")
                    .unwrap_or_else(|e| panic!("{e}"))
            )
        );
        assert_eq!(
            create_program_address(&["☉".as_ref(), &[0]], &program_id),
            Ok(
                Pubkey::from_str("13yWmRpaTR4r5nAktwLqMpRNr28tnVUZw26rTvPSSB19")
                    .unwrap_or_else(|e| panic!("{e}"))
            )
        );
        assert_eq!(
            create_program_address(&[b"Talking", b"Squirrels"], &program_id),
            Ok(
                Pubkey::from_str("2fnQrngrQT4SeLcdToJAD96phoEjNL2man2kfRLCASVk")
                    .unwrap_or_else(|e| panic!("{e}"))
            )
        );
        assert_eq!(
            create_program_address(&[public_key.as_ref(), &[1]], &program_id),
            Ok(
                Pubkey::from_str("976ymqVnfE32QFe6NfGDctSvVa36LWnvYxhU6G2232YL")
                    .unwrap_or_else(|e| panic!("{e}"))
            )
        );
    }

    #[test]
    fn create_program_address_rejects_oversized_seed_input() {
        let program_id = Pubkey::from_str("BPFLoaderUpgradeab1e11111111111111111111111")
            .unwrap_or_else(|e| panic!("{e}"));
        let exceeded_seed = &[127; MAX_SEED_LEN + 1];
        let exceeded_seeds: &[&[u8]] = &[
            &[1],
            &[2],
            &[3],
            &[4],
            &[5],
            &[6],
            &[7],
            &[8],
            &[9],
            &[10],
            &[11],
            &[12],
            &[13],
            &[14],
            &[15],
            &[16],
            &[17],
        ];

        assert_eq!(
            create_program_address(&[exceeded_seed], &program_id),
            Err(PdaError::MaxSeedLengthExceeded)
        );
        assert_eq!(
            create_program_address(exceeded_seeds, &program_id),
            Err(PdaError::MaxSeedLengthExceeded)
        );
    }

    #[test]
    fn create_program_address_rejects_on_curve_hashes() {
        let program_id = Pubkey::from_str("BPFLoaderUpgradeab1e11111111111111111111111")
            .unwrap_or_else(|e| panic!("{e}"));
        let Some(on_curve_seed) = (0u16..=u16::MAX).map(u16::to_le_bytes).find(|seed| {
            matches!(
                create_program_address(&[seed.as_ref()], &program_id),
                Err(PdaError::InvalidSeeds)
            )
        }) else {
            panic!("test fixture should find an on-curve PDA hash");
        };

        assert_eq!(
            create_program_address(&[on_curve_seed.as_ref()], &program_id),
            Err(PdaError::InvalidSeeds)
        );
    }

    #[test]
    fn find_program_address_returns_highest_off_curve_bump() {
        let program_id = Pubkey::from_str("BPFLoaderUpgradeab1e11111111111111111111111")
            .unwrap_or_else(|e| panic!("{e}"));
        let Some(seed) = (0u16..=u16::MAX).map(u16::to_le_bytes).find(|seed| {
            matches!(
                create_program_address(&[seed.as_ref(), &[u8::MAX]], &program_id),
                Err(PdaError::InvalidSeeds)
            ) && create_program_address(&[seed.as_ref(), &[u8::MAX - 1]], &program_id).is_ok()
        }) else {
            panic!("test fixture should find a seed that skips bump 255");
        };

        let (address, bump) = find_program_address(&[seed.as_ref()], &program_id);
        let expected = match create_program_address(&[seed.as_ref(), &[u8::MAX - 1]], &program_id) {
            Ok(address) => address,
            Err(err) => panic!("expected bump 254 to be off-curve, got {err:?}"),
        };

        assert_eq!(bump, u8::MAX - 1);
        assert_eq!(address, expected);
    }

    #[test]
    fn pda_derivations_are_deterministic() {
        let create_key = Pubkey::new_unique();
        let (msig1, bump1) = multisig_pda(&create_key, &PROGRAM_ID);
        let (msig2, bump2) = multisig_pda(&create_key, &PROGRAM_ID);
        assert_eq!(msig1, msig2);
        assert_eq!(bump1, bump2);
    }

    #[test]
    fn vault_pda_varies_by_index() {
        let multisig = Pubkey::new_unique();
        let (vault0, _) = vault_pda(&multisig, 0, &PROGRAM_ID);
        let (vault1, _) = vault_pda(&multisig, 1, &PROGRAM_ID);
        assert_ne!(vault0, vault1);
    }

    #[test]
    fn transaction_and_proposal_pda_differ() {
        let multisig = Pubkey::new_unique();
        let (tx, _) = transaction_pda(&multisig, 1, &PROGRAM_ID);
        let (prop, _) = proposal_pda(&multisig, 1, &PROGRAM_ID);
        assert_ne!(tx, prop);
    }
}
