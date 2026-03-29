import { type MouseEvent, useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { formatShortcut } from "../utils/platform";

interface ModelSetupStatus {
  phase: string;
  currentFile: string | null;
  downloadedBytes: number;
  totalBytes: number;
  completedFiles: number;
  totalFiles: number;
  installPath: string;
  error: string | null;
  ready: boolean;
  setupRequired: boolean;
}

const INITIAL_STATUS: ModelSetupStatus = {
  phase: "checking",
  currentFile: null,
  downloadedBytes: 0,
  totalBytes: 0,
  completedFiles: 0,
  totalFiles: 5,
  installPath: "",
  error: null,
  ready: false,
  setupRequired: true,
};

function formatBytes(bytes: number): string {
  if (!bytes) return "0 MB";
  const units = ["B", "KB", "MB", "GB"];
  const power = Math.min(Math.floor(Math.log(bytes) / Math.log(1024)), units.length - 1);
  const value = bytes / Math.pow(1024, power);
  return `${value.toFixed(power === 0 ? 0 : 1)} ${units[power]}`;
}

export default function Setup() {
  const [status, setStatus] = useState<ModelSetupStatus>(INITIAL_STATUS);
  const [hotkey, setHotkey] = useState("alt+space");

  useEffect(() => {
    invoke<ModelSetupStatus>("get_model_setup_status")
      .then(setStatus)
      .catch((error) => console.error("Failed to load model setup status:", error));
    invoke<{ general?: { hotkey?: string } }>("get_config")
      .then((config) => {
        if (config.general?.hotkey) {
          setHotkey(config.general.hotkey);
        }
      })
      .catch((error) => console.error("Failed to load setup hotkey:", error));

    const unlisten = listen<ModelSetupStatus>("model-setup-updated", (event) => {
      setStatus(event.payload);
    });

    return () => {
      unlisten.then((dispose) => dispose());
    };
  }, []);

  const progress = useMemo(() => {
    if (status.ready) return 1;
    if (status.totalFiles <= 0) return 0;
    const fileProgress =
      status.totalBytes > 0 ? Math.min(status.downloadedBytes / status.totalBytes, 1) : 0;
    return Math.min((status.completedFiles + fileProgress) / status.totalFiles, 1);
  }, [status]);

  const canDownload = !["downloading", "installing", "ready"].includes(status.phase);
  const fileLabel = status.currentFile ? status.currentFile.replace(/_/g, " ") : null;

  const handleDownload = async () => {
    try {
      await invoke("download_required_models");
    } catch (error) {
      console.error("Model download failed:", error);
    }
  };

  const handleClose = async () => {
    await invoke("hide_setup_window");
  };

  const handleWindowDrag = (event: MouseEvent<HTMLDivElement>) => {
    const target = event.target as HTMLElement;
    if (target.closest("[data-no-drag]")) {
      return;
    }
    void getCurrentWindow().startDragging();
  };

  return (
    <div className="setup-window">
      <div className="setup-backdrop" />
      <div className="setup-card setup-enter" onMouseDown={handleWindowDrag}>
        <div className="setup-header">
          <span className="setup-eyebrow">Welcome screen</span>
        </div>

        <div className="setup-copy">
          <h1>
            Welcome to <strong>Copi</strong>
          </h1>
          <p>Your new clipboard copilot.</p>
        </div>

        {status.ready ? (
          <div className="setup-content">
            <div className="setup-message">
              <h2>You&apos;re all set.</h2>
              <p>Hit {formatShortcut(hotkey)} to try it out.</p>
            </div>

            <button
              type="button"
              className="setup-button"
              data-no-drag
              onMouseDown={(event) => event.stopPropagation()}
              onClick={handleClose}
            >
              Close
            </button>
          </div>
        ) : (
          <div className="setup-content">
            <div className="setup-message">
              <h2>Copi needs an embeddings model to run semantic search locally.</h2>
              <p>Download our tested multilingual model once and keep it on this device.</p>
            </div>

            <div className="setup-download-block">
              <div className="setup-file-summary">
                <span>Download intfloat/multilingual-e5-small (~300MB)</span>
                {fileLabel ? (
                  <span>
                    {fileLabel}
                    {status.totalBytes > 0
                      ? ` • ${formatBytes(status.downloadedBytes)} of ${formatBytes(status.totalBytes)}`
                      : ""}
                  </span>
                ) : (
                  <span>{status.completedFiles} of {status.totalFiles} files ready</span>
                )}
              </div>

              <button
                type="button"
                className="setup-button"
                data-no-drag
                disabled={!canDownload}
                onMouseDown={(event) => event.stopPropagation()}
                onClick={handleDownload}
              >
                {status.phase === "installing"
                  ? "Installing"
                  : status.phase === "downloading"
                    ? "Downloading"
                    : "Download"}
              </button>

              <div className="setup-progress" aria-hidden="true">
                <div className="setup-progress-fill" style={{ width: `${progress * 100}%` }} />
              </div>

              <div className="setup-meta">
                <span>Stored in app local data</span>
                {status.installPath ? <span>{status.installPath}</span> : null}
              </div>

              {status.error ? <div className="setup-error">{status.error}</div> : null}
            </div>
          </div>
        )}
      </div>
    </div>
  );
}
