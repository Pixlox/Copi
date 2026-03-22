import { useRef, useEffect, useCallback } from "react";
import { Search, X } from "lucide-react";
import { FilterType } from "../hooks/useSearch";

interface SearchBarProps {
  query: string;
  onQueryChange: (query: string) => void;
  activeFilter: FilterType;
  onFilterChange: (filter: FilterType) => void;
}

const FILTER_LABELS: Record<FilterType, string> = {
  all: "All",
  text: "Text",
  url: "URLs",
  code: "Code",
  image: "Images",
  pinned: "Pinned",
};

function SearchBar({ query, onQueryChange, activeFilter, onFilterChange }: SearchBarProps) {
  const inputRef = useRef<HTMLInputElement>(null);

  const focusInput = useCallback(() => {
    inputRef.current?.focus();
  }, []);

  useEffect(() => {
    focusInput();
    const onFocus = () => setTimeout(focusInput, 10);
    window.addEventListener("focus", onFocus);
    return () => window.removeEventListener("focus", onFocus);
  }, [focusInput]);

  const filters: FilterType[] = ["all", "text", "url", "code", "image", "pinned"];

  return (
    <div className="border-b border-white/[0.06]">
      <div className="flex items-center gap-2 px-4 py-3">
        <Search size={16} className="text-white/40 shrink-0" />
        <input
          ref={inputRef}
          type="text"
          value={query}
          onChange={(e) => onQueryChange(e.target.value)}
          placeholder="Search your clipboard…"
          className="flex-1 bg-transparent outline-none text-[14px] text-white/90"
          spellCheck={false}
          autoComplete="off"
        />
        {query.length > 0 && (
          <button
            onClick={() => onQueryChange("")}
            className="p-0.5 rounded-full hover:bg-white/10 text-white/40 hover:text-white/70 transition-colors"
          >
            <X size={14} />
          </button>
        )}
      </div>

      <div className="flex items-center gap-1 px-4 pb-2">
        {filters.map((filter) => (
          <button
            key={filter}
            onClick={() => onFilterChange(filter)}
            className={`filter-pill ${
              activeFilter === filter
                ? "active"
                : "text-white/35 hover:text-white/60"
            }`}
          >
            {FILTER_LABELS[filter]}
          </button>
        ))}
      </div>
    </div>
  );
}

export default SearchBar;
