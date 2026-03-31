//! Device Identity Management
//!
//! Handles this device's identity including:
//! - Generating and storing X25519 keypair
//! - Device name and platform detection
//! - Persisting identity to database

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use x25519_dalek::{PublicKey, StaticSecret};

use super::{SyncError, SyncResult};

/// Platform identifier
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    MacOS,
    Windows,
    Linux,
    Unknown,
}

impl Platform {
    /// Detect the current platform
    pub fn current() -> Self {
        #[cfg(target_os = "macos")]
        return Platform::MacOS;

        #[cfg(target_os = "windows")]
        return Platform::Windows;

        #[cfg(target_os = "linux")]
        return Platform::Linux;

        #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
        return Platform::Unknown;
    }

    /// Get display name for the platform
    pub fn display_name(&self) -> &'static str {
        match self {
            Platform::MacOS => "macOS",
            Platform::Windows => "Windows",
            Platform::Linux => "Linux",
            Platform::Unknown => "Unknown",
        }
    }
}

impl std::fmt::Display for Platform {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.display_name())
    }
}

/// Basic device information (safe to share)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceInfo {
    pub device_id: String,
    pub device_name: String,
    pub platform: Platform,
    pub public_key: Vec<u8>,
}

/// Full device identity (includes private key, never shared)
pub struct DeviceIdentity {
    pub device_id: String,
    pub device_name: String,
    pub platform: Platform,
    pub private_key: StaticSecret,
    pub public_key: PublicKey,
}

impl std::fmt::Debug for DeviceIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeviceIdentity")
            .field("device_id", &self.device_id)
            .field("device_name", &self.device_name)
            .field("platform", &self.platform)
            .field("private_key", &"[REDACTED]")
            .field("public_key", &hex::encode(self.public_key.as_bytes()))
            .finish()
    }
}

impl Clone for DeviceIdentity {
    fn clone(&self) -> Self {
        // StaticSecret doesn't implement Clone, so we need to reconstruct it
        let private_key_bytes = self.private_key.as_bytes();
        let private_key = StaticSecret::from(*private_key_bytes);
        let public_key = PublicKey::from(&private_key);

        Self {
            device_id: self.device_id.clone(),
            device_name: self.device_name.clone(),
            platform: self.platform,
            private_key,
            public_key,
        }
    }
}

impl DeviceIdentity {
    /// Generate a new device identity
    pub fn generate(device_name: String) -> Self {
        let device_id = uuid::Uuid::new_v4().to_string();
        let private_key = StaticSecret::random_from_rng(rand::thread_rng());
        let public_key = PublicKey::from(&private_key);

        Self {
            device_id,
            device_name,
            platform: Platform::current(),
            private_key,
            public_key,
        }
    }

    /// Get shareable device info (excludes private key)
    pub fn to_info(&self) -> DeviceInfo {
        DeviceInfo {
            device_id: self.device_id.clone(),
            device_name: self.device_name.clone(),
            platform: self.platform,
            public_key: self.public_key.as_bytes().to_vec(),
        }
    }

    /// Load existing identity from database or generate new one
    pub fn load_or_generate(conn: &Connection, default_name: Option<String>) -> SyncResult<Self> {
        // Try to load existing identity
        if let Some(identity) = Self::load_from_db(conn)? {
            return Ok(identity);
        }

        // Generate new identity
        let device_name = default_name.unwrap_or_else(|| get_device_name());
        let identity = Self::generate(device_name);

        // Save to database
        identity.save_to_db(conn)?;

        Ok(identity)
    }

    /// Load identity from database
    fn load_from_db(conn: &Connection) -> SyncResult<Option<Self>> {
        let result: Option<(String, String, String, Vec<u8>, Vec<u8>)> = conn
            .query_row(
                "SELECT device_id, device_name, platform, private_key, public_key FROM device_info LIMIT 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
            )
            .optional()?;

        match result {
            Some((device_id, device_name, platform_str, private_key_bytes, _public_key_bytes)) => {
                // Reconstruct private key
                let private_key_array: [u8; 32] = private_key_bytes
                    .try_into()
                    .map_err(|_| SyncError::EncryptionError("Invalid private key length".into()))?;
                let private_key = StaticSecret::from(private_key_array);
                let public_key = PublicKey::from(&private_key);

                // Parse platform
                let platform = match platform_str.as_str() {
                    "macos" => Platform::MacOS,
                    "windows" => Platform::Windows,
                    "linux" => Platform::Linux,
                    _ => Platform::Unknown,
                };

                Ok(Some(Self {
                    device_id,
                    device_name,
                    platform,
                    private_key,
                    public_key,
                }))
            }
            None => Ok(None),
        }
    }

    /// Save identity to database
    fn save_to_db(&self, conn: &Connection) -> SyncResult<()> {
        let platform_str = match self.platform {
            Platform::MacOS => "macos",
            Platform::Windows => "windows",
            Platform::Linux => "linux",
            Platform::Unknown => "unknown",
        };

        let private_key_bytes = self.private_key.as_bytes().to_vec();
        let public_key_bytes = self.public_key.as_bytes().to_vec();
        let created_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        conn.execute(
            "INSERT OR REPLACE INTO device_info (device_id, device_name, platform, private_key, public_key, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![
                self.device_id,
                self.device_name,
                platform_str,
                private_key_bytes,
                public_key_bytes,
                created_at,
            ],
        )?;

        Ok(())
    }
}

/// Get the device name from the system
fn get_device_name() -> String {
    // Try to get hostname
    if let Ok(hostname) = hostname::get() {
        if let Some(name) = hostname.to_str() {
            // Remove .local suffix on macOS
            let name = name.strip_suffix(".local").unwrap_or(name);
            return name.to_string();
        }
    }

    // Fallback to platform-specific default
    format!("{} Device", Platform::current().display_name())
}

/// Information about a paired device
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairedDevice {
    pub device_id: String,
    pub device_name: String,
    pub platform: Platform,
    pub public_key: Vec<u8>,
    pub paired_at: i64,
    pub last_seen: Option<i64>,
    pub last_sync_version: i64,
}

impl PairedDevice {
    /// Load all paired devices from database
    pub fn load_all(conn: &Connection) -> SyncResult<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT device_id, device_name, platform, public_key, paired_at, last_seen, last_sync_version
             FROM paired_devices ORDER BY last_seen DESC NULLS LAST",
        )?;

        let devices = stmt
            .query_map([], |row| {
                let platform_str: String = row.get(2)?;
                let platform = match platform_str.as_str() {
                    "macos" => Platform::MacOS,
                    "windows" => Platform::Windows,
                    "linux" => Platform::Linux,
                    _ => Platform::Unknown,
                };

                Ok(PairedDevice {
                    device_id: row.get(0)?,
                    device_name: row.get(1)?,
                    platform,
                    public_key: row.get(3)?,
                    paired_at: row.get(4)?,
                    last_seen: row.get(5)?,
                    last_sync_version: row.get(6)?,
                })
            })?
            .filter_map(|r| r.ok())
            .collect();

        Ok(devices)
    }

    /// Save a newly paired device to database
    pub fn save(&self, conn: &Connection) -> SyncResult<()> {
        let platform_str = match self.platform {
            Platform::MacOS => "macos",
            Platform::Windows => "windows",
            Platform::Linux => "linux",
            Platform::Unknown => "unknown",
        };

        conn.execute(
            "INSERT OR REPLACE INTO paired_devices 
             (device_id, device_name, platform, public_key, paired_at, last_seen, last_sync_version)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![
                self.device_id,
                self.device_name,
                platform_str,
                self.public_key,
                self.paired_at,
                self.last_seen,
                self.last_sync_version,
            ],
        )?;

        Ok(())
    }

    /// Remove a paired device
    pub fn remove(conn: &Connection, device_id: &str) -> SyncResult<()> {
        conn.execute(
            "DELETE FROM paired_devices WHERE device_id = ?1",
            rusqlite::params![device_id],
        )?;

        Ok(())
    }

    /// Check if a device is paired
    pub fn is_paired(conn: &Connection, device_id: &str) -> SyncResult<bool> {
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM paired_devices WHERE device_id = ?1",
            rusqlite::params![device_id],
            |row| row.get(0),
        )?;

        Ok(count > 0)
    }
}

// Need to import OptionalExtension for .optional()
use rusqlite::OptionalExtension;
