import { useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { ThemeProvider } from "./contexts/ThemeContext";
import Overlay from "./overlay/Overlay";
import Settings from "./settings/Settings";
import { checkForUpdates } from "./utils/updater";

function App() {
  const isSettings = getCurrentWindow().label === "settings";

  useEffect(() => {
    if (isSettings) {
      document.documentElement.classList.add("settings-window");
    }
  }, [isSettings]);

  // Auto-update check on startup (only in overlay/main window)
  useEffect(() => {
    if (isSettings) return;

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
  }, [isSettings]);

  if (isSettings) {
    return (
      <ThemeProvider>
        <div className="settings-root w-full min-h-screen">
          <Settings />
        </div>
      </ThemeProvider>
    );
  }

  return (
    <ThemeProvider>
      <div className="w-full h-screen">
        <Overlay />
      </div>
    </ThemeProvider>
  );
}

export default App;
