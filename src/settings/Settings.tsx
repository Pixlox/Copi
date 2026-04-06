import { useEffect, useState, useCallback, useRef, type MouseEvent as ReactMouseEvent } from "react";
import { getVersion } from "@tauri-apps/api/app";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import {
  Keyboard,
  Palette,
  Shield,
  Wifi,
  HardDrive,
  Sun,
  Moon,
  Monitor,
  Plus,
  X,
  Trash2,
  RefreshCw,
  FolderOpen,
  Pencil,
  Check,
  Laptop,
  Shell,
} from "lucide-react";
import { useThemeContext } from "../contexts/ThemeContext";
import Picker from "../components/Picker";
import { checkForUpdates } from "../utils/updater";
import { formatShortcut, isMacPlatform, platformName } from "../utils/platform";

// ════════════════════════════════════════════════════════════════════════════
// Types
// ════════════════════════════════════════════════════════════════════════════

interface CopiConfig {
  general: {
    hotkey: string;
    launch_at_login: boolean;
    default_paste_behaviour: string;
    history_retention_days: number;
    auto_check_updates: boolean;
  };
  appearance: {
    theme: string;
    compact_mode: boolean;
    show_app_icons: boolean;
  };
  privacy: {
    excluded_apps: string[];
  };
  sync: {
    enabled: boolean;
    device_name: string | null;
    auto_connect: boolean;
    sync_embeddings: boolean;
    sync_collections_and_pins: boolean;
  };
}

interface SyncIdentity {
  device_id: string;
  device_name: string;
}

interface SyncStatus {
  enabled: boolean;
  connectedCount: number;
  deviceId?: string;
  deviceName?: string;
}

interface SyncPairedDevice {
  device_id: string;
  display_name: string;
  online: boolean;
}

interface SyncDiscoveredDevice {
  device_id: string;
  display_name: string;
  addr: string;
  pin: string;
}

interface SyncPinPayload {
  pin: string;
  expires_at: number;
}

interface CollectionInfo {
  id: number;
  name: string;
  color: string;
  clip_count: number;
  created_at: number;
}

type Section = "general" | "appearance" | "privacy" | "sync" | "data" | "collections";

const SECTIONS: { id: Section; label: string; icon: typeof Keyboard }[] = [
  { id: "general", label: "General", icon: Keyboard },
  { id: "appearance", label: "Appearance", icon: Palette },
  { id: "privacy", label: "Privacy", icon: Shield },
  { id: "sync", label: "Sync", icon: Wifi },
  { id: "data", label: "Data", icon: HardDrive },
  { id: "collections", label: "Collections", icon: FolderOpen },
];

const RETENTION_OPTIONS = [
  { label: "7 days", value: "7" },
  { label: "30 days", value: "30" },
  { label: "90 days", value: "90" },
  { label: "1 year", value: "365" },
  { label: "Forever", value: "0" },
];

const COLLECTION_COLORS = [
  "#0A84FF", "#34C759", "#FF9500", "#FF3B30",
  "#AF52DE", "#FF2D55", "#5AC8FA", "#FFD60A",
];

function getHotkeyPresets() {
  return isMacPlatform
    ? [
        { label: formatShortcut("alt+space", " + "), value: "alt+space" },
        { label: formatShortcut("cmd+shift+space", " + "), value: "cmd+shift+space" },
        { label: formatShortcut("cmd+shift+v", " + "), value: "cmd+shift+v" },
        { label: formatShortcut("ctrl+shift+space", " + "), value: "ctrl+shift+space" },
      ]
    : [
        { label: formatShortcut("alt+space"), value: "alt+space" },
        { label: formatShortcut("ctrl+shift+space"), value: "ctrl+shift+space" },
        { label: formatShortcut("ctrl+shift+v"), value: "ctrl+shift+v" },
        { label: formatShortcut("ctrl+alt+v"), value: "ctrl+alt+v" },
      ];
}

function formatBytes(bytes: number): string {
  if (bytes === 0) return "0 B";
  const k = 1024;
  const sizes = ["B", "KB", "MB", "GB"];
  const i = Math.floor(Math.log(bytes) / Math.log(k));
  return `${parseFloat((bytes / Math.pow(k, i)).toFixed(1))} ${sizes[i]}`;
}

// ════════════════════════════════════════════════════════════════════════════
// UI Components
// ════════════════════════════════════════════════════════════════════════════

function Logo({ size = 28 }: { size?: number }) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 512 512"
      xmlns="http://www.w3.org/2000/svg"
    >
      <defs>
        <linearGradient id="settings-bg" x1="0%" y1="0%" x2="100%" y2="100%">
          <stop offset="0%" stopColor="#1a1a1f" />
          <stop offset="100%" stopColor="#111114" />
        </linearGradient>
        <linearGradient id="settings-front" x1="0%" y1="0%" x2="100%" y2="100%">
          <stop offset="0%" stopColor="#ffffff" />
          <stop offset="100%" stopColor="#c8c8d0" />
        </linearGradient>
        <linearGradient id="settings-back" x1="0%" y1="0%" x2="100%" y2="100%">
          <stop offset="0%" stopColor="#ffffff" stopOpacity="0.22" />
          <stop offset="100%" stopColor="#ffffff" stopOpacity="0.10" />
        </linearGradient>
      </defs>
      <g transform="translate(56, 56) scale(0.781)">
        <rect width="512" height="512" rx="112" fill="url(#settings-bg)" />
        <rect x="190" y="170" width="196" height="236" rx="28" fill="url(#settings-back)" stroke="rgba(255,255,255,0.12)" strokeWidth="1.5" />
        <rect x="158" y="138" width="196" height="236" rx="28" fill="url(#settings-front)" />
        <rect x="188" y="186" width="80" height="8" rx="4" fill="#1a1a1f" opacity="0.18" />
        <rect x="188" y="206" width="136" height="8" rx="4" fill="#1a1a1f" opacity="0.12" />
        <rect x="188" y="226" width="112" height="8" rx="4" fill="#1a1a1f" opacity="0.12" />
      </g>
    </svg>
  );
}

function Toggle({ checked, onChange }: { checked: boolean; onChange: (v: boolean) => void }) {
  return (
    <button
      type="button"
      className={`settings-toggle ${checked ? "on" : ""}`}
      onClick={() => onChange(!checked)}
      role="switch"
      aria-checked={checked}
    >
      <div className="settings-toggle-knob" />
    </button>
  );
}

function SettingRow({
  label,
  description,
  children,
}: {
  label: string;
  description?: string;
  children?: React.ReactNode;
}) {
  return (
    <div className="settings-row">
      <div className="settings-row-info">
        <span className="settings-row-label">{label}</span>
        {description && <span className="settings-row-desc">{description}</span>}
      </div>
      {children && <div className="settings-row-control">{children}</div>}
    </div>
  );
}

function SettingCard({ children, className }: { children: React.ReactNode; className?: string }) {
  return <div className={`settings-card ${className || ""}`}>{children}</div>;
}

function SettingDivider() {
  return <div className="settings-divider" />;
}

// ════════════════════════════════════════════════════════════════════════════
// Section Content Components
// ════════════════════════════════════════════════════════════════════════════

function GeneralSection({
  config,
  saveConfig,
}: {
  config: CopiConfig;
  saveConfig: (c: CopiConfig) => void;
}) {
  return (
    <>
      <SettingCard>
        <SettingRow label="Global Hotkey" description="The shortcut that opens Copi">
          <Picker
            value={config.general.hotkey}
            options={getHotkeyPresets()}
            onChange={(val) => saveConfig({ ...config, general: { ...config.general, hotkey: val } })}
          />
        </SettingRow>
      </SettingCard>

      <SettingCard>
        <SettingRow label="Launch at Login" description={`Start Copi when your ${platformName} starts`}>
          <Toggle
            checked={config.general.launch_at_login}
            onChange={(v) => saveConfig({ ...config, general: { ...config.general, launch_at_login: v } })}
          />
        </SettingRow>
      </SettingCard>

      <SettingCard>
        <SettingRow label="Default Action" description="What Enter does in the overlay">
          <div className="settings-segment">
            <button
              className={config.general.default_paste_behaviour === "copy" ? "active" : ""}
              onClick={() => saveConfig({ ...config, general: { ...config.general, default_paste_behaviour: "copy" } })}
            >
              Copy
            </button>
            <button
              className={config.general.default_paste_behaviour === "paste" ? "active" : ""}
              onClick={() => saveConfig({ ...config, general: { ...config.general, default_paste_behaviour: "paste" } })}
            >
              Paste
            </button>
          </div>
        </SettingRow>
      </SettingCard>

      <SettingCard>
        <SettingRow label="History Retention" description="Auto-delete clips older than this">
          <Picker
            value={String(config.general.history_retention_days)}
            options={RETENTION_OPTIONS}
            onChange={(val) => saveConfig({ ...config, general: { ...config.general, history_retention_days: parseInt(val) } })}
          />
        </SettingRow>
      </SettingCard>

      <SettingCard>
        <SettingRow label="Auto-check for Updates" description="Check for updates on startup">
          <Toggle
            checked={config.general.auto_check_updates}
            onChange={(v) => saveConfig({ ...config, general: { ...config.general, auto_check_updates: v } })}
          />
        </SettingRow>
      </SettingCard>
    </>
  );
}

function AppearanceSection({
  config,
  saveConfig,
}: {
  config: CopiConfig;
  saveConfig: (c: CopiConfig) => void;
}) {
  const { theme, setTheme } = useThemeContext();

  return (
    <>
      <SettingCard>
        <SettingRow label="Theme" description="Choose your color scheme">
          <div className="settings-segment">
            <button className={theme === "dark" ? "active" : ""} onClick={() => setTheme("dark")}>
              <Moon size={12} /> Dark
            </button>
            <button className={theme === "light" ? "active" : ""} onClick={() => setTheme("light")}>
              <Sun size={12} /> Light
            </button>
            <button className={theme === "system" ? "active" : ""} onClick={() => setTheme("system")}>
              <Monitor size={12} /> Auto
            </button>
          </div>
        </SettingRow>
      </SettingCard>

      <SettingCard>
        <SettingRow label="Compact Mode" description="Smaller rows in the overlay">
          <Toggle
            checked={config.appearance.compact_mode}
            onChange={(v) => saveConfig({ ...config, appearance: { ...config.appearance, compact_mode: v } })}
          />
        </SettingRow>
      </SettingCard>

      <SettingCard>
        <SettingRow label="Show App Icons" description="Source app icons next to clips">
          <Toggle
            checked={config.appearance.show_app_icons}
            onChange={(v) => saveConfig({ ...config, appearance: { ...config.appearance, show_app_icons: v } })}
          />
        </SettingRow>
      </SettingCard>
    </>
  );
}

function PrivacySection({
  config,
  saveConfig,
}: {
  config: CopiConfig;
  saveConfig: (c: CopiConfig) => void;
}) {
  const [newApp, setNewApp] = useState("");

  const addApp = () => {
    if (!newApp.trim()) return;
    saveConfig({
      ...config,
      privacy: { ...config.privacy, excluded_apps: [...config.privacy.excluded_apps, newApp.trim()] },
    });
    setNewApp("");
  };

  const removeApp = (index: number) => {
    const apps = [...config.privacy.excluded_apps];
    apps.splice(index, 1);
    saveConfig({ ...config, privacy: { ...config.privacy, excluded_apps: apps } });
  };

  return (
    <SettingCard>
      <SettingRow label="Excluded Apps" description="Content from these apps won't be captured" />
      <SettingDivider />
      
      <div className="settings-chips">
        {config.privacy.excluded_apps.length === 0 && (
          <span className="settings-chips-empty">No excluded apps</span>
        )}
        {config.privacy.excluded_apps.map((app, i) => (
          <span key={`${app}-${i}`} className="settings-chip">
            {app}
            <button onClick={() => removeApp(i)}>
              <X size={10} />
            </button>
          </span>
        ))}
      </div>

      <div className="settings-add-row">
        <input
          type="text"
          value={newApp}
          onChange={(e) => setNewApp(e.target.value)}
          placeholder="App name or bundle ID…"
          onKeyDown={(e) => e.key === "Enter" && addApp()}
        />
        <button onClick={addApp} disabled={!newApp.trim()}>
          <Plus size={14} />
        </button>
      </div>
    </SettingCard>
  );
}

function DataSection({
  dbSize,
  clipCount,
  appVersion,
  onClearHistory,
}: {
  dbSize: number;
  clipCount: number;
  appVersion: string | null;
  onClearHistory: () => void;
}) {
  const [checkingUpdate, setCheckingUpdate] = useState(false);

  return (
    <>
      <SettingCard>
        <SettingRow label="Database Size">
          <span className="settings-value">{formatBytes(dbSize)}</span>
        </SettingRow>
        <SettingRow label="Total Clips">
          <span className="settings-value">{clipCount.toLocaleString()}</span>
        </SettingRow>
        <SettingRow label="Version">
          <span className="settings-value">{appVersion ? `v${appVersion}` : "—"}</span>
        </SettingRow>
      </SettingCard>

      <SettingCard>
        <SettingRow label="Check for Updates">
          <button
            className="settings-btn"
            disabled={checkingUpdate}
            onClick={async () => {
              setCheckingUpdate(true);
              try {
                await checkForUpdates("interactive");
              } finally {
                setCheckingUpdate(false);
              }
            }}
          >
            <RefreshCw size={12} className={checkingUpdate ? "animate-spin" : ""} />
            {checkingUpdate ? "Checking…" : "Check Now"}
          </button>
        </SettingRow>
      </SettingCard>

      <SettingCard>
        <SettingRow label="Clear All History" description="Permanently delete all clipboard data">
          <button className="settings-btn danger" onClick={onClearHistory}>
            <Trash2 size={12} />
            Clear
          </button>
        </SettingRow>
      </SettingCard>

      <div className="settings-attribution">Made with {"<3"} by Megumi Labs</div>
    </>
  );
}

function SyncSection() {
  const [config, setConfig] = useState<CopiConfig | null>(null);
  const [identity, setIdentity] = useState<SyncIdentity | null>(null);
  const [status, setStatus] = useState<SyncStatus | null>(null);
  const [pairedDevices, setPairedDevices] = useState<SyncPairedDevice[]>([]);
  const [discoveredDevices, setDiscoveredDevices] = useState<SyncDiscoveredDevice[]>([]);
  const [pairingCode, setPairingCode] = useState<SyncPinPayload | null>(null);
  const [pairingError, setPairingError] = useState<string | null>(null);
  const [pairingBusyAddr, setPairingBusyAddr] = useState<string | null>(null);
  const [manualTargetAddr, setManualTargetAddr] = useState("");
  const [manualPin, setManualPin] = useState("");
  const [manualPairingBusy, setManualPairingBusy] = useState(false);
  const [countdownTick, setCountdownTick] = useState(0);

  const formatError = (e: unknown): string =>
    typeof e === "string"
      ? e
      : e instanceof Error
      ? e.message
      : "Sync operation failed";

  const refreshIdentity = useCallback(() => {
    invoke<SyncIdentity>("sync_get_identity").then(setIdentity).catch(() => {});
  }, []);

  const refreshPeers = useCallback(() => {
    invoke<SyncPairedDevice[]>("sync_list_peers")
      .then(setPairedDevices)
      .catch(() => {});
  }, []);

  const refreshConfig = useCallback(() => {
    invoke<CopiConfig>("get_config").then(setConfig).catch(() => {});
  }, []);

  const refreshStatus = useCallback(() => {
    invoke<SyncStatus>("sync_get_status").then(setStatus).catch(() => {});
  }, []);

  const saveSyncConfig = useCallback(
    async (updater: (cfg: CopiConfig) => CopiConfig) => {
      if (!config) return;
      const next = updater(config);
      setConfig(next);
      await invoke("set_config", { config: next });
    },
    [config]
  );

  const refreshSync = useCallback(() => {
    refreshIdentity();
    refreshStatus();
    refreshPeers();
    refreshConfig();
  }, [refreshConfig, refreshIdentity, refreshPeers, refreshStatus]);

  const refreshDiscovered = useCallback(() => {
    invoke<Array<{ device_id: string; display_name: string; addr: string }>>("sync_list_discovered")
      .then((items) => {
        setDiscoveredDevices((prev) => {
          const byId = new Map(prev.map((item) => [item.device_id, item]));
          const next = items.map((item) => ({
            device_id: item.device_id,
            display_name: item.display_name,
            addr: item.addr,
            pin: byId.get(item.device_id)?.pin ?? "",
          }));
          return next.sort((a, b) => a.display_name.localeCompare(b.display_name));
        });
      })
      .catch(() => {});
  }, []);

  const upsertDiscovered = useCallback(
    (payload: { device_id: string; display_name: string; addr: string }) => {
      setDiscoveredDevices((prev) => {
        const next = prev.filter((item) => item.device_id !== payload.device_id);
        const previous = prev.find((item) => item.device_id === payload.device_id);
        next.push({
          device_id: payload.device_id,
          display_name: payload.display_name,
          addr: payload.addr,
          pin: previous?.pin ?? "",
        });
        return next.sort((a, b) => a.display_name.localeCompare(b.display_name));
      });
    },
    []
  );

  useEffect(() => {
    refreshSync();
    refreshDiscovered();
  }, [refreshDiscovered, refreshSync]);

  useEffect(() => {
    const unlistenConfig = listen("sync:config-updated", () => {
      refreshSync();
      refreshDiscovered();
    });
    return () => {
      unlistenConfig.then((fn) => fn());
    };
  }, [refreshDiscovered, refreshSync]);

  useEffect(() => {
    const unlistenPaired = listen("sync:paired", () => {
      refreshPeers();
    });
    const unlistenConnected = listen("sync:connected", () => {
      refreshPeers();
    });
    const unlistenDisconnected = listen("sync:disconnected", () => {
      refreshPeers();
    });
    const unlistenDiscovered = listen<{ device_id: string; display_name: string; addr: string }>(
      "sync:discovered",
      (event) => {
        upsertDiscovered(event.payload);
      }
    );

    return () => {
      unlistenPaired.then((fn) => fn());
      unlistenConnected.then((fn) => fn());
      unlistenDisconnected.then((fn) => fn());
      unlistenDiscovered.then((fn) => fn());
    };
  }, [refreshPeers, upsertDiscovered]);

  // Tick every second while a pairing code is active to update the countdown
  useEffect(() => {
    if (!pairingCode) return;
    
    const now = Math.floor(Date.now() / 1000);
    const remaining = pairingCode.expires_at - now;
    
    // If already expired, don't start the timer
    if (remaining <= 0) return;
    
    const interval = setInterval(() => {
      setCountdownTick((t) => t + 1);
    }, 1000);
    
    return () => clearInterval(interval);
  }, [pairingCode]);

  // Compute remaining time using the tick to force re-render
  const codeRemainingSeconds = pairingCode
    ? Math.max(0, pairingCode.expires_at - Math.floor(Date.now() / 1000))
    : 0;
  // Use countdownTick to suppress the unused variable warning
  void countdownTick;

  return (
    <>
      <SettingCard>
        <SettingRow label="Enable Sync" description="Turn LAN sync on or off for this device">
          <Toggle
            checked={Boolean(config?.sync.enabled)}
            onChange={(value) => {
              void saveSyncConfig((cfg) => ({
                ...cfg,
                sync: {
                  ...cfg.sync,
                  enabled: value,
                },
              }));
            }}
          />
        </SettingRow>
        <SettingDivider />
        <SettingRow label="Device Name" description="Override hostname shown to peers">
          <input
            className="settings-sync-input"
            value={config?.sync.device_name ?? ""}
            onChange={(event) => {
              const value = event.target.value;
              setConfig((prev) =>
                prev
                  ? {
                      ...prev,
                      sync: {
                        ...prev.sync,
                        device_name: value,
                      },
                    }
                  : prev
              );
            }}
            onBlur={() => {
              void saveSyncConfig((cfg) => ({
                ...cfg,
                sync: {
                  ...cfg.sync,
                  device_name:
                    (cfg.sync.device_name ?? "").trim().length > 0
                      ? (cfg.sync.device_name ?? "").trim()
                      : null,
                },
              }));
            }}
            placeholder="Auto-detected hostname"
          />
        </SettingRow>
        <SettingDivider />
        <SettingRow label="Auto-connect" description="Reconnect to trusted peers automatically">
          <Toggle
            checked={Boolean(config?.sync.auto_connect ?? true)}
            onChange={(value) => {
              void saveSyncConfig((cfg) => ({
                ...cfg,
                sync: {
                  ...cfg.sync,
                  auto_connect: value,
                },
              }));
            }}
          />
        </SettingRow>
        <SettingDivider />
        <SettingRow
          label="Sync Collections & Pins"
          description="Share collection metadata and pinned state across paired devices"
        >
          <Toggle
            checked={Boolean(config?.sync.sync_collections_and_pins ?? false)}
            onChange={(value) => {
              void saveSyncConfig((cfg) => ({
                ...cfg,
                sync: {
                  ...cfg.sync,
                  sync_collections_and_pins: value,
                },
              }));
            }}
          />
        </SettingRow>
      </SettingCard>

      <SettingCard>
        <SettingRow label="Status" description="Current LAN sync service status">
          <button
            className="settings-btn"
            onClick={() => {
              refreshSync();
              refreshDiscovered();
            }}
          >
            <RefreshCw size={12} /> Refresh
          </button>
        </SettingRow>
        <SettingDivider />
        <div className="settings-sync-metrics">
          <div className="settings-sync-metric">
            <span>Mode</span>
            <strong>{config?.sync.enabled ? "Enabled" : "Disabled"}</strong>
          </div>
          <div className="settings-sync-metric">
            <span>Connected</span>
            <strong>{status?.connectedCount ?? pairedDevices.filter((device) => device.online).length}</strong>
          </div>
          <div className="settings-sync-metric">
            <span>Paired</span>
            <strong>{pairedDevices.length}</strong>
          </div>
          <div className="settings-sync-metric">
            <span>Found</span>
            <strong>{discoveredDevices.length}</strong>
          </div>
        </div>
        {identity && (
          <div className="settings-sync-device-pill">
            <Laptop size={12} />
            <span>
              {identity.device_name} ({identity.device_id.slice(0, 8)})
            </span>
          </div>
        )}
      </SettingCard>

      <SettingCard>
        <SettingRow label="Pairing PIN" description="Generate a 6-digit PIN and enter it on the other device">
          <button
            className="settings-btn primary"
            onClick={async () => {
              setPairingError(null);
              try {
                const payload = await invoke<SyncPinPayload>("sync_generate_pin");
                setPairingCode(payload);
              } catch (e) {
                setPairingError(formatError(e));
              }
            }}
          >
            <Wifi size={12} /> Generate
          </button>
        </SettingRow>
        <SettingDivider />
        <div className="settings-sync-code-box">
          <span className="settings-sync-code-value" style={{ letterSpacing: "0.2em" }}>
            {pairingCode?.pin ?? "------"}
          </span>
          <span className="settings-sync-code-exp">
            {pairingCode
              ? codeRemainingSeconds > 0
                ? `Expires in ${codeRemainingSeconds}s`
                : "Code expired"
              : "No active code"}
          </span>
        </div>
      </SettingCard>

      <SettingCard>
        <SettingRow
          label="Pair by Address"
          description="Fallback when discovery is blocked: enter peer IP (or IP:port) and PIN"
        />
        <SettingDivider />
        {pairingError && <span className="settings-sync-error">{pairingError}</span>}
        <div className="settings-sync-pair-form">
          <div className="settings-sync-pair-grid">
            <div className="settings-sync-pair-field">
              <span className="settings-sync-pair-label">Peer Address</span>
              <input
                className="settings-sync-input"
                placeholder="192.168.1.153 or 192.168.1.153:51827"
                value={manualTargetAddr}
                onChange={(e) => setManualTargetAddr(e.target.value)}
              />
            </div>
            <div className="settings-sync-pair-field settings-sync-pair-field--pin">
              <span className="settings-sync-pair-label">PIN</span>
              <input
                className="settings-sync-input"
                placeholder="6-digit PIN"
                value={manualPin}
                onChange={(e) => setManualPin(e.target.value.replace(/[^0-9]/g, "").slice(0, 6))}
              />
            </div>
          </div>
          <div className="settings-sync-pair-actions">
            <button
              className="settings-btn primary"
              disabled={manualPairingBusy || manualPin.length !== 6 || manualTargetAddr.trim().length === 0}
              onClick={async () => {
                const targetAddr = manualTargetAddr.trim();
                if (!targetAddr || manualPin.length !== 6) return;
                setManualPairingBusy(true);
                setPairingError(null);
                try {
                  await invoke("sync_pair_with", { targetAddr, target_addr: targetAddr, pin: manualPin });
                  setManualPin("");
                  refreshStatus();
                  refreshPeers();
                  refreshDiscovered();
                } catch (e) {
                  setPairingError(formatError(e));
                } finally {
                  setManualPairingBusy(false);
                }
              }}
            >
              {manualPairingBusy ? "Pairing…" : "Pair by Address"}
            </button>
          </div>
        </div>
      </SettingCard>

      <SettingCard>
        <SettingRow label="Found on this network" description="Discovered devices that can be paired">
          <button
            className="settings-btn"
            onClick={() => {
              refreshSync();
              refreshDiscovered();
            }}
          >
            <RefreshCw size={12} /> Refresh
          </button>
        </SettingRow>
        <SettingDivider />
        {pairingError && <span className="settings-sync-error">{pairingError}</span>}
        <div className="settings-sync-list">
          {discoveredDevices.filter((d) => !pairedDevices.some((p) => p.device_id === d.device_id)).length === 0 && (
            <div className="settings-row">
              <div className="settings-row-info">
                <span className="settings-row-label" style={{ color: "var(--text-tertiary)" }}>
                  No devices discovered yet
                </span>
              </div>
            </div>
          )}
          {discoveredDevices
            .filter((d) => !pairedDevices.some((p) => p.device_id === d.device_id))
            .map((device) => (
            <div key={device.device_id} className="settings-sync-device-row">
              <div style={{ flex: 1, minWidth: 0 }}>
                <div className="settings-sync-device-name">{device.display_name}</div>
                <div className="settings-sync-device-meta">{device.addr}</div>
                <input
                  className="settings-sync-input"
                  placeholder="Enter 6-digit PIN"
                  value={device.pin}
                  onChange={(e) => {
                    const pin = e.target.value.replace(/[^0-9]/g, "").slice(0, 6);
                    setDiscoveredDevices((prev) =>
                      prev.map((item) =>
                        item.device_id === device.device_id ? { ...item, pin } : item
                      )
                    );
                  }}
                  style={{ marginTop: 6 }}
                />
              </div>
              <button
                className="settings-btn primary"
                disabled={device.pin.length !== 6 || pairingBusyAddr === device.addr}
                onClick={async () => {
                  setPairingBusyAddr(device.addr);
                  setPairingError(null);
                  try {
                    await invoke("sync_pair_with", { targetAddr: device.addr, target_addr: device.addr, pin: device.pin });
                    setDiscoveredDevices((prev) =>
                      prev.filter((item) => item.device_id !== device.device_id)
                    );
                    refreshStatus();
                    refreshPeers();
                  } catch (e) {
                    const message = formatError(e);
                    setPairingError(message);
                    console.error("Pair by code failed", e);
                  } finally {
                    setPairingBusyAddr(null);
                  }
                }}
              >
                {pairingBusyAddr === device.addr ? "Pairing…" : "Pair"}
              </button>
            </div>
          ))}
        </div>
      </SettingCard>

      <SettingCard>
        <SettingRow label="Paired Devices" description="Trusted devices and online status" />
        <SettingDivider />
        <div className="settings-sync-list">
          {pairedDevices.length === 0 && (
            <div className="settings-row">
              <div className="settings-row-info">
                <span className="settings-row-label" style={{ color: "var(--text-tertiary)" }}>
                  No paired devices yet
                </span>
              </div>
            </div>
          )}
          {pairedDevices.map((device) => (
            <div key={device.device_id} className="settings-sync-device-row">
              <div>
                <div className="settings-sync-device-name">{device.display_name}</div>
                <div className="settings-sync-device-meta">{device.device_id}</div>
              </div>
              <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
                <span className="settings-sync-device-meta">{device.online ? "Online" : "Offline"}</span>
                <button
                  className="settings-btn danger"
                  onClick={async () => {
                    await invoke("sync_remove_peer", { deviceId: device.device_id, device_id: device.device_id });
                    refreshStatus();
                    refreshPeers();
                  }}
                >
                  Remove
                </button>
              </div>
            </div>
          ))}
        </div>
      </SettingCard>
    </>
  );
}

function CollectionsSection({
  collections,
  onDelete,
  onRename,
  onCreate,
  onUpdateColor,
}: {
  collections: CollectionInfo[];
  onDelete: (id: number) => void;
  onRename: (id: number, name: string) => void;
  onCreate: (name: string, color: string) => void;
  onUpdateColor: (id: number, color: string) => void;
}) {
  const [editingId, setEditingId] = useState<number | null>(null);
  const [editValue, setEditValue] = useState("");
  const [colorPickerId, setColorPickerId] = useState<number | null>(null);
  const [newName, setNewName] = useState("");
  const [isCreating, setIsCreating] = useState(false);
  const editInputRef = useRef<HTMLInputElement>(null);
  const newInputRef = useRef<HTMLInputElement>(null);
  const colorPickerRef = useRef<HTMLDivElement>(null);

  // Focus input when entering edit mode
  useEffect(() => {
    if (editingId !== null && editInputRef.current) {
      editInputRef.current.focus();
      editInputRef.current.select();
    }
  }, [editingId]);

  // Focus input when creating new collection
  useEffect(() => {
    if (isCreating && newInputRef.current) {
      newInputRef.current.focus();
    }
  }, [isCreating]);

  // Close color picker when clicking outside
  useEffect(() => {
    if (colorPickerId === null) return;
    
    const handleClickOutside = (e: MouseEvent) => {
      if (colorPickerRef.current && !colorPickerRef.current.contains(e.target as Node)) {
        setColorPickerId(null);
      }
    };
    
    document.addEventListener("mousedown", handleClickOutside);
    return () => document.removeEventListener("mousedown", handleClickOutside);
  }, [colorPickerId]);

  const startEditing = (col: CollectionInfo) => {
    setEditingId(col.id);
    setEditValue(col.name);
    setColorPickerId(null);
  };

  const saveEdit = () => {
    if (editingId !== null && editValue.trim()) {
      onRename(editingId, editValue.trim());
    }
    setEditingId(null);
    setEditValue("");
  };

  const cancelEdit = () => {
    setEditingId(null);
    setEditValue("");
  };

  const handleCreate = () => {
    if (!newName.trim()) return;
    const color = COLLECTION_COLORS[collections.length % COLLECTION_COLORS.length];
    onCreate(newName.trim(), color);
    setNewName("");
    setIsCreating(false);
  };

  const handleColorSelect = (id: number, color: string) => {
    onUpdateColor(id, color);
    setColorPickerId(null);
  };

  return (
    <>
      <SettingCard className={colorPickerId !== null ? "settings-card--picker-open" : ""}>
        {collections.length === 0 && !isCreating ? (
          <div className="settings-row">
            <div className="settings-row-info">
              <span className="settings-row-label" style={{ color: "var(--text-tertiary)" }}>
                No collections yet
              </span>
            </div>
          </div>
        ) : (
          collections.map((col, i) => (
            <div key={col.id}>
              {i > 0 && <SettingDivider />}
              <div className="settings-row settings-collection-row">
                {/* Color picker */}
                <div className="settings-color-picker-container" ref={colorPickerId === col.id ? colorPickerRef : undefined}>
                  <button
                    className="settings-collection-color-btn"
                    style={{ background: col.color }}
                    onClick={() => setColorPickerId(colorPickerId === col.id ? null : col.id)}
                    title="Change color"
                  />
                  {colorPickerId === col.id && (
                    <div className="settings-color-picker-popover">
                      {COLLECTION_COLORS.map((color) => (
                        <button
                          key={color}
                          className={`settings-color-option ${color === col.color ? "selected" : ""}`}
                          style={{ background: color }}
                          onClick={() => handleColorSelect(col.id, color)}
                        />
                      ))}
                    </div>
                  )}
                </div>

                {/* Name (editable) */}
                <div className="settings-collection-info">
                  {editingId === col.id ? (
                    <input
                      ref={editInputRef}
                      type="text"
                      className="settings-collection-edit-input"
                      value={editValue}
                      onChange={(e) => setEditValue(e.target.value)}
                      onKeyDown={(e) => {
                        if (e.key === "Enter") saveEdit();
                        if (e.key === "Escape") cancelEdit();
                      }}
                      onBlur={saveEdit}
                    />
                  ) : (
                    <span
                      className="settings-collection-name"
                      onDoubleClick={() => startEditing(col)}
                      title="Double-click to rename"
                    >
                      {col.name}
                    </span>
                  )}
                  <span className="settings-collection-count">
                    {col.clip_count} clip{col.clip_count !== 1 ? "s" : ""}
                  </span>
                </div>

                {/* Actions */}
                <div className="settings-collection-actions">
                  {editingId !== col.id && (
                    <button
                      onClick={() => startEditing(col)}
                      className="settings-collection-action-btn"
                      title="Rename"
                    >
                      <Pencil size={12} />
                    </button>
                  )}
                  <button
                    onClick={() => onDelete(col.id)}
                    className="settings-collection-action-btn settings-collection-action-btn--danger"
                    title="Delete"
                  >
                    <Trash2 size={12} />
                  </button>
                </div>
              </div>
            </div>
          ))
        )}

        {/* Create new collection row */}
        {collections.length > 0 && <SettingDivider />}
        <div className="settings-collection-create-row">
          {isCreating ? (
            <>
              <div
                className="settings-collection-color-btn"
                style={{ background: COLLECTION_COLORS[collections.length % COLLECTION_COLORS.length] }}
              />
              <input
                ref={newInputRef}
                type="text"
                className="settings-collection-new-input"
                placeholder="Collection name..."
                value={newName}
                onChange={(e) => setNewName(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter") handleCreate();
                  if (e.key === "Escape") {
                    setIsCreating(false);
                    setNewName("");
                  }
                }}
                onBlur={() => {
                  if (!newName.trim()) {
                    setIsCreating(false);
                  }
                }}
              />
              <button
                className="settings-collection-action-btn settings-collection-action-btn--confirm"
                onClick={handleCreate}
                disabled={!newName.trim()}
              >
                <Check size={14} />
              </button>
              <button
                className="settings-collection-action-btn"
                onClick={() => {
                  setIsCreating(false);
                  setNewName("");
                }}
              >
                <X size={14} />
              </button>
            </>
          ) : (
            <button className="settings-collection-add-btn" onClick={() => setIsCreating(true)}>
              <Plus size={14} />
              <span>New Collection</span>
            </button>
          )}
        </div>
      </SettingCard>
    </>
  );
}

// ════════════════════════════════════════════════════════════════════════════
// Main Component
// ════════════════════════════════════════════════════════════════════════════

export default function Settings() {
  const [config, setConfig] = useState<CopiConfig | null>(null);
  const [activeSection, setActiveSection] = useState<Section>("general");
  const [dbSize, setDbSize] = useState(0);
  const [clipCount, setClipCount] = useState(0);
  const [appVersion, setAppVersion] = useState<string | null>(null);
  const [collections, setCollections] = useState<CollectionInfo[]>([]);
  const [confirmClear, setConfirmClear] = useState(false);
  const [clearError, setClearError] = useState<string | null>(null);
  const [clearing, setClearing] = useState(false);

  const saveTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const statsRefreshTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  const fetchCollections = useCallback(() => {
    invoke<CollectionInfo[]>("list_collections").then(setCollections).catch(() => {});
  }, []);

  const refreshStats = useCallback(async () => {
    const [sizeResult, countResult] = await Promise.allSettled([
      invoke<number>("get_db_size"),
      invoke<number>("get_total_clip_count"),
    ]);

    if (sizeResult.status === "fulfilled") {
      setDbSize(sizeResult.value);
    }
    if (countResult.status === "fulfilled") {
      setClipCount(countResult.value);
    }
  }, []);

  const scheduleStatsRefresh = useCallback(
    (delayMs: number = 90) => {
      if (statsRefreshTimer.current) {
        clearTimeout(statsRefreshTimer.current);
      }
      statsRefreshTimer.current = setTimeout(() => {
        void refreshStats();
      }, delayMs);
    },
    [refreshStats]
  );

  useEffect(() => {
    invoke<CopiConfig>("get_config").then(setConfig).catch(console.error);
    void refreshStats();
    getVersion().then(setAppVersion).catch(() => {});
    fetchCollections();
  }, [fetchCollections, refreshStats]);

  // Listen to collections-changed event for real-time updates
  useEffect(() => {
    const unlisten = listen("collections-changed", () => {
      fetchCollections();
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, [fetchCollections]);

  useEffect(() => {
    const unlistenNew = listen("new-clip", () => {
      scheduleStatsRefresh();
    });
    const unlistenChanged = listen("clips-changed", () => {
      scheduleStatsRefresh();
    });
    const unlistenShown = listen("settings:shown", () => {
      scheduleStatsRefresh(0);
      fetchCollections();
    });

    return () => {
      if (statsRefreshTimer.current) {
        clearTimeout(statsRefreshTimer.current);
      }
      unlistenNew.then((fn) => fn());
      unlistenChanged.then((fn) => fn());
      unlistenShown.then((fn) => fn());
    };
  }, [fetchCollections, scheduleStatsRefresh]);

  const saveConfig = useCallback(
    async (updated: CopiConfig) => {
      const previous = config;
      setConfig(updated);
      if (saveTimer.current) clearTimeout(saveTimer.current);
      saveTimer.current = setTimeout(async () => {
        try {
          await invoke("set_config", { config: updated });
        } catch (e) {
          console.error("Save failed:", e);
          if (previous) setConfig(previous);
        }
      }, 150);
    },
    [config]
  );

  const handleDeleteCollection = async (id: number) => {
    try {
      await invoke("delete_collection", { id });
      fetchCollections();
    } catch (e) {
      console.error("Delete collection failed:", e);
    }
  };

  const handleRenameCollection = async (id: number, name: string) => {
    try {
      await invoke("rename_collection", { id, name });
      fetchCollections();
    } catch (e) {
      console.error("Rename collection failed:", e);
    }
  };

  const handleCreateCollection = async (name: string, color: string) => {
    try {
      await invoke("create_collection", { name, color });
      fetchCollections();
    } catch (e) {
      console.error("Create collection failed:", e);
    }
  };

  const handleUpdateCollectionColor = async (id: number, color: string) => {
    try {
      await invoke("update_collection_color", { id, color });
      fetchCollections();
    } catch (e) {
      console.error("Update collection color failed:", e);
    }
  };

  const handleClearHistory = async () => {
    setClearing(true);
    setClearError(null);
    try {
      await invoke("clear_all_history");
      await refreshStats();
      setConfirmClear(false);
    } catch (e) {
      const msg = typeof e === "string" ? e : (e instanceof Error ? e.message : "Unknown error");
      setClearError(msg);
      console.error("Clear failed:", e);
    } finally {
      setClearing(false);
    }
  };

  const handleWindowDragStart = useCallback((event: ReactMouseEvent<HTMLElement>) => {
    if (!isMacPlatform || event.button !== 0) return;

    const target = event.target as HTMLElement;
    if (target.closest("button, input, textarea, select, a, [role='button'], [data-no-drag]")) {
      return;
    }

    void getCurrentWindow().startDragging();
  }, []);

  if (!config) {
    return (
      <div className="settings-root settings-loading">
        <span>Loading…</span>
      </div>
    );
  }

  const activeSectionData = SECTIONS.find((s) => s.id === activeSection)!;

  return (
    <div className="settings-root">
      {/* ── Sidebar ─────────────────────────────────────────────────── */}
      <aside className="settings-sidebar" onMouseDown={handleWindowDragStart}>
        {isMacPlatform && <div className="settings-sidebar-header" aria-hidden="true" />}
        <div className="settings-sidebar-brand">
          <Logo size={28} />
          <span>Copi</span>
        </div>

        <nav className="settings-sidebar-nav">
          {SECTIONS.map((section) => (
            <button
              key={section.id}
              className={`settings-nav-item ${activeSection === section.id ? "active" : ""}`}
              onClick={() => setActiveSection(section.id)}
            >
              <section.icon size={15} />
              <span>{section.label}</span>
            </button>
          ))}
        </nav>

        <div className="settings-sidebar-footer">
          <button 
            className="settings-wormhole-btn"
            onClick={() => invoke("open_wormhole_window")}
            title="Open Wormhole - Send large files"
          >
            <Shell size={14} />
            <span>Wormhole</span>
          </button>
          <span>{appVersion ? `v${appVersion}` : ""}</span>
        </div>
      </aside>

      {/* ── Content ─────────────────────────────────────────────────── */}
      <main className="settings-content">
        <header className="settings-content-header" onMouseDown={handleWindowDragStart}>
          <h1>{activeSectionData.label}</h1>
        </header>

        <div className="settings-content-body">
          {activeSection === "general" && <GeneralSection config={config} saveConfig={saveConfig} />}
          {activeSection === "appearance" && <AppearanceSection config={config} saveConfig={saveConfig} />}
          {activeSection === "privacy" && <PrivacySection config={config} saveConfig={saveConfig} />}
          {activeSection === "sync" && (
            <SyncSection />
          )}
          {activeSection === "data" && (
            <DataSection
              dbSize={dbSize}
              clipCount={clipCount}
              appVersion={appVersion}
              onClearHistory={() => setConfirmClear(true)}
            />
          )}
          {activeSection === "collections" && (
            <CollectionsSection
              collections={collections}
              onDelete={handleDeleteCollection}
              onRename={handleRenameCollection}
              onCreate={handleCreateCollection}
              onUpdateColor={handleUpdateCollectionColor}
            />
          )}
        </div>

        <footer className="settings-content-footer">
          Press {formatShortcut(config.general.hotkey, " + ")} to open
        </footer>
      </main>

      {/* ── Clear Confirmation Dialog ───────────────────────────────── */}
      {confirmClear && (
        <div className="settings-dialog-overlay" onClick={(e) => { if (e.target === e.currentTarget) { setConfirmClear(false); setClearError(null); } }}>
          <div className="settings-dialog">
            <h3>Clear all history?</h3>
            <p>
              This will permanently delete all {clipCount.toLocaleString()} clips, including pinned items.
              This cannot be undone.
            </p>
            {clearError && (
              <p className="settings-dialog-error">Error: {clearError}</p>
            )}
            <div className="settings-dialog-actions">
              <button onClick={() => { setConfirmClear(false); setClearError(null); }}>Cancel</button>
              <button
                className="danger"
                onClick={handleClearHistory}
                disabled={clearing}
              >
                {clearing ? "Clearing…" : "Delete All"}
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
