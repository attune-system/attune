import { useEffect, useRef } from "react";
import type { RefObject } from "react";

interface InfiniteScrollTriggerProps {
  containerRef?: RefObject<HTMLElement | null>;
  fetchNextPage: () => Promise<unknown>;
  hasNextPage: boolean;
  isFetchingNextPage: boolean;
  itemLabel: string;
}

export default function InfiniteScrollTrigger({
  containerRef,
  fetchNextPage,
  hasNextPage,
  isFetchingNextPage,
  itemLabel,
}: InfiniteScrollTriggerProps) {
  const triggerRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const trigger = triggerRef.current;
    if (!trigger || !hasNextPage || isFetchingNextPage) {
      return;
    }

    const observer = new IntersectionObserver(
      ([entry]) => {
        if (entry.isIntersecting) {
          void fetchNextPage();
        }
      },
      {
        root: containerRef?.current ?? null,
        rootMargin: "0px 0px 200px",
      },
    );

    observer.observe(trigger);
    return () => observer.disconnect();
  }, [containerRef, fetchNextPage, hasNextPage, isFetchingNextPage]);

  if (!hasNextPage && !isFetchingNextPage) {
    return null;
  }

  return (
    <div
      ref={triggerRef}
      className="p-3 text-center text-sm text-gray-500"
      role="status"
      aria-live="polite"
    >
      {isFetchingNextPage ? (
        `Loading more ${itemLabel}...`
      ) : (
        <button
          type="button"
          onClick={() => void fetchNextPage()}
          className="text-blue-600 hover:text-blue-800"
        >
          Load more {itemLabel}
        </button>
      )}
    </div>
  );
}
