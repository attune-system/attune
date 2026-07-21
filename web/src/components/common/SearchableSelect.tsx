import { useState, useRef, useEffect, useCallback, memo } from "react";
import { ChevronDown, X } from "lucide-react";

export interface SearchableSelectOption {
  value: string | number;
  label: string;
}

interface SearchableSelectProps {
  /** Optional HTML id for the wrapper (useful for label htmlFor association) */
  id?: string;
  /** Accessible name for the combobox */
  ariaLabel?: string;
  /** The available options to choose from */
  options: SearchableSelectOption[];
  /** Currently selected value – should match one of the option values, or an "empty" sentinel (0, "") */
  value: string | number;
  /** Called when the user picks an option */
  onChange: (value: string | number) => void;
  /** Placeholder shown when nothing is selected */
  placeholder?: string;
  /** Disables the control (read-only grey appearance) */
  disabled?: boolean;
  /** Shows a red error border */
  error?: boolean;
  /** Additional CSS classes applied to the outermost wrapper */
  className?: string;
}

/**
 * A single-value select control with a built-in text search filter.
 *
 * Drop-in replacement for `<select>` – supports keyboard navigation,
 * click-outside-to-close, disabled & error styling, and a clear button.
 */
const SearchableSelect = memo(function SearchableSelect({
  id,
  ariaLabel,
  options,
  value,
  onChange,
  placeholder = "Select...",
  disabled = false,
  error = false,
  className = "",
}: SearchableSelectProps) {
  const [isOpen, setIsOpen] = useState(false);
  const [searchQuery, setSearchQuery] = useState("");
  const [highlightedIndex, setHighlightedIndex] = useState(-1);

  const containerRef = useRef<HTMLDivElement>(null);
  const searchInputRef = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLUListElement>(null);

  // ----- derived data -----

  const selectedOption = options.find((o) => o.value === value) ?? null;

  const filtered = searchQuery
    ? options.filter((o) =>
        o.label.toLowerCase().includes(searchQuery.toLowerCase()),
      )
    : options;

  // Clamp highlight when the filtered list shrinks
  const safeIndex = highlightedIndex >= filtered.length ? -1 : highlightedIndex;

  // ----- side-effects -----

  // Close on outside click
  useEffect(() => {
    const handler = (e: MouseEvent) => {
      if (
        containerRef.current &&
        !containerRef.current.contains(e.target as Node)
      ) {
        setIsOpen(false);
        setSearchQuery("");
        setHighlightedIndex(-1);
      }
    };
    document.addEventListener("mousedown", handler);
    return () => document.removeEventListener("mousedown", handler);
  }, []);

  // Auto-focus the search input when dropdown opens
  useEffect(() => {
    if (isOpen && searchInputRef.current) {
      searchInputRef.current.focus();
    }
  }, [isOpen]);

  // Scroll highlighted item into view
  useEffect(() => {
    if (safeIndex >= 0 && listRef.current) {
      const items = listRef.current.children;
      if (items[safeIndex]) {
        (items[safeIndex] as HTMLElement).scrollIntoView({ block: "nearest" });
      }
    }
  }, [safeIndex]);

  // ----- handlers -----

  const openDropdown = useCallback(() => {
    if (disabled) return;
    setIsOpen(true);
    setSearchQuery("");
    setHighlightedIndex(-1);
  }, [disabled]);

  const closeDropdown = useCallback(() => {
    setIsOpen(false);
    setSearchQuery("");
    setHighlightedIndex(-1);
  }, []);

  const selectOption = useCallback(
    (opt: SearchableSelectOption) => {
      onChange(opt.value);
      closeDropdown();
    },
    [onChange, closeDropdown],
  );

  const handleClear = useCallback(
    (e: React.MouseEvent) => {
      e.stopPropagation();
      // Reset to the "empty" sentinel that matches the value's type
      const empty: string | number = typeof value === "number" ? 0 : "";
      onChange(empty);
      closeDropdown();
    },
    [onChange, value, closeDropdown],
  );

  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent) => {
      if (!isOpen) {
        if (e.key === "ArrowDown" || e.key === "Enter" || e.key === " ") {
          e.preventDefault();
          openDropdown();
        }
        return;
      }

      switch (e.key) {
        case "ArrowDown":
          e.preventDefault();
          setHighlightedIndex((prev) =>
            prev < filtered.length - 1 ? prev + 1 : 0,
          );
          break;
        case "ArrowUp":
          e.preventDefault();
          setHighlightedIndex((prev) =>
            prev > 0 ? prev - 1 : filtered.length - 1,
          );
          break;
        case "Enter":
          e.preventDefault();
          if (safeIndex >= 0 && safeIndex < filtered.length) {
            selectOption(filtered[safeIndex]);
          }
          break;
        case "Escape":
          e.preventDefault();
          closeDropdown();
          break;
        case "Tab":
          closeDropdown();
          break;
      }
    },
    [isOpen, filtered, safeIndex, openDropdown, closeDropdown, selectOption],
  );

  // ----- styles -----

  const borderColor = error
    ? "border-red-500"
    : isOpen
      ? "border-blue-500 ring-2 ring-blue-500"
      : "border-gray-300 hover:border-gray-400";

  const disabledStyles = disabled
    ? "bg-gray-100 cursor-not-allowed"
    : "bg-white cursor-pointer";

  return (
    <div
      id={id}
      ref={containerRef}
      className={`relative ${className}`}
      onKeyDown={handleKeyDown}
    >
      {/* Trigger button (shows selected value or placeholder) */}
      <div
        onClick={() => (isOpen ? closeDropdown() : openDropdown())}
        tabIndex={disabled ? -1 : 0}
        role="combobox"
        aria-label={ariaLabel}
        aria-expanded={isOpen}
        aria-haspopup="listbox"
        className={`flex items-center justify-between w-full px-3 py-2 border rounded-lg text-sm focus:outline-none ${borderColor} ${disabledStyles}`}
      >
        <span
          className={
            selectedOption ? "text-gray-900 truncate" : "text-gray-400 truncate"
          }
        >
          {selectedOption ? selectedOption.label : placeholder}
        </span>

        <div className="flex items-center gap-1 flex-shrink-0 ml-2">
          {selectedOption && !disabled && (
            <button
              type="button"
              onClick={handleClear}
              tabIndex={-1}
              className="text-gray-400 hover:text-gray-600 p-0.5 rounded"
              aria-label="Clear selection"
            >
              <X className="h-3.5 w-3.5" />
            </button>
          )}
          <ChevronDown
            className={`h-4 w-4 text-gray-400 transition-transform ${
              isOpen ? "rotate-180" : ""
            }`}
          />
        </div>
      </div>

      {/* Dropdown */}
      {isOpen && (
        <div className="absolute z-50 mt-1 w-full bg-white border border-gray-300 rounded-lg shadow-lg overflow-hidden">
          {/* Search input */}
          <div className="p-2 border-b border-gray-200">
            <input
              ref={searchInputRef}
              type="text"
              value={searchQuery}
              onChange={(e) => {
                setSearchQuery(e.target.value);
                setHighlightedIndex(0);
              }}
              placeholder="Type to search..."
              autoComplete="off"
              className="w-full px-3 py-1.5 border border-gray-300 rounded text-sm focus:outline-none focus:ring-2 focus:ring-blue-500 focus:border-blue-500"
              onClick={(e) => e.stopPropagation()}
            />
          </div>

          {/* Options list */}
          <ul ref={listRef} role="listbox" className="max-h-56 overflow-y-auto">
            {filtered.length === 0 ? (
              <li className="px-3 py-2 text-sm text-gray-500 text-center">
                No options found
              </li>
            ) : (
              filtered.map((option, index) => {
                const isHighlighted = index === safeIndex;
                const isSelected = option.value === value;

                // Highlight matching substring in the label
                const matchIndex = searchQuery
                  ? option.label
                      .toLowerCase()
                      .indexOf(searchQuery.toLowerCase())
                  : -1;

                return (
                  <li
                    key={String(option.value)}
                    role="option"
                    aria-selected={isSelected}
                    onMouseDown={(e) => {
                      e.preventDefault();
                      selectOption(option);
                    }}
                    onMouseEnter={() => setHighlightedIndex(index)}
                    className={`px-3 py-2 text-sm cursor-pointer flex items-center justify-between ${
                      isHighlighted ? "bg-blue-50" : ""
                    } ${isSelected ? "font-medium text-blue-900" : "text-gray-900"}`}
                  >
                    <span className="truncate">
                      {searchQuery && matchIndex >= 0 ? (
                        <>
                          {option.label.slice(0, matchIndex)}
                          <span className="font-semibold">
                            {option.label.slice(
                              matchIndex,
                              matchIndex + searchQuery.length,
                            )}
                          </span>
                          {option.label.slice(matchIndex + searchQuery.length)}
                        </>
                      ) : (
                        option.label
                      )}
                    </span>
                    {isSelected && (
                      <span className="text-blue-600 text-xs flex-shrink-0 ml-2">
                        ✓
                      </span>
                    )}
                  </li>
                );
              })
            )}
          </ul>
        </div>
      )}
    </div>
  );
});

export default SearchableSelect;
