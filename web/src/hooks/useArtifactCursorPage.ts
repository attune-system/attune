import { useCallback, useState } from "react";
import { useArtifactLivePage } from "./useArtifactLivePage";

/** A cursor is a browse position, never an authorization result or snapshot. */
export function useArtifactCursorPage(
  scope: "list" | "execution" | "versions",
  paused: boolean,
  id?: number,
  filters = "",
) {
  const identity = `${scope}:${id ?? ""}:${filters}`;
  const [browse, setBrowse] = useState<{
    identity: string;
    cursors: Array<string | undefined>;
  }>({ identity, cursors: [undefined] });
  // Reset during render so no request can send a cursor bound to old filters or
  // another artifact/execution before an effect clears it.
  const cursors = browse.identity === identity ? browse.cursors : [undefined];
  if (browse.identity !== identity) setBrowse({ identity, cursors });
  const reset = useCallback(
    () => setBrowse((value) => ({ ...value, cursors: [undefined] })),
    [],
  );
  useArtifactLivePage({
    scope,
    id,
    paused,
    page: cursors.length,
    setPage: reset,
  });
  return {
    cursor: cursors.at(-1),
    hasPrevious: cursors.length > 1,
    reset,
    previous: () =>
      setBrowse((value) => ({
        ...value,
        cursors:
          value.cursors.length > 1 ? value.cursors.slice(0, -1) : value.cursors,
      })),
    next: (cursor: string) =>
      setBrowse((value) => ({ ...value, cursors: [...value.cursors, cursor] })),
  };
}
