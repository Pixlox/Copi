//! 4-Digit Pairing Protocol
//!
//! Implements secure device pairing using a 4-digit numeric code.
//! The code is displayed on one device and entered on the other.
//!
//! Protocol:
//! 1. Device A generates a 4-digit code and displays it
//! 2. Device B enters the code
//! 3. Both devices derive a shared secret from the code + device IDs
//! 4. This shared secret is used to verify the initial encrypted handshake
//! 5. On success, devices exchange and store each other's public keys

use super::device::DeviceInfo;
use rand::Rng;
use serde::{Deserialize, Serialize};

/// A 4-digit pairing code
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairingCode {
    digits: [u8; 4],
}

impl PairingCode {
    /// Generate a new random pairing code
    pub fn generate() -> Self {
        let mut rng = rand::thread_rng();
        let digits = [
            rng.gen_range(0..10),
            rng.gen_range(0..10),
            rng.gen_range(0..10),
            rng.gen_range(0..10),
        ];
        Self { digits }
    }

    /// Get the raw 4-digit string
    pub fn to_string(&self) -> String {
        format!(
            "{}{}{}{}",
            self.digits[0], self.digits[1], self.digits[2], self.digits[3]
        )
    }
}

/// Pairing request message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairingRequest {
    pub device_info: DeviceInfo,
    pub verification_hash: Vec<u8>,
}

impl PairingRequest {}

/// Pairing response message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairingResponse {
    pub success: bool,
    pub device_info: Option<DeviceInfo>,
    pub error: Option<String>,
}

impl PairingResponse {}

/// Pairing message wrapper
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PairingMessage {
    Request(PairingRequest),
    Response(PairingResponse),
}
