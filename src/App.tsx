import { useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { ThemeProvider } from "./contexts/ThemeContext";
import { ErrorBoundary } from "./components/ErrorBoundary";
import Overlay from "./overlay/Overlay";
import Settings from "./settings/Settings";
import Setup from "./setup/Setup";
import Wormhole from "./wormhole/Wormhole";
import { checkForUpdates } from "./utils/updater";
import { isMacPlatform } from "./utils/platform";

function App() {
  const windowLabel = getCurrentWindow().label;
  const isSettings = windowLabel === "settings";
  const isSetup = windowLabel === "setup";
  const isWormhole = windowLabel === "wormhole";

  useEffect(() => {
    if (isSettings) {
      document.documentElement.classList.add("settings-window");
    } else {
      document.documentElement.classList.remove("settings-window");
    }
  }, [isSettings]);

  useEffect(() => {
    if (isWormhole) {
      document.documentElement.classList.add("wormhole-window");
    } else {
      document.documentElement.classList.remove("wormhole-window");
    }
  }, [isWormhole]);

  useEffect(() => {
    const root = document.documentElement;
    if (isMacPlatform) {
      root.classList.add("platform-macos");
      root.classList.remove("platform-windows");
    } else {
      root.classList.add("platform-windows");
      root.classList.remove("platform-macos");
    }
  }, []);

  // Auto-update check on startup (only in overlay/main window)
  useEffect(() => {
    if (isSettings || isSetup || isWormhole) return;

    const timer = setTimeout(async () => {
      try {
        const config = await invoke<{
          general: { auto_check_updates: boolean };
        }>("get_config");
        if (config.general.auto_check_updates) {
          await checkForUpdates("background");
        }
      } catch (e) {
        console.error("[Updater] Check failed:", e);
      }
    }, 3000);

    return () => clearTimeout(timer);
  }, [isSettings, isSetup, isWormhole]);

  if (isSettings) {
    return (
      <ThemeProvider>
        <div className="settings-root w-full min-h-screen">
          <Settings />
        </div>
      </ThemeProvider>
    );
  }

  if (isSetup) {
    return (
      <ThemeProvider>
        <div className="w-full h-screen">
          <Setup />
        </div>
      </ThemeProvider>
    );
  }

  if (isWormhole) {
    return (
      <ThemeProvider>
        <div className="w-full h-screen">
          <Wormhole />
        </div>
      </ThemeProvider>
    );
  }

  return (
    <ThemeProvider>
      <div className="w-full h-screen">
        <ErrorBoundary>
          <Overlay />
        </ErrorBoundary>
      </div>
    </ThemeProvider>
  );
}

export default App;
