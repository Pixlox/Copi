//! mDNS Service Discovery
//!
//! Uses mDNS to discover other Copi instances on the local network.
//! Service type: `_copi._tcp.local.`

use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, RwLock};
use std::thread;
use tokio::sync::broadcast;

use super::device::{DeviceInfo, Platform};
use super::{SyncError, SyncResult};

/// mDNS service type for Copi
const SERVICE_TYPE: &str = "_copi._tcp.local.";

/// Default port for sync connections
pub const DEFAULT_SYNC_PORT: u16 = 47524; // "COPI" on phone keypad (2674) + offset

/// Discovered device on the network
#[derive(Debug, Clone)]
pub struct DiscoveredDevice {
    pub device_id: String,
    pub device_name: String,
    pub platform: Platform,
    pub addresses: Vec<IpAddr>,
    pub port: u16,
}

/// Discovery service for finding peers on the network
pub struct DiscoveryService {
    daemon: ServiceDaemon,
    our_device_id: String,
    discovered: Arc<RwLock<HashMap<String, DiscoveredDevice>>>,
    event_tx: broadcast::Sender<DiscoveryEvent>,
    service_name: String,
}

/// Events from the discovery service
#[derive(Debug, Clone)]
pub enum DiscoveryEvent {
    /// New device discovered
    DeviceFound(DiscoveredDevice),
    /// Device went offline
    DeviceLost(String),
    /// Device info updated (e.g., IP changed)
    DeviceUpdated(DiscoveredDevice),
}

impl DiscoveryService {
    /// Create a new discovery service
    pub fn new(device_info: &DeviceInfo) -> SyncResult<Self> {
        let daemon = ServiceDaemon::new()
            .map_err(|e| SyncError::IoError(std::io::Error::new(std::io::ErrorKind::Other, e)))?;

        let (event_tx, _) = broadcast::channel(100);
        let service_name = format!("copi-{}", &device_info.device_id[..8]);

        Ok(Self {
            daemon,
            our_device_id: device_info.device_id.clone(),
            discovered: Arc::new(RwLock::new(HashMap::new())),
            event_tx,
            service_name,
        })
    }

    /// Start advertising this device and browsing for others
    pub fn start(&self, device_info: &DeviceInfo, port: u16) -> SyncResult<()> {
        // Register our service
        self.register_service(device_info, port)?;

        // Start browsing for other services
        self.browse()?;

        Ok(())
    }

    /// Register this device as an mDNS service
    fn register_service(&self, device_info: &DeviceInfo, port: u16) -> SyncResult<()> {
        let platform_str = match device_info.platform {
            Platform::MacOS => "macos",
            Platform::Windows => "windows",
            Platform::Linux => "linux",
            Platform::Unknown => "unknown",
        };

        // Encode public key as base64 for TXT record
        let public_key_b64 = base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            &device_info.public_key,
        );

        let properties = [
            ("id", device_info.device_id.as_str()),
            ("name", device_info.device_name.as_str()),
            ("platform", platform_str),
            ("pubkey", &public_key_b64),
            ("version", "1"),
        ];

        let service = ServiceInfo::new(
            SERVICE_TYPE,
            &self.service_name,
            &format!("{}.local.", self.service_name),
            (), // Empty IPs, will be auto-filled by enable_addr_auto()
            port,
            &properties[..],
        )
        .map_err(|e| SyncError::IoError(std::io::Error::new(std::io::ErrorKind::Other, e)))?
        .enable_addr_auto(); // Auto-fill host IPs from network interfaces

        self.daemon
            .register(service)
            .map_err(|e| SyncError::IoError(std::io::Error::new(std::io::ErrorKind::Other, e)))?;

        eprintln!(
            "[Sync] Registered mDNS service: {} on port {}",
            self.service_name, port
        );

        Ok(())
    }

    /// Browse for other Copi services on the network
    fn browse(&self) -> SyncResult<()> {
        let receiver = self
            .daemon
            .browse(SERVICE_TYPE)
            .map_err(|e| SyncError::IoError(std::io::Error::new(std::io::ErrorKind::Other, e)))?;

        let discovered = self.discovered.clone();
        let event_tx = self.event_tx.clone();
        let our_device_id = self.our_device_id.clone();

        // Use std::thread for the blocking mDNS receiver (crossbeam channel)
        thread::Builder::new()
            .name("mdns-browse".to_string())
            .spawn(move || loop {
                match receiver.recv() {
                    Ok(event) => {
                        Self::handle_discovery_event(event, &discovered, &event_tx, &our_device_id);
                    }
                    Err(e) => {
                        eprintln!("[Sync] mDNS browse error: {:?}", e);
                        break;
                    }
                }
            })
            .map_err(|e| SyncError::IoError(e))?;

        Ok(())
    }

    /// Handle an mDNS discovery event (runs on blocking thread)
    fn handle_discovery_event(
        event: ServiceEvent,
        discovered: &Arc<RwLock<HashMap<String, DiscoveredDevice>>>,
        event_tx: &broadcast::Sender<DiscoveryEvent>,
        our_device_id: &str,
    ) {
        match event {
            ServiceEvent::ServiceResolved(info) => {
                // Parse device info from TXT records
                let device_id = info.get_property_val_str("id");
                let device_name = info.get_property_val_str("name");
                let platform_str = info.get_property_val_str("platform");

                // Skip if missing required fields or if it's our own device
                let Some(device_id) = device_id else { return };
                if device_id == our_device_id {
                    return;
                }

                let device_name = device_name.unwrap_or("Unknown");
                let platform = match platform_str.unwrap_or("unknown") {
                    "macos" => Platform::MacOS,
                    "windows" => Platform::Windows,
                    "linux" => Platform::Linux,
                    _ => Platform::Unknown,
                };

                let addresses: Vec<IpAddr> = info.get_addresses().iter().copied().collect();
                let port = info.get_port();

                let device = DiscoveredDevice {
                    device_id: device_id.to_string(),
                    device_name: device_name.to_string(),
                    platform,
                    addresses,
                    port,
                };

                let is_new = {
                    let mut map = discovered.write().unwrap();
                    let is_new = !map.contains_key(&device.device_id);
                    map.insert(device.device_id.clone(), device.clone());
                    is_new
                };

                let _ = if is_new {
                    eprintln!(
                        "[Sync] Discovered device: {} ({}) at {:?}:{}",
                        device.device_name, device.platform, device.addresses, device.port
                    );
                    event_tx.send(DiscoveryEvent::DeviceFound(device))
                } else {
                    event_tx.send(DiscoveryEvent::DeviceUpdated(device))
                };
            }

            ServiceEvent::ServiceRemoved(_service_type, fullname) => {
                // Extract device ID from service name
                // Service name format: "copi-{device_id_prefix}"
                let device_id = {
                    let map = discovered.read().unwrap();
                    map.iter()
                        .find(|(_, d)| fullname.contains(&d.device_id[..8]))
                        .map(|(id, _)| id.clone())
                };

                if let Some(device_id) = device_id {
                    {
                        let mut map = discovered.write().unwrap();
                        map.remove(&device_id);
                    }
                    eprintln!("[Sync] Device lost: {}", device_id);
                    let _ = event_tx.send(DiscoveryEvent::DeviceLost(device_id));
                }
            }

            _ => {}
        }
    }

    /// Subscribe to discovery events
    pub fn subscribe(&self) -> broadcast::Receiver<DiscoveryEvent> {
        self.event_tx.subscribe()
    }

    /// Get a specific discovered device by ID (blocking)
    pub fn get_device(&self, device_id: &str) -> Option<DiscoveredDevice> {
        self.discovered.read().ok()?.get(device_id).cloned()
    }

    /// Stop the discovery service
    pub fn stop(&self) -> SyncResult<()> {
        // Unregister our service
        let _ = self.daemon.unregister(&self.service_name);
        // Stop browsing
        let _ = self.daemon.stop_browse(SERVICE_TYPE);

        eprintln!("[Sync] Discovery service stopped");
        Ok(())
    }
}

impl Drop for DiscoveryService {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}
