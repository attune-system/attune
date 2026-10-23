import { useEffect, useMemo, useRef } from "react";
import { hashKey, useQueryClient } from "@tanstack/react-query";

function browsePage(queryKey: readonly unknown[]): number | undefined {
  const params = queryKey.at(-1);
  return typeof params === "object" &&
    params !== null &&
    "page" in params &&
    typeof params.page === "number"
    ? params.page
    : undefined;
}

/** Offset pages shift on insertion/retention. An affected live refresh starts a
 * new browse at page one; paused navigation does not reset or discard pages. */
export function useArtifactLivePage({
  scope,
  id,
  page,
  setPage,
  paused,
}: {
  scope: "list" | "execution" | "versions";
  id?: number;
  page: number;
  setPage: (page: number) => void;
  paused: boolean;
}) {
  const client = useQueryClient();
  const previousPause = useRef(paused);
  const key = useMemo(() => {
    switch (scope) {
      case "list":
        return ["artifacts", "list"];
      case "execution":
        return ["artifacts", "execution", id];
      case "versions":
        return ["artifacts", id, "versions"];
    }
  }, [scope, id]);

  useEffect(() => {
    const resumed = previousPause.current && !paused;
    previousPause.current = paused;
    if (paused) return;
    if (resumed && page > 1) {
      client.removeQueries({
        queryKey: key,
        predicate: (query) => browsePage(query.queryKey) !== undefined,
      });
      setPage(1);
    }
    return client.getQueryCache().subscribe((event) => {
      if (
        event.type !== "updated" ||
        event.action.type !== "invalidate" ||
        hashKey(event.query.queryKey.slice(0, key.length)) !== hashKey(key)
      )
        return;
      if (page > 1) {
        // Remove the old browse before mounting page one. The next query must
        // fetch fresh rows, not present a still-fresh cached first page.
        // Keep independently mounted discovery queries under this prefix.
        client.removeQueries({
          queryKey: key,
          predicate: (query) => browsePage(query.queryKey) !== undefined,
        });
        setPage(1);
      } else {
        client.removeQueries({
          queryKey: key,
          predicate: (query) => (browsePage(query.queryKey) ?? 1) > 1,
        });
      }
    });
  }, [client, key, page, paused, setPage]);
}
