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

type SetupState = "initial" | "downloading" | "installing" | "ready" | "error";

function deriveState(status: ModelSetupStatus): SetupState {
  if (status.error) return "error";
  if (status.ready) return "ready";
  if (status.phase === "installing") return "installing";
  if (status.phase === "downloading") return "downloading";
  return "initial";
}

// Inline SVG logo (extracted from icons/copi-logo.svg, simplified for setup)
function Logo({ className }: { className?: string }) {
  return (
    <svg
      className={className}
      width="80"
      height="80"
      viewBox="0 0 512 512"
      xmlns="http://www.w3.org/2000/svg"
    >
      <defs>
        <linearGradient id="setup-bg" x1="0%" y1="0%" x2="100%" y2="100%">
          <stop offset="0%" stopColor="#1a1a1f" />
          <stop offset="100%" stopColor="#111114" />
        </linearGradient>
        <linearGradient id="setup-front" x1="0%" y1="0%" x2="100%" y2="100%">
          <stop offset="0%" stopColor="#ffffff" stopOpacity="1" />
          <stop offset="100%" stopColor="#c8c8d0" stopOpacity="1" />
        </linearGradient>
        <linearGradient id="setup-back" x1="0%" y1="0%" x2="100%" y2="100%">
          <stop offset="0%" stopColor="#ffffff" stopOpacity="0.22" />
          <stop offset="100%" stopColor="#ffffff" stopOpacity="0.10" />
        </linearGradient>
      </defs>
      <g transform="translate(56, 56) scale(0.781)">
        <rect width="512" height="512" rx="112" ry="112" fill="url(#setup-bg)" />
        <rect
          x="190" y="170"
          width="196" height="236"
          rx="28" ry="28"
          fill="url(#setup-back)"
          stroke="rgba(255,255,255,0.12)"
          strokeWidth="1.5"
        />
        <rect
          x="158" y="138"
          width="196" height="236"
          rx="28" ry="28"
          fill="url(#setup-front)"
        />
        <rect x="188" y="186" width="80" height="8" rx="4" fill="#1a1a1f" opacity="0.18" />
        <rect x="188" y="206" width="136" height="8" rx="4" fill="#1a1a1f" opacity="0.12" />
        <rect x="188" y="226" width="112" height="8" rx="4" fill="#1a1a1f" opacity="0.12" />
      </g>
    </svg>
  );
}

// Download icon for initial/downloading states
function DownloadIcon({ className, animated }: { className?: string; animated?: boolean }) {
  return (
    <svg
      className={`${className ?? ""} ${animated ? "setup-icon-pulse" : ""}`}
      width="32"
      height="32"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.5"
      strokeLinecap="round"
      strokeLinejoin="round"
    >
      <path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4" />
      <polyline points="7 10 12 15 17 10" />
      <line x1="12" y1="15" x2="12" y2="3" />
    </svg>
  );
}

// Checkmark icon for ready state
function CheckIcon({ className }: { className?: string }) {
  return (
    <svg
      className={className}
      width="32"
      height="32"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="2"
      strokeLinecap="round"
      strokeLinejoin="round"
    >
      <polyline points="20 6 9 17 4 12" />
    </svg>
  );
}

// Warning icon for error state
function WarningIcon({ className }: { className?: string }) {
  return (
    <svg
      className={className}
      width="32"
      height="32"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.5"
      strokeLinecap="round"
      strokeLinejoin="round"
    >
      <circle cx="12" cy="12" r="10" />
      <line x1="12" y1="8" x2="12" y2="12" />
      <line x1="12" y1="16" x2="12.01" y2="16" />
    </svg>
  );
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

  const state = deriveState(status);
  const canDownload = state === "initial" || state === "error";

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
    if (target.closest("[data-no-drag]")) return;
    void getCurrentWindow().startDragging();
  };

  // Status text based on state
  const statusText = useMemo(() => {
    switch (state) {
      case "initial":
        return "One-time download required";
      case "downloading":
        return "Downloading search model...";
      case "installing":
        return "Almost ready...";
      case "ready":
        return "Ready";
      case "error":
        return "Something went wrong";
    }
  }, [state]);

  // Brief explanation based on state
  const explanation = useMemo(() => {
    switch (state) {
      case "initial":
        return "Copi uses a small AI model (~300 MB) to search your clipboard intelligently.";
      case "downloading":
        return `${Math.round(progress * 100)}%`;
      case "installing":
        return "Preparing model for first use";
      case "ready":
        return `Press ${formatShortcut(hotkey)} to open`;
      case "error":
        return status.error ?? "Download failed";
    }
  }, [state, progress, hotkey, status.error]);

  return (
    <div className="setup-window" onMouseDown={handleWindowDrag}>
      <div className="setup-container">
        {/* Logo */}
        <Logo className="setup-logo" />

        {/* Status card */}
        <div className="setup-card">
          {/* Icon */}
          <div className="setup-status-icon">
            {state === "ready" ? (
              <CheckIcon className="setup-icon setup-icon-success" />
            ) : state === "error" ? (
              <WarningIcon className="setup-icon setup-icon-error" />
            ) : (
              <DownloadIcon
                className="setup-icon"
                animated={state === "downloading" || state === "installing"}
              />
            )}
          </div>

          {/* Status text */}
          <div className="setup-status-text">{statusText}</div>

          {/* Explanation / progress */}
          <div className="setup-explanation">{explanation}</div>

          {/* Progress bar (only during download) */}
          {(state === "downloading" || state === "installing") && (
            <div className="setup-progress">
              <div
                className={`setup-progress-fill ${state === "installing" ? "setup-progress-indeterminate" : ""}`}
                style={state === "downloading" ? { width: `${progress * 100}%` } : undefined}
              />
            </div>
          )}

          {/* Action button */}
          {canDownload && (
            <button
              type="button"
              className="setup-button"
              data-no-drag
              onMouseDown={(e) => e.stopPropagation()}
              onClick={handleDownload}
            >
              {state === "error" ? "Retry" : "Download"}
            </button>
          )}

          {state === "ready" && (
            <button
              type="button"
              className="setup-button setup-button-primary"
              data-no-drag
              onMouseDown={(e) => e.stopPropagation()}
              onClick={handleClose}
            >
              Get Started
            </button>
          )}
        </div>
      </div>
    </div>
  );
}
