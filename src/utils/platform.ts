const userAgent = navigator.userAgent.toLowerCase();

export const isMacPlatform = /mac|iphone|ipad|ipod/.test(userAgent);
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
