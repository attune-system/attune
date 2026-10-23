import { useEffect, useMemo, useRef } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { useWebSocketContext } from "@/contexts/WebSocketContext";
import {
  registerArtifactReconciliation,
  scheduleArtifactRefresh,
} from "@/lib/artifact-refresh";

export function useArtifactReconciliation({
  scope,
  id,
  isRunning = false,
  paused = false,
}: {
  scope: "list" | "execution" | "detail" | "versions";
  id?: number;
  isRunning?: boolean;
  paused?: boolean;
}) {
  const client = useQueryClient();
  const { connected } = useWebSocketContext();
  const previous = useRef({ connected, isRunning, paused });
  const key = useMemo(() => {
    switch (scope) {
      case "list":
        return ["artifacts", "list"];
      case "execution":
        return ["artifacts", "execution", id];
      case "detail":
        return ["artifacts", id];
      case "versions":
        return ["artifacts", id, "versions"];
    }
  }, [scope, id]);

  useEffect(() => {
    const before = previous.current;
    previous.current = { connected, isRunning, paused };
    if (paused || (scope !== "list" && !id)) return;
    let active = true;
    const requester = () => active;
    if (
      (!before.connected && connected) ||
      (before.isRunning && !isRunning) ||
      before.paused
    ) {
      scheduleArtifactRefresh(client, key, requester);
    }
    const unregister = registerArtifactReconciliation(
      client,
      key,
      !connected && isRunning ? 3000 : 30000,
      requester,
    );
    return () => {
      active = false;
      unregister();
    };
  }, [client, connected, id, isRunning, key, paused, scope]);
}
