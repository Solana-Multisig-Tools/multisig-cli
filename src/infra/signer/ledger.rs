use crate::error::MsigError;
use crate::infra::signer::Signer;
use std::sync::Mutex;

const CLA: u8 = 0xE0;
const INS_GET_PUBKEY: u8 = 0x05;
const INS_SIGN_MESSAGE: u8 = 0x06;
const P1_CONFIRM: u8 = 0x01;
const P2_EXTEND: u8 = 0x01;
const P2_MORE: u8 = 0x02;
const CHUNK_SIZE: usize = 255;

fn check_status_word(sw: u16) -> Result<(), MsigError> {
    match sw {
        0x9000 => Ok(()),
        0x6985 => Err(MsigError::Signing(
            "Ledger: transaction rejected by user".to_string(),
        )),
        0x6700 => Err(MsigError::Signing(
            "Ledger: Solana app not open on device (no response to APDU)".to_string(),
        )),
        0x6982 => Err(MsigError::Signing(
            "Ledger: device may be locked or no APDU received".to_string(),
        )),
        0x6E00 => Err(MsigError::Signing(
            "Ledger: wrong app open on device (invalid CLA)".to_string(),
        )),
        other => Err(MsigError::Signing(format!(
            "Ledger: unexpected status word 0x{other:04X}"
        ))),
    }
}

fn serialize_derivation_path(account: u32, change: Option<u32>) -> Vec<u8> {
    let purpose = 44u32 | 0x8000_0000;
    let coin = 501u32 | 0x8000_0000;
    let acct = account | 0x8000_0000;
    let components = match change {
        Some(c) => vec![purpose, coin, acct, c | 0x8000_0000],
        None => vec![purpose, coin, acct],
    };
    let mut buf = Vec::with_capacity(1 + components.len() * 4);
    buf.push(components.len() as u8);
    for c in &components {
        buf.extend_from_slice(&c.to_be_bytes());
    }
    buf
}

pub struct LedgerSigner {
    state: Mutex<LedgerState>,
    account: u32,
    change: Option<u32>,
    cached_pubkey: Mutex<Option<solana_pubkey::Pubkey>>,
}

struct LedgerState {
    api: Option<ledger_transport_hid::hidapi::HidApi>,
    transport: Option<ledger_transport_hid::TransportNativeHID>,
}

impl LedgerSigner {
    pub fn new(account: u32, change: Option<u32>) -> Self {
        Self {
            state: Mutex::new(LedgerState {
                api: None,
                transport: None,
            }),
            account,
            change,
            cached_pubkey: Mutex::new(None),
        }
    }

    pub fn from_flag(value: &str) -> Result<Self, MsigError> {
        let (account, change) = parse_ledger_uri(value)?;
        Ok(Self::new(account, change))
    }

    fn with_transport<F, R>(&self, f: F) -> Result<R, MsigError>
    where
        F: FnOnce(&ledger_transport_hid::TransportNativeHID) -> Result<R, MsigError>,
    {
        let mut state = self
            .state
            .lock()
            .map_err(|_| MsigError::Signing("Ledger: internal lock poisoned".to_string()))?;

        if state.transport.is_none() {
            if state.api.is_none() {
                let api = ledger_transport_hid::hidapi::HidApi::new().map_err(|e| {
                    MsigError::Signing(format!(
                        "Ledger: failed to initialise USB HID: {e}. Is the device connected?"
                    ))
                })?;
                let devices: Vec<_> = api
                    .device_list()
                    .filter(|d| d.vendor_id() == 0x2c97)
                    .collect();
                if devices.is_empty() {
                    return Err(MsigError::Signing(
                        "Ledger: no device found. Connect your Ledger and unlock it.".to_string(),
                    ));
                }
                if devices.len() > 1 {
                    return Err(MsigError::Signing(format!(
                        "Ledger: {} devices found. Please connect only one Ledger at a time.",
                        devices.len()
                    )));
                }
                state.api = Some(api);
            }
            if let Some(ref api) = state.api {
                let transport =
                    ledger_transport_hid::TransportNativeHID::new(api).map_err(|e| {
                        MsigError::Signing(format!("Ledger: failed to open HID transport: {e}"))
                    })?;
                state.transport = Some(transport);
            }
        }

        match state.transport {
            Some(ref transport) => f(transport),
            None => Err(MsigError::Signing(
                "Ledger: transport not available".to_string(),
            )),
        }
    }

    fn exchange(
        &self,
        cla: u8,
        ins: u8,
        p1: u8,
        p2: u8,
        data: &[u8],
    ) -> Result<Vec<u8>, MsigError> {
        let data_owned = data.to_vec();
        self.with_transport(|transport| {
            let command = ledger_apdu::APDUCommand { cla, ins, p1, p2, data: data_owned.clone() };
            let answer = transport.exchange(&command)
                .map_err(|e| MsigError::Signing(format!("Ledger: USB communication error: {e}. Check that the device is still connected.")))?;
            check_status_word(answer.retcode())?;
            Ok(answer.data().to_vec())
        })
    }

    fn get_pubkey_from_device(&self) -> Result<solana_pubkey::Pubkey, MsigError> {
        let path_bytes = serialize_derivation_path(self.account, self.change);
        let response = self.exchange(CLA, INS_GET_PUBKEY, 0x00, 0x00, &path_bytes)?;
        if response.len() < 32 {
            return Err(MsigError::Signing(format!(
                "Ledger: expected 32-byte public key, got {} bytes",
                response.len()
            )));
        }
        let mut key_bytes = [0u8; 32];
        key_bytes.copy_from_slice(&response[..32]);
        Ok(solana_pubkey::Pubkey::from(key_bytes))
    }

    fn sign_message_on_device(&self, message: &[u8]) -> Result<Vec<u8>, MsigError> {
        let path_bytes = serialize_derivation_path(self.account, self.change);
        let mut payload = Vec::with_capacity(path_bytes.len() + message.len());
        payload.extend_from_slice(&path_bytes);
        payload.extend_from_slice(message);

        let chunks: Vec<&[u8]> = payload.chunks(CHUNK_SIZE).collect();
        let last_idx = chunks.len().saturating_sub(1);
        let mut response = Vec::new();

        for (i, chunk) in chunks.iter().enumerate() {
            let is_first = i == 0;
            let is_last = i == last_idx;
            let p1 = P1_CONFIRM;
            let p2 = if is_first {
                if is_last {
                    0x00
                } else {
                    P2_MORE
                }
            } else if is_last {
                P2_EXTEND
            } else {
                P2_EXTEND | P2_MORE
            };
            response = self.exchange(CLA, INS_SIGN_MESSAGE, p1, p2, chunk)?;
        }

        if response.len() < 64 {
            return Err(MsigError::Signing(format!(
                "Ledger: expected 64-byte signature, got {} bytes",
                response.len()
            )));
        }
        Ok(response[..64].to_vec())
    }
}

impl Signer for LedgerSigner {
    fn pubkey(&self) -> solana_pubkey::Pubkey {
        if let Ok(guard) = self.cached_pubkey.lock() {
            if let Some(pk) = *guard {
                return pk;
            }
        }
        match self.get_pubkey_from_device() {
            Ok(pk) => {
                if let Ok(mut guard) = self.cached_pubkey.lock() {
                    *guard = Some(pk);
                }
                pk
            }
            Err(e) => {
                eprintln!("error: {e}");
                eprintln!("  hint: Connect your Ledger, unlock it, and open the Solana app.");
                std::process::exit(40);
            }
        }
    }

    fn sign(&self, message: &[u8]) -> Result<Vec<u8>, MsigError> {
        let has_pubkey = self
            .cached_pubkey
            .lock()
            .map(|g| g.is_some())
            .unwrap_or(false);
        if !has_pubkey {
            let pk = self.get_pubkey_from_device()?;
            if let Ok(mut guard) = self.cached_pubkey.lock() {
                *guard = Some(pk);
            }
        }
        eprintln!("Please review and approve the transaction on your Ledger device...");
        self.sign_message_on_device(message)
    }

    fn requires_device_review(&self) -> bool {
        true
    }
}

pub fn parse_ledger_uri(input: &str) -> Result<(u32, Option<u32>), MsigError> {
    let input = input.trim();
    if input.is_empty() {
        return Ok((0, None));
    }
    if let Ok(n) = input.parse::<u32>() {
        return Ok((n, None));
    }

    if let Some(rest) = input.strip_prefix("usb://ledger") {
        let mut account = 0u32;
        let mut change: Option<u32> = None;
        if let Some(query) = rest.strip_prefix('?') {
            for pair in query.split('&') {
                let mut kv = pair.splitn(2, '=');
                let key = kv.next().unwrap_or("");
                let val = kv.next().unwrap_or("");
                match key {
                    "key" => {
                        account = val.parse::<u32>().map_err(|_| {
                            MsigError::Config(format!(
                                "invalid Ledger key index: '{val}'. Expected a number."
                            ))
                        })?;
                    }
                    "change" => {
                        change = Some(val.parse::<u32>().map_err(|_| {
                            MsigError::Config(format!(
                                "invalid Ledger change index: '{val}'. Expected a number."
                            ))
                        })?);
                    }
                    _ => {}
                }
            }
        }
        return Ok((account, change));
    }

    Err(MsigError::Config(format!(
        "invalid --ledger value: '{input}'. Expected a number (e.g. '0') or 'usb://ledger?key=N'."
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_ledger_uri_empty() {
        let (acct, change) = parse_ledger_uri("").unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(acct, 0);
        assert!(change.is_none());
    }

    #[test]
    fn parse_ledger_uri_number() {
        let (acct, change) = parse_ledger_uri("3").unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(acct, 3);
        assert!(change.is_none());
    }

    #[test]
    fn parse_ledger_uri_usb_with_key_and_change() {
        let (acct, change) =
            parse_ledger_uri("usb://ledger?key=2&change=1").unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(acct, 2);
        assert_eq!(change, Some(1));
    }

    #[test]
    fn parse_ledger_uri_invalid() {
        assert!(parse_ledger_uri("invalid://thing").is_err());
    }

    #[test]
    fn derivation_path_3_components() {
        let path = serialize_derivation_path(0, None);
        assert_eq!(path[0], 3);
        assert_eq!(path.len(), 1 + 3 * 4);
    }

    #[test]
    fn derivation_path_4_components() {
        let path = serialize_derivation_path(2, Some(1));
        assert_eq!(path[0], 4);
        assert_eq!(path.len(), 1 + 4 * 4);
    }
}
