use serde::Serialize;
use std::fmt;

/// Output mode for error formatting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputMode {
    Text,
    Json,
}

/// Account parsing errors with full structural context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    Eof {
        offset: usize,
        need: usize,
        have: usize,
    },
    VecTooLong {
        len: u32,
        cap: u32,
    },
    InvalidTag {
        tag: u8,
        type_name: &'static str,
    },
    InvalidDiscriminator {
        expected: [u8; 8],
        got: [u8; 8],
    },
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Eof { offset, need, have } => {
                write!(
                    f,
                    "unexpected EOF at offset {offset}: need {need} bytes, have {have}"
                )
            }
            Self::VecTooLong { len, cap } => {
                write!(f, "vec length {len} exceeds cap {cap}")
            }
            Self::InvalidTag { tag, type_name } => {
                write!(f, "invalid tag {tag} for type {type_name}")
            }
            Self::InvalidDiscriminator { expected, got } => {
                write!(
                    f,
                    "invalid discriminator: expected {expected:02x?}, got {got:02x?}"
                )
            }
        }
    }
}

impl std::error::Error for ParseError {}

/// RPC-specific errors with retryability semantics.
#[derive(Debug)]
pub enum RpcError {
    Connection(String),
    RateLimited,
    Timeout,
    JsonRpc { code: i64, message: String },
}

impl RpcError {
    pub fn exit_code(&self) -> u8 {
        match self {
            Self::Connection(_) => 20,
            Self::RateLimited => 21,
            Self::Timeout => 22,
            Self::JsonRpc { .. } => 20,
        }
    }

    pub fn is_retryable(&self) -> bool {
        matches!(self, Self::RateLimited | Self::Timeout)
    }
}

impl fmt::Display for RpcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Connection(msg) => write!(f, "RPC connection error: {msg}"),
            Self::RateLimited => write!(f, "RPC rate limited"),
            Self::Timeout => write!(f, "RPC request timed out"),
            Self::JsonRpc { code, message } => write!(f, "RPC JSON-RPC error {code}: {message}"),
        }
    }
}

/// Top-level error type for msig.
#[derive(Debug)]
pub enum MsigError {
    Usage(String),
    Config(String),
    Rpc(RpcError),
    Parse(ParseError),
    Transaction(String),
    SimulationFailed(String),
    InsufficientSignatures {
        have: u16,
        need: u16,
    },
    Signing(String),
    MultisigNotFound(String),
    ProposalNotFound(u64),
    PermissionDenied(String),
    #[allow(dead_code)]
    TimeLockActive {
        remaining_secs: u64,
    },
    Io(std::io::Error),
    Cancelled,
    HelpDisplayed,
}

impl MsigError {
    pub fn exit_code(&self) -> u8 {
        match self {
            Self::Io(_) => 1,
            Self::Usage(_) => 2,
            Self::Config(_) => 10,
            Self::Rpc(e) => e.exit_code(),
            Self::Parse(_) => 30,
            Self::Transaction(_) => 30,
            Self::SimulationFailed(_) => 31,
            Self::InsufficientSignatures { .. } => 32,
            Self::Signing(_) => 40,
            Self::MultisigNotFound(_) => 50,
            Self::ProposalNotFound(_) => 51,
            Self::PermissionDenied(_) => 52,
            Self::TimeLockActive { .. } => 53,
            Self::Cancelled => 60,
            Self::HelpDisplayed => 0,
        }
    }

    pub fn fix_suggestion(&self) -> Option<String> {
        match self {
            Self::Usage(_) => Some("Run `msig --help` for usage information.".into()),
            Self::Config(_) => Some(
                "Check your config file at ~/.config/msig/config.toml or set via CLI flags.".into(),
            ),
            Self::Rpc(RpcError::Connection(_)) => {
                Some("Check your RPC URL and network connectivity.".into())
            }
            Self::Rpc(RpcError::RateLimited) => {
                Some("Wait a moment and retry, or use a different RPC endpoint.".into())
            }
            Self::Rpc(RpcError::Timeout) => {
                Some("The RPC endpoint is slow. Retry or switch to a faster endpoint.".into())
            }
            Self::Rpc(RpcError::JsonRpc { .. }) => {
                Some("Check the RPC error code and message for details.".into())
            }
            Self::Parse(e) => Some(format!("Account data could not be parsed: {e}")),
            Self::Transaction(_) => Some("Review the transaction parameters and try again.".into()),
            Self::SimulationFailed(_) => {
                Some("The transaction simulation failed. Check logs for details.".into())
            }
            Self::InsufficientSignatures { have, need } => Some(format!(
                "Need {need} signatures but only have {have}. Collect more approvals."
            )),
            Self::Signing(_) => {
                Some("Check that your signer is available and has correct permissions.".into())
            }
            Self::MultisigNotFound(_) => {
                Some("Verify the multisig address is correct and exists on-chain.".into())
            }
            Self::ProposalNotFound(idx) => Some(format!(
                "Proposal #{idx} not found. Check the proposal index."
            )),
            Self::PermissionDenied(_) => Some(
                "Your key is not a member of this multisig or lacks the required permission."
                    .into(),
            ),
            Self::TimeLockActive { remaining_secs } => Some(format!(
                "Time lock active. Wait {remaining_secs} seconds before executing."
            )),
            Self::Io(_) => Some("Check file permissions and disk space.".into()),
            Self::Cancelled => None,
            Self::HelpDisplayed => None,
        }
    }

    fn error_kind(&self) -> &'static str {
        match self {
            Self::Usage(_) => "usage",
            Self::Config(_) => "config",
            Self::Rpc(_) => "rpc",
            Self::Parse(_) => "parse",
            Self::Transaction(_) => "transaction",
            Self::SimulationFailed(_) => "simulation",
            Self::InsufficientSignatures { .. } => "insufficient_signatures",
            Self::Signing(_) => "signing",
            Self::MultisigNotFound(_) => "multisig_not_found",
            Self::ProposalNotFound(_) => "proposal_not_found",
            Self::PermissionDenied(_) => "permission_denied",
            Self::TimeLockActive { .. } => "time_lock_active",
            Self::Io(_) => "io",
            Self::Cancelled => "cancelled",
            Self::HelpDisplayed => "help",
        }
    }

    pub fn to_error_report(&self) -> ErrorReport {
        let mut cause_chain = Vec::new();
        if let Self::Io(e) = self {
            if let Some(inner) = e.get_ref() {
                cause_chain.push(inner.to_string());
            }
        }
        if let Self::Rpc(rpc_err) = self {
            cause_chain.push(rpc_err.to_string());
        }
        if let Self::Parse(parse_err) = self {
            cause_chain.push(parse_err.to_string());
        }

        ErrorReport {
            error: self.to_string(),
            error_code: self.exit_code(),
            error_kind: self.error_kind().to_string(),
            cause_chain,
            fix_suggestion: self.fix_suggestion(),
        }
    }
}

impl fmt::Display for MsigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Usage(msg) => write!(f, "Usage error: {msg}"),
            Self::Config(msg) => write!(f, "Config error: {msg}"),
            Self::Rpc(e) => write!(f, "{e}"),
            Self::Parse(e) => write!(f, "Account parse error: {e}"),
            Self::Transaction(msg) => write!(f, "Transaction error: {msg}"),
            Self::SimulationFailed(msg) => write!(f, "Simulation failed: {msg}"),
            Self::InsufficientSignatures { have, need } => {
                write!(f, "Insufficient signatures: have {have}, need {need}")
            }
            Self::Signing(msg) => write!(f, "Signing error: {msg}"),
            Self::MultisigNotFound(addr) => write!(f, "Multisig not found: {addr}"),
            Self::ProposalNotFound(idx) => write!(f, "Proposal not found: #{idx}"),
            Self::PermissionDenied(msg) => write!(f, "Permission denied: {msg}"),
            Self::TimeLockActive { remaining_secs } => {
                write!(f, "Time lock active: {remaining_secs}s remaining")
            }
            Self::Io(e) => write!(f, "I/O error: {e}"),
            Self::Cancelled => write!(f, "Operation cancelled"),
            Self::HelpDisplayed => write!(f, "Help displayed"),
        }
    }
}

impl From<std::io::Error> for MsigError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<ParseError> for MsigError {
    fn from(e: ParseError) -> Self {
        Self::Parse(e)
    }
}

/// Structured error report for JSON output.
#[derive(Debug, Serialize)]
pub struct ErrorReport {
    pub error: String,
    pub error_code: u8,
    pub error_kind: String,
    pub cause_chain: Vec<String>,
    pub fix_suggestion: Option<String>,
}

/// Install a panic handler that produces clean output instead of a stack trace.
pub fn install_panic_handler(output_mode: OutputMode) {
    std::panic::set_hook(Box::new(move |info| {
        #[cfg(feature = "tui")]
        {
            let _ = crossterm::terminal::disable_raw_mode();
            let _ =
                crossterm::execute!(std::io::stderr(), crossterm::terminal::LeaveAlternateScreen);
        }

        let payload = if let Some(s) = info.payload().downcast_ref::<&str>() {
            (*s).to_string()
        } else if let Some(s) = info.payload().downcast_ref::<String>() {
            s.clone()
        } else {
            "unknown panic".to_string()
        };

        let location = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_default();

        match output_mode {
            OutputMode::Json => {
                let report = ErrorReport {
                    error: format!("Internal error (panic): {payload}"),
                    error_code: 100,
                    error_kind: "panic".to_string(),
                    cause_chain: if location.is_empty() { vec![] } else { vec![format!("at {location}")] },
                    fix_suggestion: Some("This is a bug. Please report it at https://github.com/squads-protocol/msig/issues".to_string()),
                };
                if let Ok(json) = serde_json::to_string(&report) {
                    eprintln!("{json}");
                }
            }
            OutputMode::Text => {
                eprintln!("msig: internal error (panic): {payload}");
                if !location.is_empty() {
                    eprintln!("  at {location}");
                }
                eprintln!();
                eprintln!("This is a bug. Please report it at:\n  https://github.com/squads-protocol/msig/issues");
            }
        }

        std::process::exit(100);
    }));
}
