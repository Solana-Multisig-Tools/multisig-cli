pub mod methods;
pub mod retry;
pub mod types;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::de::DeserializeOwned;

use crate::error::{MsigError, RpcError};
use types::{RpcRequest, RpcResponse};

/// Decoded on-chain account data.
#[derive(Debug, Clone)]
pub struct AccountData {
    pub lamports: u64,
    pub data: Vec<u8>,
    pub owner: String,
}

/// RPC abstraction. Implemented by SolanaRpcClient.
/// Also used for mock testing.
pub trait RpcProvider {
    fn get_account_info(&self, pubkey: &str) -> Result<Option<AccountData>, MsigError>;
    fn get_multiple_accounts(
        &self,
        pubkeys: &[&str],
    ) -> Result<Vec<Option<AccountData>>, MsigError>;
    fn get_latest_blockhash(&self) -> Result<(String, u64), MsigError>;
    fn send_transaction(&self, base64_tx: &str) -> Result<String, MsigError>;
    fn simulate_transaction(&self, base64_tx: &str) -> Result<types::SimulationResult, MsigError>;
    fn get_signature_statuses(
        &self,
        sigs: &[&str],
    ) -> Result<Vec<Option<types::TxStatus>>, MsigError>;
}

struct RateLimiter {
    tokens: f64,
    last_refill: Instant,
    max_tokens: f64,
    refill_rate: f64,
}

impl RateLimiter {
    fn new(max_tokens: f64, refill_rate: f64) -> Self {
        Self {
            tokens: max_tokens,
            last_refill: Instant::now(),
            max_tokens,
            refill_rate,
        }
    }

    fn acquire(&mut self) {
        self.refill();
        while self.tokens < 1.0 {
            let wait = Duration::from_secs_f64((1.0 - self.tokens) / self.refill_rate);
            std::thread::sleep(wait);
            self.refill();
        }
        self.tokens -= 1.0;
    }

    fn refill(&mut self) {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_refill).as_secs_f64();
        self.tokens = (self.tokens + elapsed * self.refill_rate).min(self.max_tokens);
        self.last_refill = now;
    }
}

pub struct SolanaRpcClient {
    agent: ureq::Agent,
    url: String,
    commitment: String,
    request_id: AtomicU64,
    rate_limiter: Mutex<RateLimiter>,
    max_retries: u32,
}

impl SolanaRpcClient {
    pub fn with_commitment(url: &str, commitment: &str) -> Self {
        let config = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(10)))
            .http_status_as_error(false)
            .build();
        let agent: ureq::Agent = config.into();

        Self {
            agent,
            url: url.to_string(),
            commitment: commitment.to_string(),
            request_id: AtomicU64::new(1),
            rate_limiter: Mutex::new(RateLimiter::new(10.0, 10.0)),
            max_retries: 3,
        }
    }

    pub fn commitment(&self) -> &str {
        &self.commitment
    }

    fn safe_url(&self) -> String {
        match self.url.find('?') {
            Some(idx) => format!("{}?<redacted>", &self.url[..idx]),
            None => self.url.clone(),
        }
    }

    pub(crate) fn call_raw<T: DeserializeOwned>(
        &self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Result<RpcResponse<T>, MsigError> {
        let id = self.request_id.fetch_add(1, Ordering::Relaxed);
        let request = RpcRequest {
            jsonrpc: "2.0",
            id,
            method,
            params,
        };

        let body = serde_json::to_vec(&request)
            .map_err(|e| MsigError::Rpc(RpcError::Connection(format!("serialize request: {e}"))))?;

        let mut last_err: Option<MsigError> = None;

        for attempt in 0..=self.max_retries {
            if attempt > 0 {
                retry::sleep_before_retry(attempt - 1);
            }

            if let Ok(mut limiter) = self.rate_limiter.lock() {
                limiter.acquire();
            }

            match self.do_request(&body) {
                Ok(resp_body) => {
                    let parsed: RpcResponse<T> = serde_json::from_str(&resp_body).map_err(|e| {
                        MsigError::Rpc(RpcError::Connection(format!(
                            "parse response from {}: {e}",
                            self.safe_url()
                        )))
                    })?;

                    if let Some(ref rpc_err) = parsed.error {
                        return Err(MsigError::Rpc(RpcError::JsonRpc {
                            code: rpc_err.code,
                            message: rpc_err.message.clone(),
                        }));
                    }

                    return Ok(parsed);
                }
                Err(e) => {
                    if attempt < self.max_retries && is_retryable(&e) {
                        last_err = Some(e);
                        continue;
                    }
                    return Err(e);
                }
            }
        }

        Err(last_err.unwrap_or_else(|| MsigError::Rpc(RpcError::Connection("unknown".into()))))
    }

    fn do_request(&self, body: &[u8]) -> Result<String, MsigError> {
        let response = self
            .agent
            .post(&self.url)
            .header("Content-Type", "application/json")
            .send(body)
            .map_err(|e| ureq_to_msig_error(e, &self.safe_url()))?;

        let status = response.status().as_u16();
        if status == 429 {
            return Err(MsigError::Rpc(RpcError::RateLimited));
        }
        if status == 503 {
            return Err(MsigError::Rpc(RpcError::Connection(format!(
                "service unavailable (503) from {}",
                self.safe_url()
            ))));
        }

        response
            .into_body()
            .read_to_string()
            .map_err(|e| MsigError::Rpc(RpcError::Connection(format!("read body: {e}"))))
    }
}

impl RpcProvider for SolanaRpcClient {
    fn get_account_info(&self, pubkey: &str) -> Result<Option<AccountData>, MsigError> {
        self.get_account_info(pubkey)
    }
    fn get_multiple_accounts(
        &self,
        pubkeys: &[&str],
    ) -> Result<Vec<Option<AccountData>>, MsigError> {
        self.get_multiple_accounts(pubkeys)
    }
    fn get_latest_blockhash(&self) -> Result<(String, u64), MsigError> {
        self.get_latest_blockhash()
    }
    fn send_transaction(&self, base64_tx: &str) -> Result<String, MsigError> {
        self.send_transaction(base64_tx)
    }
    fn simulate_transaction(&self, base64_tx: &str) -> Result<types::SimulationResult, MsigError> {
        self.simulate_transaction(base64_tx)
    }
    fn get_signature_statuses(
        &self,
        sigs: &[&str],
    ) -> Result<Vec<Option<types::TxStatus>>, MsigError> {
        self.get_signature_statuses(sigs)
    }
}

fn is_retryable(err: &MsigError) -> bool {
    match err {
        MsigError::Rpc(rpc_err) => rpc_err.is_retryable(),
        _ => false,
    }
}

fn ureq_to_msig_error(err: ureq::Error, safe_url: &str) -> MsigError {
    match &err {
        ureq::Error::Timeout(_) => MsigError::Rpc(RpcError::Timeout),
        ureq::Error::StatusCode(429) => MsigError::Rpc(RpcError::RateLimited),
        ureq::Error::ConnectionFailed | ureq::Error::HostNotFound => MsigError::Rpc(
            RpcError::Connection(format!("failed to connect to {safe_url}")),
        ),
        ureq::Error::Io(io_err) => MsigError::Rpc(RpcError::Connection(format!(
            "I/O error with {safe_url}: {io_err}"
        ))),
        other => MsigError::Rpc(RpcError::Connection(format!(
            "request to {safe_url}: {other}"
        ))),
    }
}
