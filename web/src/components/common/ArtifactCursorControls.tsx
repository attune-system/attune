export default function ArtifactCursorControls({
  hasPrevious,
  hasMore,
  onPrevious,
  onNext,
  disabled = false,
}: {
  hasPrevious: boolean;
  hasMore: boolean;
  onPrevious: () => void;
  onNext: () => void;
  disabled?: boolean;
}) {
  return (
    <nav
      aria-label="Artifact pagination"
      className="flex justify-end gap-2 px-4 py-3"
    >
      <button
        className="px-3 py-1 border rounded disabled:opacity-50"
        disabled={disabled || !hasPrevious}
        onClick={onPrevious}
      >
        Previous
      </button>
      <button
        className="px-3 py-1 border rounded disabled:opacity-50"
        disabled={disabled || !hasMore}
        onClick={onNext}
      >
        Next
      </button>
    </nav>
  );
}
