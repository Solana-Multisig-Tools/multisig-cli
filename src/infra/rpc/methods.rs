use crate::error::{MsigError, RpcError};
use crate::infra::rpc::types::*;
use crate::infra::rpc::SolanaRpcClient;

impl SolanaRpcClient {
    pub fn get_account_info(&self, pubkey: &str) -> Result<Option<super::AccountData>, MsigError> {
        let params =
            serde_json::json!([pubkey, { "encoding": "base64", "commitment": self.commitment() }]);
        let resp: RpcResponse<AccountInfoResult> = self.call_raw("getAccountInfo", Some(params))?;
        let result = resp
            .result
            .ok_or_else(|| MsigError::Rpc(RpcError::Connection("missing result".into())))?;
        match result.value {
            None => Ok(None),
            Some(val) => {
                let data = val.decode_data()?;
                Ok(Some(super::AccountData {
                    lamports: val.lamports,
                    data,
                    owner: val.owner,
                }))
            }
        }
    }

    pub fn get_multiple_accounts(
        &self,
        pubkeys: &[&str],
    ) -> Result<Vec<Option<super::AccountData>>, MsigError> {
        let mut results = Vec::with_capacity(pubkeys.len());
        for chunk in pubkeys.chunks(100) {
            let keys: Vec<&str> = chunk.to_vec();
            let params = serde_json::json!([keys, { "encoding": "base64", "commitment": self.commitment() }]);
            let resp: RpcResponse<MultipleAccountsResult> =
                self.call_raw("getMultipleAccounts", Some(params))?;
            let result = resp
                .result
                .ok_or_else(|| MsigError::Rpc(RpcError::Connection("missing result".into())))?;
            for maybe_val in result.value {
                match maybe_val {
                    None => results.push(None),
                    Some(val) => {
                        let data = val.decode_data()?;
                        results.push(Some(super::AccountData {
                            lamports: val.lamports,
                            data,
                            owner: val.owner,
                        }));
                    }
                }
            }
        }
        Ok(results)
    }

    pub fn get_latest_blockhash(&self) -> Result<(String, u64), MsigError> {
        let params = serde_json::json!([{ "commitment": self.commitment() }]);
        let resp: RpcResponse<BlockhashResult> =
            self.call_raw("getLatestBlockhash", Some(params))?;
        let result = resp
            .result
            .ok_or_else(|| MsigError::Rpc(RpcError::Connection("missing result".into())))?;
        Ok((result.value.blockhash, result.value.last_valid_block_height))
    }

    pub fn send_transaction(&self, base64_tx: &str) -> Result<String, MsigError> {
        let params = serde_json::json!([base64_tx, { "encoding": "base64", "skipPreflight": false, "preflightCommitment": self.commitment() }]);
        let resp: RpcResponse<String> = self.call_raw("sendTransaction", Some(params))?;
        resp.result
            .ok_or_else(|| MsigError::Rpc(RpcError::Connection("missing result".into())))
    }

    pub fn simulate_transaction(&self, base64_tx: &str) -> Result<SimulationResult, MsigError> {
        let params = serde_json::json!([base64_tx, { "encoding": "base64", "replaceRecentBlockhash": true, "sigVerify": false, "commitment": self.commitment() }]);
        self.simulate_transaction_inner(params)
    }

    pub fn simulate_transaction_with_accounts(
        &self,
        base64_tx: &str,
        accounts: &[String],
    ) -> Result<SimulationResult, MsigError> {
        let params = serde_json::json!([
            base64_tx,
            {
                "encoding": "base64",
                "replaceRecentBlockhash": true,
                "sigVerify": false,
                "commitment": self.commitment(),
                "accounts": {
                    "encoding": "base64",
                    "addresses": accounts,
                }
            }
        ]);
        self.simulate_transaction_inner(params)
    }

    fn simulate_transaction_inner(
        &self,
        params: serde_json::Value,
    ) -> Result<SimulationResult, MsigError> {
        let resp: RpcResponse<SimulateResult> =
            self.call_raw("simulateTransaction", Some(params))?;
        let result = resp
            .result
            .ok_or_else(|| MsigError::Rpc(RpcError::Connection("missing result".into())))?;
        let accounts = result
            .value
            .accounts
            .unwrap_or_default()
            .into_iter()
            .map(|maybe_val| {
                maybe_val
                    .map(|val| {
                        val.decode_data().map(|data| super::AccountData {
                            lamports: val.lamports,
                            data,
                            owner: val.owner,
                        })
                    })
                    .transpose()
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(SimulationResult {
            err: result.value.err,
            logs: result.value.logs.unwrap_or_default(),
            units_consumed: result.value.units_consumed.unwrap_or(0),
            accounts,
        })
    }

    pub fn get_signature_statuses(
        &self,
        sigs: &[&str],
    ) -> Result<Vec<Option<TxStatus>>, MsigError> {
        let params = serde_json::json!([sigs]);
        let resp: RpcResponse<SignatureStatusesResult> =
            self.call_raw("getSignatureStatuses", Some(params))?;
        let result = resp
            .result
            .ok_or_else(|| MsigError::Rpc(RpcError::Connection("missing result".into())))?;
        Ok(result
            .value
            .into_iter()
            .map(|opt| opt.map(TxStatus::from))
            .collect())
    }
}
