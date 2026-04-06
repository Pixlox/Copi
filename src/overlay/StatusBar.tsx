import type { SearchStatus } from "../hooks/useSearch";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { formatShortcut, formatSymbolShortcut, isMacPlatform } from "../utils/platform";
import { useEffect, useRef, useState } from "react";

interface StatusBarProps {
  totalCount: number;
  query: string;
  searchStatus: SearchStatus;
  defaultEnterAction: "copy" | "paste";
  actionsOpen: boolean;
  canOpenActions: boolean;
  onToggleActions: () => void;
}

function formatCount(count: number): string {
  return count.toLocaleString();
}

function detectFilters(query: string): string[] {
  const badges: string[] = [];
  const lower = query.toLowerCase();

  // Temporal
  if (/\b(yesterday|today|last\s+(week|month|hour|day)|\d+\s+days?\s+ago|recently|this\s+(morning|afternoon|evening)|around|tonight|friday|monday|tuesday|wednesday|thursday|saturday|sunday)\b/.test(lower)) {
    badges.push("⏱ time");
  }

  // Source app (from/in/via + word)
  const appMatch = lower.match(/\b(?:from|in|via)\s+([a-z][a-z0-9. ]{1,30})/);
  if (appMatch) {
    badges.push(`📱 ${appMatch[1].trim()}`);
  }

  // Content type
  if (/\b(urls?|links?)\b/.test(lower)) badges.push("🔗 URLs");
  if (/\bcode\b/.test(lower)) badges.push("⌨️ Code");
  if (/\b(images?|photos?)\b/.test(lower)) badges.push("🖼 Images");
  if (/\btext\b/.test(lower)) badges.push("📝 Text");

  return badges;
}

function formatSearchStatus(status: SearchStatus): string | null {
  if (status.phase === "indexing" && status.totalItems > 0) {
    const processed = Math.min(status.totalItems, status.completedItems + status.failedItems);
    if (status.failedItems > 0) {
      return `Indexing ${processed}/${status.totalItems} (${status.failedItems} failed)`;
    }
    return `Indexing ${processed}/${status.totalItems}`;
  }
  if (status.phase === "starting") {
    return "Preparing search...";
  }
  if (status.phase === "unavailable") {
    return "Semantic unavailable";
  }
  if (status.phase === "error") {
    return "Semantic index error";
  }
  if (!status.semanticReady && status.phase === "idle") {
    return "Semantic pending";
  }
  return null;
}

function StatusBar({
  totalCount,
  query,
  searchStatus,
  defaultEnterAction,
  actionsOpen,
  canOpenActions,
  onToggleActions,
}: StatusBarProps) {
  const [syncEnabled, setSyncEnabled] = useState<boolean>(false);
  const [syncBadge, setSyncBadge] = useState<string | null>(null);
  const syncStatusKeyRef = useRef<string | null>(null);
  const syncReadyRef = useRef<boolean>(false);
  const syncBadgeTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => {
    let alive = true;
    const setSyncBadgeForTransition = (enabled: boolean, connectedCount: number) => {
      if (!enabled) {
        setSyncBadge(null);
        if (syncBadgeTimerRef.current) {
          clearTimeout(syncBadgeTimerRef.current);
          syncBadgeTimerRef.current = null;
        }
        return;
      }

      const label = connectedCount > 0 ? `Sync online (${connectedCount})` : "Sync idle";
      setSyncBadge(label);
      if (syncBadgeTimerRef.current) {
        clearTimeout(syncBadgeTimerRef.current);
      }
      syncBadgeTimerRef.current = setTimeout(() => {
        setSyncBadge(null);
        syncBadgeTimerRef.current = null;
      }, 3000);
    };

    const refresh = async () => {
      try {
        const [status, peers] = await Promise.all([
          invoke<{ enabled: boolean; connectedCount: number }>("sync_get_status"),
          invoke<Array<{ device_id: string; display_name: string; online: boolean }>>("sync_list_peers"),
        ]);
        if (!alive) return;
        const enabled = Boolean(status?.enabled);
        const connected =
          peers.length > 0
            ? peers.filter((peer) => peer.online).length
            : Number(status?.connectedCount ?? 0);

        setSyncEnabled(enabled);
        const nextKey = `${enabled ? 1 : 0}:${connected}`;
        const prevKey = syncStatusKeyRef.current;
        syncStatusKeyRef.current = nextKey;
        if (!syncReadyRef.current) {
          syncReadyRef.current = true;
          return;
        }
        if (prevKey !== nextKey) {
          setSyncBadgeForTransition(enabled, connected);
        }
      } catch {
        if (!alive) return;
        const prevKey = syncStatusKeyRef.current;
        syncStatusKeyRef.current = "0:0";
        if (syncReadyRef.current && prevKey !== "0:0") {
          setSyncBadgeForTransition(false, 0);
        }
        syncReadyRef.current = true;
        setSyncEnabled(false);
      }
    };

    void refresh();
    const timer = setInterval(() => void refresh(), 3000);
    const unlistenPaired = listen("sync:paired", () => {
      void refresh();
    });
    const unlistenConnected = listen("sync:connected", () => {
      void refresh();
    });
    const unlistenDisconnected = listen("sync:disconnected", () => {
      void refresh();
    });

    return () => {
      alive = false;
      clearInterval(timer);
      if (syncBadgeTimerRef.current) {
        clearTimeout(syncBadgeTimerRef.current);
        syncBadgeTimerRef.current = null;
      }
      unlistenPaired.then((fn) => fn());
      unlistenConnected.then((fn) => fn());
      unlistenDisconnected.then((fn) => fn());
    };
  }, []);

  const filters = detectFilters(query);
  const statusLabel = formatSearchStatus(searchStatus);
  const primaryLabel = defaultEnterAction === "copy" ? "copy" : "paste";
  const secondaryLabel = defaultEnterAction === "copy" ? "paste" : "copy";
  const actionsShortcut = formatShortcut(isMacPlatform ? "cmd+k" : "ctrl+k");
  const primaryShortcut = formatSymbolShortcut("enter");
  const secondaryShortcut = formatSymbolShortcut("shift+enter");

  return (
    <div
      className="flex min-h-[46px] items-center justify-between px-4 py-2 text-[11px]"
      style={{ borderTop: "1px solid var(--border-default)", color: "var(--text-tertiary)" }}
    >
      <div className="flex items-center gap-2">
        <span>{formatCount(totalCount)} clips</span>
        {statusLabel && (
          <span className="temporal-badge">{statusLabel}</span>
        )}
        {syncEnabled && syncBadge && (
          <span className="temporal-badge">{syncBadge}</span>
        )}
        {filters.map((f) => (
          <span key={f} className="temporal-badge">{f}</span>
        ))}
      </div>
      <div className="flex items-center gap-3" style={{ color: "var(--text-tertiary)" }}>
        <span key={`primary-${defaultEnterAction}`}>{primaryShortcut} {primaryLabel}</span>
        <span key={`secondary-${defaultEnterAction}`}>{secondaryShortcut} {secondaryLabel}</span>
        <button
          type="button"
          data-no-drag
          disabled={!canOpenActions}
          onClick={onToggleActions}
          className="rounded-full border px-2.5 py-1 text-[11px] transition-colors"
          style={
            canOpenActions
              ? actionsOpen
                ? { borderColor: "var(--accent-border)", background: "var(--accent-bg)", color: "var(--text-primary)" }
                : { borderColor: "var(--border-default)", background: "var(--surface-primary)", color: "var(--text-secondary)" }
              : { borderColor: "var(--border-subtle)", background: "var(--surface-secondary)", color: "var(--text-muted)" }
          }
        >
          Actions {actionsShortcut}
        </button>
      </div>
    </div>
  );
}

export default StatusBar;
