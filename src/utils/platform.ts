const userAgent = navigator.userAgent.toLowerCase();

export const isMacPlatform = /mac|iphone|ipad|ipod/.test(userAgent);
export const isWindowsPlatform = /windows/.test(userAgent);
export const platformName = isMacPlatform ? "Mac" : "PC";

function formatShortcutPart(part: string): string {
  switch (part.trim().toLowerCase()) {
    case "cmd":
    case "command":
    case "super":
      return isMacPlatform ? "⌘" : "Win";
    case "ctrl":
    case "control":
      return isMacPlatform ? "⌃" : "Ctrl";
    case "alt":
    case "option":
      return isMacPlatform ? "⌥" : "Alt";
    case "shift":
      return isMacPlatform ? "⇧" : "Shift";
    case "space":
      return "Space";
    case "enter":
      return isMacPlatform ? "↩" : "Enter";
    default:
      return part.length === 1 ? part.toUpperCase() : part;
  }
}

export function formatShortcut(shortcut: string, separator = isMacPlatform ? "" : " + "): string {
  return shortcut
    .split("+")
    .map((part) => formatShortcutPart(part))
    .join(separator);
}

export function formatSymbolShortcut(shortcut: string): string {
  const parts = shortcut.split("+").map((part) => part.trim().toLowerCase());
  const mapPart = (part: string): string => {
    if (part === "ctrl" || part === "control") return isMacPlatform ? "⌃" : "Ctrl";
    if (part === "cmd" || part === "command" || part === "super") return isMacPlatform ? "⌘" : "Win";
    if (part === "alt" || part === "option") return isMacPlatform ? "⌥" : "Alt";
    if (part === "shift") return isMacPlatform ? "⇧" : "Shift";
    if (part === "enter") return "↵";
    if (part === "space") return "␠";
    return part.length === 1 ? part.toUpperCase() : part;
  };

  if (isMacPlatform) {
    return parts.map(mapPart).join("");
  }

  const mapped = parts.map(mapPart);
  if (mapped.length === 1) {
    return mapped[0];
  }
  const leading = mapped.slice(0, -1).join("+");
  return `${leading} + ${mapped[mapped.length - 1]}`;
}

export function normalizeAppName(name: string): string {
  const trimmed = name.trim();
  if (!trimmed) return "";

  if (!isWindowsPlatform) {
    return trimmed;
  }

  const lower = trimmed.toLowerCase();
  const known: Record<string, string> = {
    windowsterminal: "Windows Terminal",
    msedge: "Microsoft Edge",
    firefox: "Firefox",
    chrome: "Google Chrome",
    discord: "Discord",
    code: "VS Code",
    explorer: "File Explorer",
    devenv: "Visual Studio",
    pwsh: "PowerShell",
  };
  if (known[lower] && /^[a-z0-9_-]+$/.test(trimmed)) {
    return known[lower];
  }

  return trimmed;
}
