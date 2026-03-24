import type { SearchStatus } from "../hooks/useSearch";

interface StatusBarProps {
  totalCount: number;
  query: string;
  searchStatus: SearchStatus;
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
  if (status.phase === "backfill_embeddings" && status.totalItems > 0) {
    return `Semantic ${status.completedItems}/${status.totalItems}`;
  }
  if (!status.semanticReady) {
    return "Semantic warming";
  }
  return null;
}

function StatusBar({ totalCount, query, searchStatus, actionsOpen, canOpenActions, onToggleActions }: StatusBarProps) {
  const filters = detectFilters(query);
  const statusLabel = formatSearchStatus(searchStatus);

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
        {filters.map((f) => (
          <span key={f} className="temporal-badge">{f}</span>
        ))}
      </div>
      <div className="flex items-center gap-3" style={{ color: "var(--text-tertiary)" }}>
        <span>↵ paste</span>
        <span>⇧↵ copy</span>
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
          Actions ⌘K
        </button>
      </div>
    </div>
  );
}

export default StatusBar;
