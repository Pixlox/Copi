import { useState, useEffect, useCallback, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen, emit } from "@tauri-apps/api/event";

export type ThemePreference = "dark" | "light" | "system";
export type ResolvedTheme = "dark" | "light";

interface FullConfig {
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

function getSystemTheme(): ResolvedTheme {
  if (window.matchMedia && window.matchMedia("(prefers-color-scheme: light)").matches) {
    return "light";
  }
  return "dark";
}

function applyTheme(resolved: ResolvedTheme) {
  const root = document.documentElement;
  if (resolved === "dark") {
    root.classList.add("dark");
    root.classList.remove("light");
  } else {
    root.classList.add("light");
    root.classList.remove("dark");
  }
}

export function useTheme() {
  const [theme, setThemeState] = useState<ThemePreference>("dark");
  const [resolvedTheme, setResolvedTheme] = useState<ResolvedTheme>("dark");
  const configRef = useRef<FullConfig | null>(null);

  // Load config on mount
  useEffect(() => {
    invoke<FullConfig>("get_config")
      .then((config) => {
        configRef.current = config;
        const pref = (config.appearance.theme as ThemePreference) || "dark";
        setThemeState(pref);
        const resolved = pref === "system" ? getSystemTheme() : pref;
        setResolvedTheme(resolved);
        applyTheme(resolved);
      })
      .catch(() => {
        applyTheme("dark");
      });
  }, []);

  // Listen for system theme changes
  useEffect(() => {
    const mq = window.matchMedia("(prefers-color-scheme: dark)");
    const handler = () => {
      if (theme === "system") {
        const resolved = getSystemTheme();
        setResolvedTheme(resolved);
        applyTheme(resolved);
      }
    };
    mq.addEventListener("change", handler);
    return () => mq.removeEventListener("change", handler);
  }, [theme]);

  // Listen for theme-changed events from other windows
  useEffect(() => {
    const unlisten = listen<string>("theme-changed", (event) => {
      const pref = (event.payload as ThemePreference) || "dark";
      setThemeState(pref);
      const resolved = pref === "system" ? getSystemTheme() : pref;
      setResolvedTheme(resolved);
      applyTheme(resolved);
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

  const setTheme = useCallback(async (newTheme: ThemePreference) => {
    setThemeState(newTheme);
    const resolved = newTheme === "system" ? getSystemTheme() : newTheme;
    setResolvedTheme(resolved);
    applyTheme(resolved);

    if (!configRef.current) {
      try {
        configRef.current = await invoke<FullConfig>("get_config");
      } catch {
        return;
      }
    }

    const updated: FullConfig = {
      ...configRef.current,
      appearance: {
        ...configRef.current.appearance,
        theme: newTheme,
      },
    };
    configRef.current = updated;

    try {
      await invoke("set_config", { config: updated });
      // Emit event so other windows update
      await emit("theme-changed", newTheme);
    } catch (e) {
      console.error("Failed to save theme:", e);
    }
  }, []);

  return { theme, resolvedTheme, setTheme };
}
