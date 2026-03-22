import { useCallback, useEffect, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";

interface UseKeyboardOptions {
  resultCount: number;
  selectedIndex: number;
  onSelect: (index: number) => void;
  onCopy: (index: number) => void;
  onPaste: (index: number) => void;
  onNumberCopy: (resultIndex: number) => void;
  onFilterCycle: () => void;
  onDelete: (index: number) => void;
  onPin: (index: number) => void;
  onActions: (index: number) => void;
}

export function useKeyboard({
  resultCount,
  selectedIndex,
  onSelect,
  onCopy,
  onPaste,
  onNumberCopy,
  onFilterCycle,
  onDelete,
  onPin,
  onActions,
}: UseKeyboardOptions) {
  const handleKeyDown = useCallback(
    (e: KeyboardEvent) => {
      // Ignore Option/Alt key combos — they interfere with typing numbers after Option+Space
      if (e.altKey) return;

      // When the search input is focused, let it handle all keys except
      // navigation/action keys (Escape, arrows, Enter, Tab, Cmd combos)
      const target = e.target as HTMLElement;
      const isInput = target.tagName === 'INPUT' || target.tagName === 'TEXTAREA';
      const navKeys = ['Escape', 'ArrowDown', 'ArrowUp', 'Enter', 'Tab'];
      if (isInput && !navKeys.includes(e.key) && !e.metaKey && !e.ctrlKey) {
        return; // let the input handle it
      }

      // Don't intercept if a modifier key is held (except for specific combos)
      const isMeta = e.metaKey || e.ctrlKey;

      if (e.key === "Escape") {
        e.preventDefault();
        invoke("hide_overlay", { paste: false });
        return;
      }

      if (e.key === "ArrowDown") {
        e.preventDefault();
        if (resultCount > 0) {
          onSelect(Math.min(selectedIndex + 1, resultCount - 1));
        }
        return;
      }

      if (e.key === "ArrowUp") {
        e.preventDefault();
        if (resultCount > 0) {
          onSelect(Math.max(selectedIndex - 1, 0));
        }
        return;
      }

      // Enter - paste (copy + auto-paste into previous app)
      if (e.key === "Enter" && !e.shiftKey && !isMeta) {
        e.preventDefault();
        if (selectedIndex >= 0 && selectedIndex < resultCount) {
          onPaste(selectedIndex);
        }
        return;
      }

      // Shift+Enter - copy to clipboard only
      if (e.key === "Enter" && e.shiftKey) {
        e.preventDefault();
        if (selectedIndex >= 0 && selectedIndex < resultCount) {
          onCopy(selectedIndex);
        }
        return;
      }

      if (e.key === "Tab") {
        e.preventDefault();
        onFilterCycle();
        return;
      }

      // Number keys 1-9 for instant copy
      if (!isMeta && !e.shiftKey && !e.altKey) {
        const num = parseInt(e.key);
        if (num >= 1 && num <= 9 && num <= resultCount) {
          e.preventDefault();
          onNumberCopy(num - 1);
          return;
        }
      }

      // Cmd+K - actions
      if (e.key === "k" && (e.metaKey || e.ctrlKey)) {
        e.preventDefault();
        if (selectedIndex >= 0 && selectedIndex < resultCount) {
          onActions(selectedIndex);
        }
        return;
      }

      // Cmd+D - delete
      if (e.key === "d" && (e.metaKey || e.ctrlKey)) {
        e.preventDefault();
        if (selectedIndex >= 0 && selectedIndex < resultCount) {
          onDelete(selectedIndex);
        }
        return;
      }

      // Cmd+P - pin/unpin
      if (e.key === "p" && (e.metaKey || e.ctrlKey)) {
        e.preventDefault();
        if (selectedIndex >= 0 && selectedIndex < resultCount) {
          onPin(selectedIndex);
        }
        return;
      }
    },
    [resultCount, selectedIndex, onSelect, onCopy, onPaste, onNumberCopy, onFilterCycle, onDelete, onPin, onActions]
  );

  useEffect(() => {
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [handleKeyDown]);
}
