import { useState, useRef, useEffect, useCallback } from "react";

interface PickerOption {
  label: string;
  value: string;
}

interface PickerProps {
  value: string;
  options: PickerOption[];
  onChange: (value: string) => void;
  disabled?: boolean;
}

export default function Picker({ value, options, onChange, disabled }: PickerProps) {
  const [open, setOpen] = useState(false);
  const [focusIndex, setFocusIndex] = useState(-1);
  const containerRef = useRef<HTMLDivElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);

  const selectedOption = options.find((o) => o.value === value);

  const close = useCallback(() => {
    setOpen(false);
    setFocusIndex(-1);
  }, []);

  const openMenu = useCallback(() => {
    if (disabled) return;
    setOpen(true);
    const idx = options.findIndex((o) => o.value === value);
    setFocusIndex(idx >= 0 ? idx : 0);
  }, [disabled, options, value]);

  // Close on outside click
  useEffect(() => {
    if (!open) return;
    const handler = (e: MouseEvent) => {
      if (containerRef.current && !containerRef.current.contains(e.target as Node)) {
        close();
      }
    };
    document.addEventListener("mousedown", handler);
    return () => document.removeEventListener("mousedown", handler);
  }, [open, close]);

  // Scroll focused option into view
  useEffect(() => {
    if (!open || focusIndex < 0 || !menuRef.current) return;
    const items = menuRef.current.querySelectorAll(".picker-option");
    items[focusIndex]?.scrollIntoView({ block: "nearest" });
  }, [open, focusIndex]);

  useEffect(() => {
    const card = containerRef.current?.closest(".settings-card");
    if (!card) return;
    if (open) {
      card.classList.add("settings-card--picker-open");
    } else {
      card.classList.remove("settings-card--picker-open");
    }
    return () => {
      card.classList.remove("settings-card--picker-open");
    };
  }, [open]);

  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent) => {
      if (!open) {
        if (e.key === "Enter" || e.key === " " || e.key === "ArrowDown") {
          e.preventDefault();
          openMenu();
        }
        return;
      }

      switch (e.key) {
        case "ArrowDown":
          e.preventDefault();
          setFocusIndex((prev) => Math.min(prev + 1, options.length - 1));
          break;
        case "ArrowUp":
          e.preventDefault();
          setFocusIndex((prev) => Math.max(prev - 1, 0));
          break;
        case "Enter":
          e.preventDefault();
          if (focusIndex >= 0 && focusIndex < options.length) {
            onChange(options[focusIndex].value);
            close();
          }
          break;
        case "Escape":
          e.preventDefault();
          close();
          break;
      }
    },
    [open, focusIndex, options, onChange, openMenu, close]
  );

  return (
    <div
      ref={containerRef}
      className="relative inline-block"
      style={{ zIndex: open ? 40 : 1 }}
      onKeyDown={handleKeyDown}
      onBlur={(e) => {
        if (!containerRef.current?.contains(e.relatedTarget as Node)) {
          close();
        }
      }}
    >
      <button
        type="button"
        className="picker-trigger"
        onClick={() => (open ? close() : openMenu())}
        disabled={disabled}
        tabIndex={0}
      >
        {selectedOption?.label ?? value}
      </button>

      {open && (
        <div ref={menuRef} className="picker-menu">
          {options.map((option, i) => (
            <div
              key={option.value}
              className={`picker-option ${option.value === value ? "selected" : ""} ${i === focusIndex ? "" : ""}`}
              style={i === focusIndex ? { background: "var(--accent-solid)", color: "#fff" } : undefined}
              onMouseDown={(e) => {
                e.preventDefault();
                onChange(option.value);
                close();
              }}
              onMouseEnter={() => setFocusIndex(i)}
            >
              {option.value === value && <span className="checkmark">✓</span>}
              {option.label}
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
