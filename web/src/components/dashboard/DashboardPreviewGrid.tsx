import { DashboardCard } from "@/components/dashboard/DashboardCard";
import type {
  DashboardBreakpoint,
  DashboardGridRect,
  DashboardSourceResult,
  DashboardSpec,
} from "@/types/dashboard";

interface DashboardPreviewGridProps {
  spec: DashboardSpec;
  breakpoint: string;
  sourceById: Map<string, DashboardSourceResult>;
  isRefreshing?: boolean;
  onRetry?: () => void;
}

function resolveCardRect(
  breakpoint: string,
  position: Record<string, DashboardGridRect>,
  breakpoints: Record<string, DashboardBreakpoint>,
  defaultColumns: number,
): DashboardGridRect {
  if (position[breakpoint]) {
    return position[breakpoint];
  }

  const fallbackKey = position.lg ? "lg" : Object.keys(position)[0];
  const fallbackRect = (fallbackKey ? position[fallbackKey] : undefined) || {
    x: 0,
    y: 0,
    w: 1,
    h: 1,
  };
  const fromColumns = breakpoints[fallbackKey]?.columns ?? defaultColumns;
  const toColumns = breakpoints[breakpoint]?.columns ?? defaultColumns;
  const safeFrom = Math.max(1, fromColumns);
  const safeTo = Math.max(1, toColumns);
  const projectedW = Math.max(
    1,
    Math.min(safeTo, Math.round((fallbackRect.w / safeFrom) * safeTo)),
  );
  const projectedX = Math.max(
    0,
    Math.min(
      safeTo - projectedW,
      Math.round((fallbackRect.x / safeFrom) * safeTo),
    ),
  );
  return {
    x: projectedX,
    y: Math.max(0, fallbackRect.y),
    w: projectedW,
    h: Math.max(1, fallbackRect.h),
  };
}

export function DashboardPreviewGrid({
  spec,
  breakpoint,
  sourceById,
  isRefreshing,
  onRetry,
}: DashboardPreviewGridProps) {
  const activeColumns =
    spec.layout.breakpoints[breakpoint]?.columns || spec.layout.columns || 12;

  return (
    <section
      className="grid"
      style={{
        gridTemplateColumns: `repeat(${activeColumns}, minmax(0, 1fr))`,
        gap: `${spec.layout.gap}px`,
        gridAutoRows: `${spec.layout.row_height}px`,
      }}
    >
      {spec.cards.map((card) => {
        const rect = resolveCardRect(
          breakpoint,
          card.position,
          spec.layout.breakpoints,
          spec.layout.columns,
        );
        return (
          <div
            key={card.id}
            style={{
              gridColumn: `${rect.x + 1} / span ${rect.w}`,
              gridRow: `${rect.y + 1} / span ${rect.h}`,
            }}
          >
            <DashboardCard
              card={card}
              source={sourceById.get(card.source)}
              isRefreshing={isRefreshing}
              onRetry={onRetry}
            />
          </div>
        );
      })}
    </section>
  );
}
