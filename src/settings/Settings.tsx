import { useEffect, useState, useCallback, useRef } from "react";
import { getVersion } from "@tauri-apps/api/app";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
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
  RefreshCw,
  FolderOpen,
  Pencil,
  Check,
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
}

interface CollectionInfo {
  id: number;
  name: string;
  color: string;
  clip_count: number;
  created_at: number;
}

type Section = "general" | "appearance" | "privacy" | "data" | "collections";

const SECTIONS: { id: Section; label: string; icon: typeof Keyboard }[] = [
  { id: "general", label: "General", icon: Keyboard },
  { id: "appearance", label: "Appearance", icon: Palette },
  { id: "privacy", label: "Privacy", icon: Shield },
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

      <div className="settings-attribution">Made with {"<3"} by Pixlox</div>
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

  const fetchCollections = useCallback(() => {
    invoke<CollectionInfo[]>("list_collections").then(setCollections).catch(() => {});
  }, []);

  useEffect(() => {
    invoke<CopiConfig>("get_config").then(setConfig).catch(console.error);
    invoke<number>("get_db_size").then(setDbSize).catch(() => {});
    invoke<number>("get_total_clip_count").then(setClipCount).catch(() => {});
    getVersion().then(setAppVersion).catch(() => {});
    fetchCollections();
  }, [fetchCollections]);

  // Listen to collections-changed event for real-time updates
  useEffect(() => {
    const unlisten = listen("collections-changed", () => {
      fetchCollections();
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, [fetchCollections]);

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
      setClipCount(0);
      setDbSize(0);
      setConfirmClear(false);
    } catch (e) {
      const msg = typeof e === "string" ? e : (e instanceof Error ? e.message : "Unknown error");
      setClearError(msg);
      console.error("Clear failed:", e);
    } finally {
      setClearing(false);
    }
  };

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
      <aside className="settings-sidebar" data-tauri-drag-region>
        <div className="settings-sidebar-brand" data-tauri-drag-region>
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
          <span>{appVersion ? `v${appVersion}` : ""}</span>
        </div>
      </aside>

      {/* ── Content ─────────────────────────────────────────────────── */}
      <main className="settings-content">
        <header className="settings-content-header" data-tauri-drag-region>
          <h1>{activeSectionData.label}</h1>
        </header>

        <div className="settings-content-body">
          {activeSection === "general" && <GeneralSection config={config} saveConfig={saveConfig} />}
          {activeSection === "appearance" && <AppearanceSection config={config} saveConfig={saveConfig} />}
          {activeSection === "privacy" && <PrivacySection config={config} saveConfig={saveConfig} />}
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
