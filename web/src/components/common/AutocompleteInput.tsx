import { useState, useRef, useEffect, useCallback, memo } from "react";

interface AutocompleteInputProps {
  label: string;
  value: string;
  onChange: (value: string) => void;
  suggestions: string[];
  placeholder?: string;
}

/**
 * A text input with dropdown autocomplete suggestions.
 * Allows free-text entry — selecting a suggestion simply fills the input.
 * Memoized so it only re-renders when its own props change, not when
 * sibling components (e.g. a results table) update.
 */
const AutocompleteInput = memo(
  ({
    label,
    value,
    onChange,
    suggestions,
    placeholder = "",
  }: AutocompleteInputProps) => {
    const [isOpen, setIsOpen] = useState(false);
    const [highlightedIndex, setHighlightedIndex] = useState(-1);
    const containerRef = useRef<HTMLDivElement>(null);
    const inputRef = useRef<HTMLInputElement>(null);
    const listRef = useRef<HTMLUListElement>(null);

    // Filter suggestions based on current input value (case-insensitive substring)
    const filtered =
      value.length === 0
        ? suggestions
        : suggestions.filter((s) =>
            s.toLowerCase().includes(value.toLowerCase()),
          );

    // Clamp the highlighted index when the filtered list shrinks
    const safeIndex =
      highlightedIndex >= filtered.length ? -1 : highlightedIndex;

    // Close dropdown when clicking outside
    useEffect(() => {
      const handleClickOutside = (event: MouseEvent) => {
        if (
          containerRef.current &&
          !containerRef.current.contains(event.target as Node)
        ) {
          setIsOpen(false);
        }
      };
      document.addEventListener("mousedown", handleClickOutside);
      return () =>
        document.removeEventListener("mousedown", handleClickOutside);
    }, []);

    // Scroll highlighted item into view
    useEffect(() => {
      if (safeIndex >= 0 && listRef.current) {
        const items = listRef.current.children;
        if (items[safeIndex]) {
          (items[safeIndex] as HTMLElement).scrollIntoView({
            block: "nearest",
          });
        }
      }
    }, [safeIndex]);

    const selectSuggestion = useCallback(
      (suggestion: string) => {
        onChange(suggestion);
        setIsOpen(false);
        setHighlightedIndex(-1);
        // Keep focus on input after selection
        inputRef.current?.focus();
      },
      [onChange],
    );

    const handleKeyDown = useCallback(
      (e: React.KeyboardEvent) => {
        if (!isOpen || filtered.length === 0) {
          // Open dropdown on arrow down even if closed
          if (e.key === "ArrowDown" && filtered.length > 0) {
            e.preventDefault();
            setIsOpen(true);
            setHighlightedIndex(0);
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
              selectSuggestion(filtered[safeIndex]);
            }
            break;
          case "Escape":
            e.preventDefault();
            setIsOpen(false);
            setHighlightedIndex(-1);
            break;
          case "Tab":
            setIsOpen(false);
            setHighlightedIndex(-1);
            break;
        }
      },
      [isOpen, filtered, safeIndex, selectSuggestion],
    );

    const handleInputChange = useCallback(
      (e: React.ChangeEvent<HTMLInputElement>) => {
        onChange(e.target.value);
        setIsOpen(true);
        setHighlightedIndex(-1);
      },
      [onChange],
    );

    const handleFocus = useCallback(() => {
      if (suggestions.length > 0) {
        setIsOpen(true);
      }
    }, [suggestions.length]);

    return (
      <div ref={containerRef} className="relative">
        <label className="block text-sm font-medium text-gray-700 mb-1">
          {label}
        </label>
        <input
          ref={inputRef}
          type="text"
          value={value}
          onChange={handleInputChange}
          onFocus={handleFocus}
          onKeyDown={handleKeyDown}
          placeholder={placeholder}
          autoComplete="off"
          className="w-full px-3 py-2 border border-gray-300 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
        />

        {isOpen && filtered.length > 0 && (
          <ul
            ref={listRef}
            className="absolute z-50 mt-1 w-full bg-white border border-gray-300 rounded-md shadow-lg max-h-48 overflow-y-auto"
          >
            {filtered.map((suggestion, index) => {
              const isHighlighted = index === safeIndex;
              // Highlight the matching substring
              const matchIndex = suggestion
                .toLowerCase()
                .indexOf(value.toLowerCase());
              const before =
                matchIndex >= 0 ? suggestion.slice(0, matchIndex) : suggestion;
              const match =
                matchIndex >= 0
                  ? suggestion.slice(matchIndex, matchIndex + value.length)
                  : "";
              const after =
                matchIndex >= 0
                  ? suggestion.slice(matchIndex + value.length)
                  : "";

              return (
                <li
                  key={suggestion}
                  onMouseDown={(e) => {
                    // Use mousedown instead of click to fire before input blur
                    e.preventDefault();
                    selectSuggestion(suggestion);
                  }}
                  onMouseEnter={() => setHighlightedIndex(index)}
                  className={`px-3 py-2 text-sm cursor-pointer ${
                    isHighlighted ? "bg-blue-50 text-blue-900" : "text-gray-900"
                  } hover:bg-blue-50`}
                >
                  {value.length > 0 && matchIndex >= 0 ? (
                    <>
                      {before}
                      <span className="font-semibold">{match}</span>
                      {after}
                    </>
                  ) : (
                    suggestion
                  )}
                </li>
              );
            })}
          </ul>
        )}
      </div>
    );
  },
);

AutocompleteInput.displayName = "AutocompleteInput";

export default AutocompleteInput;
