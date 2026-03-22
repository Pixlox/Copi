import { useState, useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";

interface CopiConfig {
  general: {
    hotkey: string;
    launch_at_login: boolean;
    default_paste_behaviour: string;
    history_retention_days: number;
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

type Section = "general" | "privacy" | "appearance" | "storage";

function Settings() {
  const [config, setConfig] = useState<CopiConfig | null>(null);
  const [activeSection, setActiveSection] = useState<Section>("general");
  const [dbSize, setDbSize] = useState(0);
  const [exportedJson, setExportedJson] = useState<string | null>(null);

  useEffect(() => {
    loadConfig();
    loadDbSize();
  }, []);

  async function loadConfig() {
    try {
      const c = await invoke<CopiConfig>("get_config");
      setConfig(c);
    } catch (error) {
      console.error("Failed to load config:", error);
    }
  }

  async function loadDbSize() {
    try {
      const size = await invoke<number>("get_db_size");
      setDbSize(size);
    } catch (error) {
      console.error("Failed to get DB size:", error);
    }
  }

  async function saveConfig(updated: CopiConfig) {
    try {
      await invoke("set_config", { config: updated });
      setConfig(updated);
    } catch (error) {
      console.error("Failed to save config:", error);
    }
  }

  async function handleClearHistory() {
    if (confirm("Are you sure you want to delete all clipboard history? This cannot be undone.")) {
      try {
        await invoke("clear_all_history");
        loadDbSize();
      } catch (error) {
        console.error("Failed to clear history:", error);
      }
    }
  }

  async function handleExport() {
    try {
      const json = await invoke<string>("export_history_json");
      setExportedJson(json);
      // Create download
      const blob = new Blob([json], { type: "application/json" });
      const url = URL.createObjectURL(blob);
      const a = document.createElement("a");
      a.href = url;
      a.download = "copi-history.json";
      a.click();
      URL.revokeObjectURL(url);
    } catch (error) {
      console.error("Failed to export:", error);
    }
  }

  if (!config) {
    return (
      <div className="flex items-center justify-center h-full">
        <span className="opacity-40">Loading…</span>
      </div>
    );
  }

  const sections: { id: Section; label: string }[] = [
    { id: "general", label: "General" },
    { id: "privacy", label: "Privacy" },
    { id: "appearance", label: "Appearance" },
    { id: "storage", label: "Storage" },
  ];

  return (
    <div className="flex h-full">
      {/* Sidebar */}
      <div className="w-40 border-r border-black/5 dark:border-white/5 p-3 flex flex-col gap-1">
        <h2 className="text-xs font-semibold opacity-40 mb-2 px-2">Settings</h2>
        {sections.map((section) => (
          <button
            key={section.id}
            onClick={() => setActiveSection(section.id)}
            className={`text-left px-2 py-1.5 rounded-md text-sm transition-colors ${
              activeSection === section.id
                ? "bg-black/8 dark:bg-white/10 font-medium"
                : "hover:bg-black/4 dark:hover:bg-white/5 opacity-70"
            }`}
          >
            {section.label}
          </button>
        ))}
      </div>

      {/* Content */}
      <div className="flex-1 p-6 overflow-y-auto">
        {activeSection === "general" && (
          <div className="space-y-6">
            <h3 className="text-lg font-semibold">General</h3>

            <label className="flex items-center justify-between">
              <span>Launch at login</span>
              <input
                type="checkbox"
                checked={config.general.launch_at_login}
                onChange={(e) =>
                  saveConfig({
                    ...config,
                    general: { ...config.general, launch_at_login: e.target.checked },
                  })
                }
                className="w-4 h-4"
              />
            </label>

            <label className="flex flex-col gap-1">
              <span>Global hotkey</span>
              <input
                type="text"
                value={config.general.hotkey}
                onChange={(e) =>
                  saveConfig({
                    ...config,
                    general: { ...config.general, hotkey: e.target.value },
                  })
                }
                className="px-3 py-1.5 rounded-md border border-black/10 dark:border-white/10 bg-transparent text-sm"
                placeholder="alt+space"
              />
              <span className="text-xs opacity-40">e.g. alt+space, ctrl+shift+c</span>
            </label>

            <label className="flex flex-col gap-1">
              <span>Default paste behaviour</span>
              <select
                value={config.general.default_paste_behaviour}
                onChange={(e) =>
                  saveConfig({
                    ...config,
                    general: { ...config.general, default_paste_behaviour: e.target.value },
                  })
                }
                className="px-3 py-1.5 rounded-md border border-black/10 dark:border-white/10 bg-transparent text-sm"
              >
                <option value="copy">Copy only</option>
                <option value="auto-paste">Auto-paste</option>
              </select>
            </label>

            <label className="flex flex-col gap-1">
              <span>History retention</span>
              <select
                value={String(config.general.history_retention_days)}
                onChange={(e) =>
                  saveConfig({
                    ...config,
                    general: { ...config.general, history_retention_days: parseInt(e.target.value) },
                  })
                }
                className="px-3 py-1.5 rounded-md border border-black/10 dark:border-white/10 bg-transparent text-sm"
              >
                <option value="30">30 days</option>
                <option value="90">90 days</option>
                <option value="365">1 year</option>
                <option value="0">Forever</option>
              </select>
            </label>
          </div>
        )}

        {activeSection === "privacy" && (
          <div className="space-y-6">
            <h3 className="text-lg font-semibold">Privacy</h3>

            <div className="flex flex-col gap-2">
              <span className="text-sm font-medium">Excluded apps</span>
              {config.privacy.excluded_apps.map((app, i) => (
                <div key={i} className="flex items-center gap-2">
                  <span className="text-sm flex-1">{app}</span>
                  <button
                    onClick={() => {
                      const apps = [...config.privacy.excluded_apps];
                      apps.splice(i, 1);
                      saveConfig({
                        ...config,
                        privacy: { ...config.privacy, excluded_apps: apps },
                      });
                    }}
                    className="text-xs opacity-40 hover:opacity-70"
                  >
                    Remove
                  </button>
                </div>
              ))}
              <button
                onClick={() => {
                  const app = prompt("Enter app name to exclude:");
                  if (app) {
                    saveConfig({
                      ...config,
                      privacy: {
                        ...config.privacy,
                        excluded_apps: [...config.privacy.excluded_apps, app],
                      },
                    });
                  }
                }}
                className="text-xs opacity-50 hover:opacity-70 self-start"
              >
                + Add app
              </button>
            </div>

            <div className="flex flex-col gap-2">
              <span className="text-sm font-medium">Privacy regex rules</span>
              <span className="text-xs opacity-40">Clips matching these patterns are auto-deleted</span>
              {config.privacy.privacy_rules.map((rule, i) => (
                <div key={i} className="flex items-center gap-2">
                  <code className="text-xs flex-1 font-mono bg-black/5 dark:bg-white/5 px-2 py-1 rounded">
                    {rule}
                  </code>
                  <button
                    onClick={() => {
                      const rules = [...config.privacy.privacy_rules];
                      rules.splice(i, 1);
                      saveConfig({
                        ...config,
                        privacy: { ...config.privacy, privacy_rules: rules },
                      });
                    }}
                    className="text-xs opacity-40 hover:opacity-70"
                  >
                    Remove
                  </button>
                </div>
              ))}
              <button
                onClick={() => {
                  const rule = prompt("Enter regex pattern:");
                  if (rule) {
                    saveConfig({
                      ...config,
                      privacy: {
                        ...config.privacy,
                        privacy_rules: [...config.privacy.privacy_rules, rule],
                      },
                    });
                  }
                }}
                className="text-xs opacity-50 hover:opacity-70 self-start"
              >
                + Add rule
              </button>
            </div>
          </div>
        )}

        {activeSection === "appearance" && (
          <div className="space-y-6">
            <h3 className="text-lg font-semibold">Appearance</h3>

            <label className="flex flex-col gap-1">
              <span>Theme</span>
              <select
                value={config.appearance.theme}
                onChange={(e) =>
                  saveConfig({
                    ...config,
                    appearance: { ...config.appearance, theme: e.target.value },
                  })
                }
                className="px-3 py-1.5 rounded-md border border-black/10 dark:border-white/10 bg-transparent text-sm"
              >
                <option value="system">System</option>
                <option value="light">Light</option>
                <option value="dark">Dark</option>
              </select>
            </label>

            <label className="flex items-center justify-between">
              <span>Compact mode</span>
              <input
                type="checkbox"
                checked={config.appearance.compact_mode}
                onChange={(e) =>
                  saveConfig({
                    ...config,
                    appearance: { ...config.appearance, compact_mode: e.target.checked },
                  })
                }
                className="w-4 h-4"
              />
            </label>

            <label className="flex items-center justify-between">
              <span>Show app icons</span>
              <input
                type="checkbox"
                checked={config.appearance.show_app_icons}
                onChange={(e) =>
                  saveConfig({
                    ...config,
                    appearance: { ...config.appearance, show_app_icons: e.target.checked },
                  })
                }
                className="w-4 h-4"
              />
            </label>
          </div>
        )}

        {activeSection === "storage" && (
          <div className="space-y-6">
            <h3 className="text-lg font-semibold">Storage</h3>

            <div className="flex flex-col gap-1">
              <span className="text-sm">Database size</span>
              <span className="text-lg font-mono">{formatBytes(dbSize)}</span>
            </div>

            <div className="flex flex-col gap-2">
              <button
                onClick={handleExport}
                className="px-4 py-2 rounded-md bg-black/5 dark:bg-white/10 hover:bg-black/10 dark:hover:bg-white/15 text-sm transition-colors"
              >
                Export history as JSON
              </button>

              <button
                onClick={handleClearHistory}
                className="px-4 py-2 rounded-md bg-red-500/10 text-red-600 dark:text-red-400 hover:bg-red-500/20 text-sm transition-colors"
              >
                Clear all history
              </button>
            </div>
          </div>
        )}
      </div>
    </div>
  );
}

function formatBytes(bytes: number): string {
  if (bytes === 0) return "0 B";
  const k = 1024;
  const sizes = ["B", "KB", "MB", "GB"];
  const i = Math.floor(Math.log(bytes) / Math.log(k));
  return `${parseFloat((bytes / Math.pow(k, i)).toFixed(1))} ${sizes[i]}`;
}

export default Settings;
