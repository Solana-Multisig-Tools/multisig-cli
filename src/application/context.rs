use solana_pubkey::Pubkey;

use crate::error::MsigError;
use crate::infra::config::Config;
use crate::infra::rpc::{RpcProvider, SolanaRpcClient};
use crate::infra::signer::Signer;

/// Immutable runtime context built once at startup and threaded through every command.
pub struct CommandContext {
    pub config: Config,
    pub rpc: SolanaRpcClient,
    pub signer: Box<dyn Signer>,
    pub program_id: Pubkey,
}

impl CommandContext {
    /// Build a CommandContext from resolved config and flags.
    pub fn build(
        config: Config,
        ledger_flag: Option<&str>,
        keypair_flag: Option<&str>,
        program_id_override: Option<&str>,
    ) -> Result<Self, MsigError> {
        let rpc = SolanaRpcClient::with_commitment(&config.cluster, &config.commitment);

        let signer = crate::infra::signer::resolve_signer(
            ledger_flag,
            keypair_flag,
            config.keypair.as_deref(),
        )?;

        let program_id = match program_id_override {
            Some(id) => id
                .parse()
                .map_err(|_| MsigError::Usage(format!("invalid program-id: '{id}'")))?,
            None => config.program_id,
        };

        Ok(Self {
            config,
            rpc,
            signer,
            program_id,
        })
    }

    /// Resolve multisig address from an optional override, falling back to config.
    pub fn resolve_multisig(&self, override_addr: Option<&str>) -> Result<Pubkey, MsigError> {
        let addr_str = override_addr
            .or(self.config.multisig.as_deref())
            .ok_or_else(|| {
                MsigError::Config(
                    "no multisig address. Use --multisig <ADDR> or set 'multisig' in config."
                        .into(),
                )
            })?;

        // Try label resolution first
        let resolved =
            crate::infra::config::labels::resolve_address(addr_str, &self.config.labels)?;
        resolved
            .parse()
            .map_err(|_| MsigError::Usage(format!("invalid multisig address: '{resolved}'")))
    }

    /// Get the RPC provider (for passing to functions that take &dyn RpcProvider).
    pub fn rpc(&self) -> &dyn RpcProvider {
        &self.rpc
    }

    /// Get vault index, with optional override.
    pub fn vault_index(&self, override_val: Option<u8>) -> u8 {
        override_val.unwrap_or(self.config.vault_index)
    }
}
