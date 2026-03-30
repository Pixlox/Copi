import { type MouseEvent, useEffect, useMemo, useState, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { formatShortcut } from "../utils/platform";
import {
  Particles,
  LogoAssembly,
  TypeWriter,
  ProgressWave,
  SuccessBurst,
} from "./animations";

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

// Animation stages for the cinematic experience
type AnimationStage =
  | "initializing"      // Window fading in, particles starting
  | "logo-assembly"     // Logo pieces sliding together
  | "title-reveal"      // "Welcome to Copi" smooth reveal
  | "tagline-reveal"    // Tagline fading in
  | "ready-to-download" // Waiting for user to click download
  | "downloading"       // Progress bar filling
  | "installing"        // Final processing
  | "success"           // Burst effect + checkmark
  | "ready-to-launch";  // Waiting for user hotkey

// Animated checkmark SVG
function AnimatedCheckmark({ className }: { className?: string }) {
  return (
    <svg className={`launch-checkmark ${className ?? ""}`} viewBox="0 0 52 52">
      <circle
        className="launch-checkmark-circle"
        cx="26"
        cy="26"
        r="24"
      />
      <path
        className="launch-checkmark-check"
        d="M14 27l8 8 16-16"
      />
    </svg>
  );
}

// Download icon with animation states
function DownloadIcon({ animated }: { animated?: boolean }) {
  return (
    <svg
      className={`launch-download-icon ${animated ? "launch-download-icon--active" : ""}`}
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

// Spinning loader icon
function SpinnerIcon() {
  return (
    <svg
      className="launch-spinner"
      width="32"
      height="32"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="2"
      strokeLinecap="round"
    >
      <path d="M21 12a9 9 0 1 1-6.219-8.56" />
    </svg>
  );
}

// Warning icon for errors
function WarningIcon() {
  return (
    <svg
      width="32"
      height="32"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.5"
      strokeLinecap="round"
      strokeLinejoin="round"
      style={{ color: "var(--danger-text)" }}
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
  const [stage, setStage] = useState<AnimationStage>("initializing");
  const [showBurst, setShowBurst] = useState(false);
  const [windowVisibleAnim, setWindowVisibleAnim] = useState(false);
  const [cardVisible, setCardVisible] = useState(false);

  const triggerWindowIntro = useCallback(() => {
    setWindowVisibleAnim(false);
    requestAnimationFrame(() => {
      requestAnimationFrame(() => {
        setWindowVisibleAnim(true);
      });
    });
  }, []);

  // Fetch initial status and config
  useEffect(() => {
    invoke<ModelSetupStatus>("get_model_setup_status")
      .then((s) => {
        setStatus(s);
        // If already ready, skip to launch stage
        if (s.ready) {
          setStage("ready-to-launch");
        }
      })
      .catch((error) => console.error("Failed to load model setup status:", error));

    invoke<{ general?: { hotkey?: string } }>("get_config")
      .then((config) => {
        if (config.general?.hotkey) {
          setHotkey(config.general.hotkey);
        }
      })
      .catch((error) => console.error("Failed to load config:", error));

    const unlisten = listen<ModelSetupStatus>("model-setup-updated", (event) => {
      setStatus(event.payload);
    });

    return () => {
      unlisten.then((dispose) => dispose());
    };
  }, []);

  // Re-trigger intro animation when setup window becomes visible/focused.
  useEffect(() => {
    const retrigger = () => {
      if (document.visibilityState === "visible") {
        triggerWindowIntro();
      }
    };

    triggerWindowIntro();
    document.addEventListener("visibilitychange", retrigger);
    window.addEventListener("focus", retrigger);

    return () => {
      document.removeEventListener("visibilitychange", retrigger);
      window.removeEventListener("focus", retrigger);
    };
  }, [triggerWindowIntro]);

  // Animation sequence orchestration
  useEffect(() => {
    if (stage !== "initializing") return;

    const timers: ReturnType<typeof setTimeout>[] = [];

    // Start logo assembly after particles appear
    timers.push(setTimeout(() => setStage("logo-assembly"), 400));

    return () => timers.forEach(clearTimeout);
  }, [stage]);

  // Handle logo animation complete
  const handleLogoComplete = useCallback(() => {
    setTimeout(() => setStage("title-reveal"), 200);
  }, []);

  // Handle title typewriter complete
  const handleTitleComplete = useCallback(() => {
    setTimeout(() => setStage("tagline-reveal"), 200);
  }, []);

  // Handle tagline animation (now just a delay since it's a fade)
  useEffect(() => {
    if (stage === "tagline-reveal") {
      const timer = setTimeout(() => setStage("ready-to-download"), 600);
      return () => clearTimeout(timer);
    }
  }, [stage]);

  // Watch for status changes to update animation stage
  useEffect(() => {
    if (status.error) {
      // Stay on current stage but show error UI
      return;
    }

    if (status.ready && stage !== "success" && stage !== "ready-to-launch") {
      setStage("success");
      setShowBurst(true);
      // Transition to ready-to-launch after success animation
      setTimeout(() => setStage("ready-to-launch"), 1200);
    } else if (status.phase === "installing" && stage === "downloading") {
      setStage("installing");
    } else if (status.phase === "downloading" && stage === "ready-to-download") {
      setStage("downloading");
    }
  }, [status, stage]);

  const progress = useMemo(() => {
    if (status.ready) return 1;
    if (status.totalFiles <= 0) return 0;
    const fileProgress =
      status.totalBytes > 0 ? Math.min(status.downloadedBytes / status.totalBytes, 1) : 0;
    return Math.min((status.completedFiles + fileProgress) / status.totalFiles, 1);
  }, [status]);

  const handleDownload = async () => {
    try {
      setStage("downloading");
      await invoke("download_required_models");
    } catch (error) {
      console.error("Model download failed:", error);
    }
  };

  const handleWindowDrag = (event: MouseEvent<HTMLDivElement>) => {
    const target = event.target as HTMLElement;
    if (target.closest("[data-no-drag]")) return;
    void getCurrentWindow().startDragging();
  };

  // Determine what to show in the action area
  const showSuccessUI = ["success", "ready-to-launch"].includes(stage);
  const canDownload = stage === "ready-to-download" && !status.error;
  const isDownloading = stage === "downloading" || stage === "installing";

  // Animation visibility states
  const showParticles = stage !== "initializing";
  const showLogo = stage !== "initializing";
  const showTitle = ["title-reveal", "tagline-reveal", "ready-to-download", "downloading", "installing", "success", "ready-to-launch"].includes(stage);
  const showTagline = ["tagline-reveal", "ready-to-download", "downloading", "installing", "success", "ready-to-launch"].includes(stage);
  const showCard = ["ready-to-download", "downloading", "installing", "success", "ready-to-launch"].includes(stage) || status.error;

  // Ensure card transitions in after mount instead of appearing instantly.
  useEffect(() => {
    if (!showCard) {
      setCardVisible(false);
      return;
    }

    setCardVisible(false);
    const timer = setTimeout(() => setCardVisible(true), 40);
    return () => clearTimeout(timer);
  }, [showCard]);

  return (
    <div
      className={`launch-window ${windowVisibleAnim ? "launch-window--visible" : ""}`}
      onMouseDown={handleWindowDrag}
    >
      {/* Particle background */}
      <Particles visible={showParticles} />

      {/* Main content container */}
      <div className="launch-container">
        <div className="launch-stage">
          {/* Branding section */}
          <div className="launch-stage-branding">
            {/* Animated logo */}
            {showLogo && (
              <LogoAssembly
                startDelay={100}
                onComplete={handleLogoComplete}
              />
            )}

            {/* Title with softened text reveal */}
            <h1 className={`launch-title ${showTitle ? "launch-title--visible" : ""}`}>
              {stage === "title-reveal" || stage === "tagline-reveal" ? (
                <TypeWriter
                  text="Welcome to Copi"
                  delay={0}
                  speed={62}
                  onComplete={handleTitleComplete}
                  showCursor={false}
                />
              ) : showTitle ? (
                "Welcome to Copi"
              ) : null}
            </h1>

            {/* Tagline - smooth fade, no typewriter */}
            <p className={`launch-tagline ${showTagline ? "launch-tagline--visible" : ""}`}>
              Your local clipboard copilot
            </p>
          </div>

          {/* Action section - download/success card */}
          <div className="launch-stage-action">
            {showCard && (
              <div
                className={`setup-card launch-card ${cardVisible ? "launch-card--visible" : ""} ${showSuccessUI ? "launch-card--success" : ""}`}
                style={{ position: "relative" }}
              >
                {/* Success burst effect */}
                <SuccessBurst active={showBurst} onComplete={() => setShowBurst(false)} />

                {/* Icon area */}
                <div className={`setup-status-icon ${showSuccessUI ? "setup-status-icon--success" : ""}`}>
                  {status.error ? (
                    <WarningIcon />
                  ) : showSuccessUI ? (
                    <AnimatedCheckmark />
                  ) : isDownloading ? (
                    <SpinnerIcon />
                  ) : (
                    <DownloadIcon animated={canDownload} />
                  )}
                </div>

                {/* Status text */}
                <div className="setup-status-text">
                  {status.error
                    ? "Something went wrong"
                    : showSuccessUI
                      ? "Ready to go!"
                      : isDownloading
                        ? stage === "installing"
                          ? "Almost ready..."
                          : "Downloading AI model..."
                        : "One-time setup required"}
                </div>

                {/* Explanation text */}
                <div className="setup-explanation">
                  {status.error
                    ? status.error
                    : showSuccessUI
                      ? `Press ${formatShortcut(hotkey, " + ")} anytime to summon Copi`
                      : isDownloading
                        ? `${Math.round(progress * 100)}% complete`
                        : "Copi uses a small AI model (~300 MB) to search your clipboard intelligently."}
                </div>

                {/* Progress bar */}
                {isDownloading && (
                  <ProgressWave
                    progress={progress}
                    indeterminate={stage === "installing"}
                  />
                )}

                {/* Action buttons */}
                {canDownload && (
                  <button
                    type="button"
                    className="setup-button launch-button"
                    data-no-drag
                    onMouseDown={(e) => e.stopPropagation()}
                    onClick={handleDownload}
                  >
                    Download Model
                  </button>
                )}

                {status.error && stage !== "downloading" && (
                  <button
                    type="button"
                    className="setup-button launch-button"
                    data-no-drag
                    onMouseDown={(e) => e.stopPropagation()}
                    onClick={handleDownload}
                  >
                    Retry Download
                  </button>
                )}

              </div>
            )}

            {/* Hotkey hint appears after card is visible */}
            {showSuccessUI && (
              <div className={`launch-hotkey ${stage === "ready-to-launch" ? "launch-hotkey--visible" : ""}`}>
                <span>Quick access:</span>
                <kbd>{formatShortcut(hotkey, " + ")}</kbd>
              </div>
            )}
          </div>
        </div>
      </div>
    </div>
  );
}
