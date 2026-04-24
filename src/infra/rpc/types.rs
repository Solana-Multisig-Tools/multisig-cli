use serde::{Deserialize, Deserializer, Serialize};

#[derive(Debug, Serialize)]
pub struct RpcRequest<'a> {
    pub jsonrpc: &'static str,
    pub id: u64,
    pub method: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
}

#[derive(Debug)]
#[allow(dead_code)]
pub struct RpcResponse<T> {
    pub jsonrpc: String,
    pub id: u64,
    pub result: Option<T>,
    pub error: Option<RpcErrorDetail>,
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for RpcResponse<T> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct Helper {
            jsonrpc: String,
            id: u64,
            result: Option<serde_json::Value>,
            error: Option<RpcErrorDetail>,
        }

        let helper = Helper::deserialize(deserializer)?;
        let result = match helper.result {
            Some(v) => Some(T::deserialize(v).map_err(serde::de::Error::custom)?),
            None => None,
        };
        Ok(RpcResponse {
            jsonrpc: helper.jsonrpc,
            id: helper.id,
            result,
            error: helper.error,
        })
    }
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct RpcErrorDetail {
    pub code: i64,
    pub message: String,
    #[serde(default)]
    pub data: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub struct AccountInfoResult {
    pub value: Option<AccountInfoValue>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct AccountInfoValue {
    pub lamports: u64,
    pub data: Vec<String>,
    pub owner: String,
    pub executable: bool,
    #[serde(rename = "rentEpoch")]
    pub rent_epoch: u64,
}

impl AccountInfoValue {
    pub fn decode_data(&self) -> Result<Vec<u8>, crate::error::MsigError> {
        use base64::Engine;
        let encoded = self.data.first().map(|s| s.as_str()).unwrap_or("");
        base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .map_err(|e| {
                crate::error::MsigError::Rpc(crate::error::RpcError::Connection(format!(
                    "base64 decode failed: {e}"
                )))
            })
    }
}

#[derive(Debug, Deserialize)]
pub struct MultipleAccountsResult {
    pub value: Vec<Option<AccountInfoValue>>,
}

#[derive(Debug, Deserialize)]
pub struct BlockhashResult {
    pub value: BlockhashValue,
}

#[derive(Debug, Deserialize)]
pub struct BlockhashValue {
    pub blockhash: String,
    #[serde(rename = "lastValidBlockHeight")]
    pub last_valid_block_height: u64,
}

#[derive(Debug, Deserialize)]
pub struct SimulateResult {
    pub value: SimulateValue,
}

#[derive(Debug, Deserialize)]
pub struct SimulateValue {
    pub err: Option<serde_json::Value>,
    #[serde(default)]
    pub logs: Option<Vec<String>>,
    #[serde(rename = "unitsConsumed")]
    #[serde(default)]
    pub units_consumed: Option<u64>,
    #[serde(default)]
    pub accounts: Option<Vec<Option<AccountInfoValue>>>,
}

#[derive(Debug)]
#[allow(dead_code)]
pub struct SimulationResult {
    pub err: Option<serde_json::Value>,
    pub logs: Vec<String>,
    pub units_consumed: u64,
    pub accounts: Vec<Option<super::AccountData>>,
}

#[derive(Debug, Deserialize)]
pub struct SignatureStatusesResult {
    pub value: Vec<Option<SignatureStatus>>,
}

#[derive(Debug, Deserialize)]
pub struct SignatureStatus {
    pub slot: u64,
    pub confirmations: Option<u64>,
    pub err: Option<serde_json::Value>,
    #[serde(rename = "confirmationStatus")]
    pub confirmation_status: Option<String>,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct TxStatus {
    pub slot: u64,
    pub confirmations: Option<u64>,
    pub err: Option<serde_json::Value>,
    pub confirmation_status: Option<String>,
}

impl From<SignatureStatus> for TxStatus {
    fn from(s: SignatureStatus) -> Self {
        Self {
            slot: s.slot,
            confirmations: s.confirmations,
            err: s.err,
            confirmation_status: s.confirmation_status,
        }
    }
}
