import { useEffect, useState, useCallback, useRef } from "react";
import { getVersion } from "@tauri-apps/api/app";
import { invoke } from "@tauri-apps/api/core";
import {
  Keyboard,
  Palette,
  Shield,
  HardDrive,
  Sun,
  Moon,
  Monitor,
  Plus,
  X,
  Trash2,
  Download,
  RefreshCw,
  FolderOpen,
} from "lucide-react";
import { useThemeContext } from "../contexts/ThemeContext";
import type { ThemePreference } from "../hooks/useTheme";
import Picker from "../components/Picker";
import { checkForUpdates } from "../utils/updater";

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
    privacy_rules: string[];
  };
}

function Toggle({
  checked,
  onChange,
}: {
  checked: boolean;
  onChange: (v: boolean) => void;
}) {
  return (
    <button
      type="button"
      className={`toggle-track ${checked ? "on" : "off"}`}
      onClick={() => onChange(!checked)}
      aria-checked={checked}
      role="switch"
    >
      <div className="toggle-knob" />
    </button>
  );
}

function SectionHeader({ icon, title }: { icon: React.ReactNode; title: string }) {
  return (
    <div className="flex items-center gap-2 px-4 pt-6 pb-1.5 first:pt-2">
      <span style={{ color: "var(--text-secondary)" }}>{icon}</span>
      <h2
        className="text-[11px] font-semibold uppercase"
        style={{ color: "var(--settings-section-title)", letterSpacing: "0.06em" }}
      >
        {title}
      </h2>
    </div>
  );
}

function Card({ children }: { children: React.ReactNode }) {
  return (
    <div
      className="rounded-[10px] settings-card-shadow"
      style={{ background: "var(--settings-card-bg)" }}
    >
      {children}
    </div>
  );
}

function Row({
  label,
  description,
  children,
}: {
  label: string;
  description?: string;
  children?: React.ReactNode;
}) {
  return (
    <div className="flex items-center gap-4 px-4 py-[11px]">
      <div className="min-w-0 flex-1">
        <div className="text-[13px]" style={{ color: "var(--text-primary)" }}>
          {label}
        </div>
        {description && (
          <div className="mt-0.5 text-[11px]" style={{ color: "var(--text-tertiary)" }}>
            {description}
          </div>
        )}
      </div>
      {children && <div className="shrink-0">{children}</div>}
    </div>
  );
}

function Divider() {
  return (
    <div className="mx-4" style={{ borderTop: "0.5px solid var(--border-subtle)" }} />
  );
}

function SaveNotice({ show }: { show: boolean }) {
  return (
    <span
      className="inline-flex items-center gap-1 px-4 pb-1 text-[11px] transition-opacity duration-200"
      style={{
        color: "var(--success-text)",
        opacity: show ? 1 : 0,
      }}
    >
      Saved
    </span>
  );
}

function formatBytes(bytes: number): string {
  if (bytes === 0) return "0 B";
  const k = 1024;
  const sizes = ["B", "KB", "MB", "GB"];
  const i = Math.floor(Math.log(bytes) / Math.log(k));
  return `${parseFloat((bytes / Math.pow(k, i)).toFixed(1))} ${sizes[i]}`;
}

const HOTKEY_PRESETS = [
  { label: "Alt + Space", value: "alt+space" },
  { label: "Ctrl + Shift + Space", value: "ctrl+shift+space" },
  { label: "Cmd + Shift + Space", value: "cmd+shift+space" },
  { label: "Cmd + Shift + V", value: "cmd+shift+v" },
  { label: "Ctrl + Shift + V", value: "ctrl+shift+v" },
  { label: "Ctrl + Alt + V", value: "ctrl+alt+v" },
];

const RETENTION_OPTIONS = [
  { label: "7 days", value: "7" },
  { label: "30 days", value: "30" },
  { label: "90 days", value: "90" },
  { label: "1 year", value: "365" },
  { label: "Forever", value: "0" },
];

export default function Settings() {
  const { theme, setTheme } = useThemeContext();
  const [config, setConfig] = useState<CopiConfig | null>(null);
  const [dbSize, setDbSize] = useState(0);
  const [clipCount, setClipCount] = useState(0);
  const [appVersion, setAppVersion] = useState<string | null>(null);

  const [savedField, setSavedField] = useState<string | null>(null);
  const saveTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  const [recordingHotkey, setRecordingHotkey] = useState(false);
  const [newApp, setNewApp] = useState("");
  const [newRule, setNewRule] = useState("");
  const [confirmClear, setConfirmClear] = useState(false);
  const [checkingUpdate, setCheckingUpdate] = useState(false);

  // Collections
  interface CollectionInfo {
    id: number;
    name: string;
    color: string;
    clip_count: number;
    created_at: number;
  }
  const [collections, setCollections] = useState<CollectionInfo[]>([]);

  const fetchCollections = useCallback(() => {
    invoke<CollectionInfo[]>("list_collections")
      .then(setCollections)
      .catch(() => {});
  }, []);

  useEffect(() => {
    invoke<CopiConfig>("get_config")
      .then(setConfig)
      .catch((e) => console.error("Config load failed:", e));
    invoke<number>("get_db_size").then(setDbSize).catch(() => {});
    invoke<number>("get_total_clip_count").then(setClipCount).catch(() => {});
    getVersion().then(setAppVersion).catch(() => {});
    fetchCollections();
  }, [fetchCollections]);

  const showSaved = useCallback((field: string) => {
    if (saveTimer.current) clearTimeout(saveTimer.current);
    setSavedField(field);
    saveTimer.current = setTimeout(() => setSavedField(null), 1500);
  }, []);

  const saveConfig = useCallback(
    async (updated: CopiConfig, field: string) => {
      setConfig(updated);
      try {
        await invoke("set_config", { config: updated });
        showSaved(field);
      } catch (e) {
        console.error("Save failed:", e);
      }
    },
    [showSaved]
  );

  // Hotkey recording
  useEffect(() => {
    if (!recordingHotkey) return;

    const handleKeyDown = (e: KeyboardEvent) => {
      e.preventDefault();
      e.stopPropagation();
      if (e.key === "Escape") {
        setRecordingHotkey(false);
      }
    };

    const handleKeyUp = (e: KeyboardEvent) => {
      e.preventDefault();
      e.stopPropagation();

      const mods: string[] = [];
      if (e.metaKey) mods.push("cmd");
      if (e.ctrlKey) mods.push("ctrl");
      if (e.altKey) mods.push("alt");
      if (e.shiftKey) mods.push("shift");

      const key = e.key.toLowerCase();
      if (["meta", "control", "alt", "shift"].includes(key)) return;

      const combo = [...mods, key].join("+");
      setRecordingHotkey(false);

      if (config) {
        const updated: CopiConfig = {
          ...config,
          general: { ...config.general, hotkey: combo },
        };
        saveConfig(updated, "hotkey");
      }
    };

    window.addEventListener("keydown", handleKeyDown, true);
    window.addEventListener("keyup", handleKeyUp, true);
    return () => {
      window.removeEventListener("keydown", handleKeyDown, true);
      window.removeEventListener("keyup", handleKeyUp, true);
    };
  }, [recordingHotkey, config, saveConfig]);

  if (!config) {
    return (
      <div
        className="flex h-screen items-center justify-center"
        style={{ background: "var(--settings-bg)", color: "var(--text-tertiary)" }}
      >
        Loading…
      </div>
    );
  }

  return (
    <div style={{ background: "var(--settings-bg)", color: "var(--text-primary)" }}>
      <div className="mx-auto max-w-[540px] px-4 py-4 pb-10">
        {/* ── Header (draggable) ─────────────────────────────────── */}
        <div
          className="mb-3 rounded-[10px] px-4 py-3 settings-card-shadow"
          style={{
            background: "var(--settings-card-bg)",
            WebkitAppRegion: "drag",
          } as React.CSSProperties}
        >
          <div className="flex items-center justify-between">
            <div>
              <h1 className="text-[15px] font-semibold" style={{ color: "var(--text-primary)" }}>
                Copi
              </h1>
              <p className="text-[11px]" style={{ color: "var(--text-tertiary)" }}>
                Your copying copilot
              </p>
            </div>
            <div className="text-right text-[11px]" style={{ color: "var(--text-tertiary)" }}>
              <div>{formatBytes(dbSize)} stored</div>
              <div>{clipCount.toLocaleString()} clips</div>
            </div>
          </div>
        </div>

        {/* ── General ─────────────────────────────────────────────── */}
        <SectionHeader icon={<Keyboard size={14} />} title="General" />
        <Card>
          <Row label="Global Hotkey" description="Press to open the clipboard overlay">
            {recordingHotkey ? (
              <span
                className="inline-flex items-center rounded-full px-3 py-1 text-[12px]"
                style={{
                  background: "var(--accent-bg)",
                  color: "var(--accent-text)",
                }}
              >
                Recording…
              </span>
            ) : (
              <div className="flex items-center gap-2">
                <span className="text-[12px]" style={{ color: "var(--text-secondary)" }}>
                  {config.general.hotkey}
                </span>
                <button
                  type="button"
                  onClick={() => setRecordingHotkey(true)}
                  className="settings-button"
                >
                  Change
                </button>
              </div>
            )}
          </Row>

          <Divider />

          <Row label="Quick Presets">
            <Picker
              value={config.general.hotkey}
              options={HOTKEY_PRESETS}
              onChange={(val) => {
                const updated: CopiConfig = {
                  ...config,
                  general: { ...config.general, hotkey: val },
                };
                saveConfig(updated, "hotkey");
              }}
            />
          </Row>

          <Divider />

          <Row label="Launch at Login" description="Start Copi when your Mac starts">
            <Toggle
              checked={config.general.launch_at_login}
              onChange={(v) => {
                const updated: CopiConfig = {
                  ...config,
                  general: { ...config.general, launch_at_login: v },
                };
                saveConfig(updated, "launch");
              }}
            />
          </Row>

          <Divider />

          <Row label="Default Paste" description="Enter key behavior">
            <div className="segmented-control">
              <button
                className={`segmented-option ${config.general.default_paste_behaviour === "copy" ? "active" : ""}`}
                onClick={() => {
                  const updated: CopiConfig = {
                    ...config,
                    general: { ...config.general, default_paste_behaviour: "copy" },
                  };
                  saveConfig(updated, "paste");
                }}
              >
                Copy
              </button>
              <button
                className={`segmented-option ${config.general.default_paste_behaviour === "paste" ? "active" : ""}`}
                onClick={() => {
                  const updated: CopiConfig = {
                    ...config,
                    general: { ...config.general, default_paste_behaviour: "paste" },
                  };
                  saveConfig(updated, "paste");
                }}
              >
                Paste
              </button>
            </div>
          </Row>

          <Divider />

          <Row label="History Retention" description="Auto-delete clips older than this">
            <Picker
              value={String(config.general.history_retention_days)}
              options={RETENTION_OPTIONS}
              onChange={(val) => {
                const updated: CopiConfig = {
                  ...config,
                  general: { ...config.general, history_retention_days: parseInt(val) },
                };
                saveConfig(updated, "retention");
              }}
            />
          </Row>

          <Divider />

          <Row label="Auto-check for Updates" description="Check for updates on startup">
            <Toggle
              checked={config.general.auto_check_updates}
              onChange={(v) => {
                const updated: CopiConfig = {
                  ...config,
                  general: { ...config.general, auto_check_updates: v },
                };
                saveConfig(updated, "updates");
              }}
            />
          </Row>

          <SaveNotice show={savedField === "hotkey" || savedField === "launch" || savedField === "paste" || savedField === "retention" || savedField === "updates"} />
        </Card>

        {/* ── Appearance ──────────────────────────────────────────── */}
        <SectionHeader icon={<Palette size={14} />} title="Appearance" />
        <Card>
          <Row label="Theme" description="Choose your color scheme">
            <div className="segmented-control">
              <button
                className={`segmented-option flex items-center gap-1.5 ${theme === "dark" ? "active" : ""}`}
                onClick={() => setTheme("dark")}
              >
                <Moon size={12} />
                Dark
              </button>
              <button
                className={`segmented-option flex items-center gap-1.5 ${theme === "light" ? "active" : ""}`}
                onClick={() => setTheme("light")}
              >
                <Sun size={12} />
                Light
              </button>
              <button
                className={`segmented-option flex items-center gap-1.5 ${theme === "system" ? "active" : ""}`}
                onClick={() => setTheme("system")}
              >
                <Monitor size={12} />
                Auto
              </button>
            </div>
          </Row>

          <Divider />

          <Row label="Compact Mode" description="Smaller rows in the overlay">
            <Toggle
              checked={config.appearance.compact_mode}
              onChange={(v) => {
                const updated: CopiConfig = {
                  ...config,
                  appearance: { ...config.appearance, compact_mode: v },
                };
                saveConfig(updated, "compact");
              }}
            />
          </Row>

          <Divider />

          <Row label="Show App Icons" description="Source app icons next to clips">
            <Toggle
              checked={config.appearance.show_app_icons}
              onChange={(v) => {
                const updated: CopiConfig = {
                  ...config,
                  appearance: { ...config.appearance, show_app_icons: v },
                };
                saveConfig(updated, "icons");
              }}
            />
          </Row>

          <SaveNotice show={savedField === "compact" || savedField === "icons"} />
        </Card>

        {/* ── Privacy ─────────────────────────────────────────────── */}
        <SectionHeader icon={<Shield size={14} />} title="Privacy" />
        <Card>
          <Row label="Excluded Apps" description="Content from these apps won't be captured" />

          <div className="px-4 pb-2 flex flex-wrap gap-1.5">
            {config.privacy.excluded_apps.length === 0 && (
              <span className="text-[11px]" style={{ color: "var(--text-tertiary)" }}>
                No excluded apps
              </span>
            )}
            {config.privacy.excluded_apps.map((app, i) => (
              <span
                key={`${app}-${i}`}
                className="inline-flex items-center gap-1.5 rounded-full px-2.5 py-1 text-[11px]"
                style={{
                  background: "var(--surface-primary)",
                  color: "var(--text-secondary)",
                }}
              >
                {app}
                <button
                  type="button"
                  onClick={() => {
                    const apps = [...config.privacy.excluded_apps];
                    apps.splice(i, 1);
                    const updated: CopiConfig = {
                      ...config,
                      privacy: { ...config.privacy, excluded_apps: apps },
                    };
                    saveConfig(updated, "excluded");
                  }}
                  style={{ color: "var(--text-tertiary)" }}
                >
                  <X size={10} />
                </button>
              </span>
            ))}
          </div>

          <div className="px-4 pb-3 flex items-center gap-2">
            <input
              type="text"
              value={newApp}
              onChange={(e) => setNewApp(e.target.value)}
              placeholder="App name or bundle ID…"
              className="settings-input flex-1"
              onKeyDown={(e) => {
                if (e.key === "Enter" && newApp.trim()) {
                  const updated: CopiConfig = {
                    ...config,
                    privacy: {
                      ...config.privacy,
                      excluded_apps: [...config.privacy.excluded_apps, newApp.trim()],
                    },
                  };
                  saveConfig(updated, "excluded");
                  setNewApp("");
                }
              }}
            />
            <button
              type="button"
              disabled={!newApp.trim()}
              onClick={() => {
                if (!newApp.trim()) return;
                const updated: CopiConfig = {
                  ...config,
                  privacy: {
                    ...config.privacy,
                    excluded_apps: [...config.privacy.excluded_apps, newApp.trim()],
                  },
                };
                saveConfig(updated, "excluded");
                setNewApp("");
              }}
              className="settings-button"
            >
              <Plus size={12} />
            </button>
          </div>

          <Divider />

          <Row label="Privacy Rules" description="Regex patterns — matching content won't be captured" />

          <div className="px-4 pb-2 space-y-1.5">
            {config.privacy.privacy_rules.map((rule, i) => (
              <div
                key={`${rule}-${i}`}
                className="flex items-center gap-2 rounded-lg px-3 py-2"
                style={{ background: "var(--surface-primary)" }}
              >
                <code
                  className="flex-1 text-[11px]"
                  style={{
                    color: "var(--accent-text)",
                    fontFamily: "'SF Mono', monospace",
                  }}
                >
                  {rule}
                </code>
                <button
                  type="button"
                  onClick={() => {
                    const rules = [...config.privacy.privacy_rules];
                    rules.splice(i, 1);
                    const updated: CopiConfig = {
                      ...config,
                      privacy: { ...config.privacy, privacy_rules: rules },
                    };
                    saveConfig(updated, "rules");
                  }}
                  style={{ color: "var(--text-tertiary)" }}
                >
                  <Trash2 size={12} />
                </button>
              </div>
            ))}
          </div>

          <div className="px-4 pb-3 flex items-center gap-2">
            <input
              type="text"
              value={newRule}
              onChange={(e) => setNewRule(e.target.value)}
              placeholder="Regex pattern…"
              className="settings-input flex-1"
              style={{ fontFamily: "'SF Mono', monospace" }}
              onKeyDown={(e) => {
                if (e.key === "Enter" && newRule.trim()) {
                  const updated: CopiConfig = {
                    ...config,
                    privacy: {
                      ...config.privacy,
                      privacy_rules: [...config.privacy.privacy_rules, newRule.trim()],
                    },
                  };
                  saveConfig(updated, "rules");
                  setNewRule("");
                }
              }}
            />
            <button
              type="button"
              disabled={!newRule.trim()}
              onClick={() => {
                if (!newRule.trim()) return;
                const updated: CopiConfig = {
                  ...config,
                  privacy: {
                    ...config.privacy,
                    privacy_rules: [...config.privacy.privacy_rules, newRule.trim()],
                  },
                };
                saveConfig(updated, "rules");
                setNewRule("");
              }}
              className="settings-button"
            >
              <Plus size={12} />
            </button>
          </div>

          <SaveNotice show={savedField === "excluded" || savedField === "rules"} />
        </Card>

        {/* ── Storage & Data ──────────────────────────────────────── */}
        <SectionHeader icon={<HardDrive size={14} />} title="Storage & Data" />
        <Card>
          <Row label="Database Size" description="Current storage used">
            <span className="text-[12px]" style={{ color: "var(--text-secondary)" }}>
              {formatBytes(dbSize)}
            </span>
          </Row>

          <Divider />

          <Row label="Total Clips">
            <span className="text-[12px]" style={{ color: "var(--text-secondary)" }}>
              {clipCount.toLocaleString()}
            </span>
          </Row>

          <Divider />

          <Row label="Current Version">
            <span className="text-[12px]" style={{ color: "var(--text-secondary)" }}>
              {appVersion ? `v${appVersion}` : "Unknown"}
            </span>
          </Row>

          <Divider />

          <Row label="Check for Updates">
            <button
              type="button"
              disabled={checkingUpdate}
              onClick={async () => {
                setCheckingUpdate(true);
                try {
                  await checkForUpdates("interactive");
                } finally {
                  setCheckingUpdate(false);
                }
              }}
              className="settings-button"
            >
              <RefreshCw size={12} className={checkingUpdate ? "animate-spin" : ""} />
              {checkingUpdate ? "Checking…" : "Check Now"}
            </button>
          </Row>

          <Divider />

          <Row label="Export History" description="Download all clips as JSON">
            <button
              type="button"
              onClick={async () => {
                try {
                  const json = await invoke<string>("export_history_json");
                  const blob = new Blob([json], { type: "application/json" });
                  const url = URL.createObjectURL(blob);
                  const a = document.createElement("a");
                  a.href = url;
                  a.download = `copi-export-${new Date().toISOString().slice(0, 10)}.json`;
                  a.click();
                  URL.revokeObjectURL(url);
                } catch (e) {
                  console.error("Export failed:", e);
                }
              }}
              className="settings-button"
            >
              <Download size={12} />
              Export
            </button>
          </Row>

          <Divider />

          <Row label="Clear All History" description="Permanently delete all clipboard data">
            <button
              type="button"
              onClick={() => setConfirmClear(true)}
              className="settings-button destructive"
            >
              <Trash2 size={12} />
              Clear
            </button>
          </Row>
        </Card>

        {/* ── Collections ───────────────────────────────────────────── */}
        <SectionHeader icon={<FolderOpen size={14} />} title="Collections" />
        <Card>
          {collections.length === 0 ? (
            <Row label="No collections yet" description="Create collections from the overlay sidebar">
              <span />
            </Row>
          ) : (
            collections.map((col, i) => (
              <div key={col.id}>
                {i > 0 && <Divider />}
                <Row label={col.name} description={`${col.clip_count} clip${col.clip_count !== 1 ? "s" : ""}`}>
                  <div className="flex items-center gap-1">
                    <div
                      className="w-3 h-3 rounded-full"
                      style={{ background: col.color }}
                    />
                    <button
                      type="button"
                      onClick={async () => {
                        try {
                          await invoke("delete_collection", { id: col.id });
                          fetchCollections();
                        } catch (e) {
                          console.error("Delete collection failed:", e);
                        }
                      }}
                      className="rounded p-1 transition-colors"
                      style={{ color: "var(--text-tertiary)" }}
                    >
                      <Trash2 size={11} />
                    </button>
                  </div>
                </Row>
              </div>
            ))
          )}
        </Card>

        {/* Footer */}
        <div className="mt-6 mb-4 text-center text-[11px]" style={{ color: "var(--text-muted)" }}>
          ⌘ + {config.general.hotkey} to open overlay
        </div>
      </div>

      {/* ── Clear History Confirmation ──────────────────────────── */}
      {confirmClear && (
        <div className="confirm-overlay">
          <div className="confirm-dialog">
            <h3 className="mb-1 text-[13px] font-semibold" style={{ color: "var(--text-primary)" }}>
              Clear all history?
            </h3>
            <p className="mb-4 text-[11px] leading-relaxed" style={{ color: "var(--text-secondary)" }}>
              This will permanently delete all {clipCount.toLocaleString()} clips, including pinned items. This cannot be undone.
            </p>
            <div className="flex items-center justify-end gap-2">
              <button
                type="button"
                onClick={() => setConfirmClear(false)}
                className="settings-button"
              >
                Cancel
              </button>
              <button
                type="button"
                onClick={async () => {
                  try {
                    await invoke("clear_all_history");
                    setClipCount(0);
                    setDbSize(0);
                    setConfirmClear(false);
                    showSaved("clear");
                  } catch (e) {
                    console.error("Clear failed:", e);
                  }
                }}
                className="settings-button destructive"
              >
                Delete All
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
