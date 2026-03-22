interface StatusBarProps {
  totalCount: number;
  query: string;
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

function StatusBar({ totalCount, query }: StatusBarProps) {
  const filters = detectFilters(query);

  return (
    <div className="flex items-center justify-between px-4 py-1.5 border-t border-white/[0.06] text-[11px] text-white/35">
      <div className="flex items-center gap-2">
        <span>{formatCount(totalCount)} clips</span>
        {filters.map((f) => (
          <span key={f} className="temporal-badge">{f}</span>
        ))}
      </div>
      <div className="flex items-center gap-3 text-white/30">
        <span>↵ paste</span>
        <span>⇧↵ copy</span>
      </div>
    </div>
  );
}

export default StatusBar;
