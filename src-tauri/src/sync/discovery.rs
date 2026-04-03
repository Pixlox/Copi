//! mDNS Service Discovery for Copi sync.
//!
//! Advertises this device and browses for peers on the local network.

use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::thread;
use std::time::Duration;
use tokio::sync::broadcast;

const SERVICE_TYPE: &str = "_copi._tcp.local.";
const BROWSE_RETRY_DELAY: Duration = Duration::from_secs(2);

#[derive(Debug, Clone)]
pub struct DiscoveredDevice {
    pub device_id: String,
    pub device_name: String,
    pub platform: String,
    pub public_key: Vec<u8>,
    pub addresses: Vec<IpAddr>,
    pub port: u16,
}

#[derive(Debug, Clone)]
pub enum DiscoveryEvent {
    DeviceFound(DiscoveredDevice),
    DeviceUpdated(DiscoveredDevice),
    DeviceLost(String),
}

pub struct DiscoveryService {
    daemon: ServiceDaemon,
    our_device_id: String,
    discovered: Arc<RwLock<HashMap<String, DiscoveredDevice>>>,
    event_tx: broadcast::Sender<DiscoveryEvent>,
    service_name: String,
    stopped: Arc<AtomicBool>,
}

impl DiscoveryService {
    pub fn new(
        device_id: &str,
        _device_name: &str,
        _platform: &str,
        _public_key: &[u8],
    ) -> Result<Self, String> {
        let daemon = ServiceDaemon::new().map_err(|e| format!("mDNS daemon failed: {}", e))?;
        let (event_tx, _) = broadcast::channel(64);
        let service_name = format!("copi-{}", &device_id[..8.min(device_id.len())]);

        Ok(Self {
            daemon,
            our_device_id: device_id.to_string(),
            discovered: Arc::new(RwLock::new(HashMap::new())),
            event_tx,
            service_name,
            stopped: Arc::new(AtomicBool::new(false)),
        })
    }

    pub fn start(
        &self,
        port: u16,
        device_name: &str,
        platform: &str,
        public_key: &[u8],
    ) -> Result<(), String> {
        self.register_service(port, device_name, platform, public_key)?;
        self.browse()?;
        Ok(())
    }

    fn register_service(
        &self,
        port: u16,
        device_name: &str,
        platform: &str,
        public_key: &[u8],
    ) -> Result<(), String> {
        let public_key_b64 =
            base64::Engine::encode(&base64::engine::general_purpose::STANDARD, public_key);

        let properties = [
            ("id", self.our_device_id.as_str()),
            ("name", device_name),
            ("platform", platform),
            ("pubkey", &public_key_b64),
            ("version", "2"),
        ];

        let service = ServiceInfo::new(
            SERVICE_TYPE,
            &self.service_name,
            &format!("{}.local.", self.service_name),
            (),
            port,
            &properties[..],
        )
        .map_err(|e| format!("ServiceInfo build failed: {}", e))?
        .enable_addr_auto();

        self.daemon
            .register(service)
            .map_err(|e| format!("mDNS register failed: {}", e))?;

        eprintln!(
            "[Sync] Registered mDNS service: {} on port {}",
            self.service_name, port
        );
        Ok(())
    }

    fn browse(&self) -> Result<(), String> {
        let daemon = self.daemon.clone();
        let discovered = self.discovered.clone();
        let event_tx = self.event_tx.clone();
        let our_device_id = self.our_device_id.clone();
        let stopped = self.stopped.clone();

        thread::Builder::new()
            .name("mdns-browse".into())
            .spawn(move || loop {
                if stopped.load(Ordering::SeqCst) {
                    break;
                }
                let receiver = match daemon.browse(SERVICE_TYPE) {
                    Ok(r) => r,
                    Err(_) => {
                        thread::sleep(BROWSE_RETRY_DELAY);
                        continue;
                    }
                };

                loop {
                    match receiver.recv() {
                        Ok(event) => {
                            Self::handle_event(&event, &discovered, &event_tx, &our_device_id);
                        }
                        Err(_) => break,
                    }
                }

                if stopped.load(Ordering::SeqCst) {
                    break;
                }
                thread::sleep(BROWSE_RETRY_DELAY);
            })
            .map_err(|e| format!("Failed to spawn browse thread: {}", e))?;

        Ok(())
    }

    fn handle_event(
        event: &ServiceEvent,
        discovered: &Arc<RwLock<HashMap<String, DiscoveredDevice>>>,
        event_tx: &broadcast::Sender<DiscoveryEvent>,
        our_device_id: &str,
    ) {
        match event {
            ServiceEvent::ServiceResolved(info) => {
                let device_id = match info.get_property_val_str("id") {
                    Some(id) => id,
                    None => return,
                };
                if device_id == our_device_id {
                    return;
                }

                let device_name = info
                    .get_property_val_str("name")
                    .unwrap_or("Unknown")
                    .to_string();
                let platform = info
                    .get_property_val_str("platform")
                    .unwrap_or("unknown")
                    .to_string();
                let public_key = info
                    .get_property_val_str("pubkey")
                    .and_then(|s| {
                        base64::Engine::decode(&base64::engine::general_purpose::STANDARD, s).ok()
                    })
                    .unwrap_or_default();

                let addresses: Vec<IpAddr> = info
                    .get_addresses()
                    .iter()
                    .copied()
                    .filter(|ip| match ip {
                        IpAddr::V4(v4) => !v4.is_loopback() && !v4.is_unspecified(),
                        IpAddr::V6(v6) => !v6.is_loopback() && !v6.is_unspecified(),
                    })
                    .collect();

                if addresses.is_empty() {
                    return;
                }

                let port = info.get_port();
                let device = DiscoveredDevice {
                    device_id: device_id.to_string(),
                    device_name,
                    platform,
                    public_key,
                    addresses,
                    port,
                };

                let is_new = {
                    let mut map = discovered.write().unwrap();
                    let is_new = !map.contains_key(&device.device_id);
                    map.insert(device.device_id.clone(), device.clone());
                    is_new
                };

                if is_new {
                    eprintln!(
                        "[Sync] Discovered: {} ({}) at {:?}:{}",
                        device.device_name, device.platform, device.addresses, device.port
                    );
                    let _ = event_tx.send(DiscoveryEvent::DeviceFound(device));
                } else {
                    let _ = event_tx.send(DiscoveryEvent::DeviceUpdated(device));
                }
            }
            ServiceEvent::ServiceRemoved(_, fullname) => {
                let device_id = {
                    let map = discovered.read().unwrap();
                    map.iter()
                        .find(|(_, d)| fullname.contains(&d.device_id[..8.min(d.device_id.len())]))
                        .map(|(id, _)| id.clone())
                };
                if let Some(id) = device_id {
                    discovered.write().unwrap().remove(&id);
                    let _ = event_tx.send(DiscoveryEvent::DeviceLost(id));
                }
            }
            _ => {}
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<DiscoveryEvent> {
        self.event_tx.subscribe()
    }

    pub fn get_device(&self, device_id: &str) -> Option<DiscoveredDevice> {
        self.discovered.read().ok()?.get(device_id).cloned()
    }

    pub fn stop(&self) {
        self.stopped.store(true, Ordering::SeqCst);
        let _ = self.daemon.unregister(&self.service_name);
        let _ = self.daemon.stop_browse(SERVICE_TYPE);
    }
}

impl Drop for DiscoveryService {
    fn drop(&mut self) {
        self.stop();
    }
}
