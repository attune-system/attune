import { extent, max, sum } from "d3-array";
import { scaleBand, scaleLinear, scalePoint, scaleSequential } from "d3-scale";
import { arc, area, line, stack, type SeriesPoint } from "d3-shape";
import { hierarchy, treemap as d3Treemap } from "d3-hierarchy";
import { interpolateBlues } from "d3-scale-chromatic";
import type { DashboardCardSpec, DashboardSourceResult } from "@/types/dashboard";
import {
  asNumber,
  buildCartesianSeriesModel,
  createSeriesColorScale,
  formatValue,
  getLevelColor,
  toKey,
  toRows,
} from "./foundation";

const CHART_WIDTH = 640;
const CHART_HEIGHT = 280;
const MARGIN = { top: 16, right: 16, bottom: 36, left: 44 };

function EmptyChart({ message }: { message: string }) {
  return (
    <div className="h-full min-h-32 flex items-center justify-center text-sm text-gray-500">
      {message}
    </div>
  );
}

function ChartLegend({
  items,
}: {
  items: Array<{ key: string; color: string }>;
}) {
  if (items.length <= 1) return null;
  return (
    <div className="flex flex-wrap gap-x-3 gap-y-1 text-xs text-gray-600 mt-2">
      {items.map((item) => (
        <span key={item.key} className="inline-flex items-center gap-1">
          <span className="w-2 h-2 rounded-sm" style={{ backgroundColor: item.color }} />
          {item.key}
        </span>
      ))}
    </div>
  );
}

function tickValues(domain: string[]): string[] {
  if (domain.length <= 6) return domain;
  const step = Math.ceil(domain.length / 6);
  return domain.filter((_, index) => index % step === 0 || index === domain.length - 1);
}

function TimeseriesChart({ card, source }: { card: DashboardCardSpec; source: DashboardSourceResult }) {
  const rows = toRows(source.data);
  if (!rows.length) return <EmptyChart message="No points in selected range." />;

  const xField = card.visualization.x_field || "bucket_start";
  const yField = card.visualization.y_field || card.visualization.value_field || "count";
  const seriesField = card.visualization.series_field;

  const model = buildCartesianSeriesModel(rows, xField, yField, seriesField);
  if (!model.xDomain.length || !model.seriesKeys.length) {
    return <EmptyChart message="No numeric points for selected fields." />;
  }

  const xScale = scalePoint<string>()
    .domain(model.xDomain)
    .range([MARGIN.left, CHART_WIDTH - MARGIN.right]);
  const yScale = scaleLinear()
    .domain([0, model.maxY])
    .nice()
    .range([CHART_HEIGHT - MARGIN.bottom, MARGIN.top]);
  const color = createSeriesColorScale(model.seriesKeys);

  const lineFactory = line<number | null>()
    .defined((value) => value !== null)
    .x((_, index) => xScale(model.xDomain[index]) ?? 0)
    .y((value) => yScale(value ?? 0));

  return (
    <div className="h-full flex flex-col">
      <svg viewBox={`0 0 ${CHART_WIDTH} ${CHART_HEIGHT}`} className="w-full h-full min-h-36">
        <line
          x1={MARGIN.left}
          x2={CHART_WIDTH - MARGIN.right}
          y1={CHART_HEIGHT - MARGIN.bottom}
          y2={CHART_HEIGHT - MARGIN.bottom}
          stroke="#e5e7eb"
        />
        <line
          x1={MARGIN.left}
          x2={MARGIN.left}
          y1={MARGIN.top}
          y2={CHART_HEIGHT - MARGIN.bottom}
          stroke="#e5e7eb"
        />

        {model.seriesKeys.map((seriesKey) => {
          const values = model.valuesBySeries.get(seriesKey) ?? [];
          const d = lineFactory(values) || "";
          const stroke = color(seriesKey);

          return (
            <g key={seriesKey}>
              <path d={d} fill="none" stroke={stroke} strokeWidth={2} />
              {values.map((value, index) => {
                if (value === null) return null;
                const cx = xScale(model.xDomain[index]);
                if (cx === undefined) return null;
                const cy = yScale(value);
                return (
                  <circle key={`${seriesKey}-${index}`} cx={cx} cy={cy} r={2.75} fill={stroke}>
                    <title>
                      {seriesKey} • {model.xDomain[index]} • {formatValue(value, card.visualization.format, source.meta.unit_hints[yField])}
                    </title>
                  </circle>
                );
              })}
            </g>
          );
        })}

        {tickValues(model.xDomain).map((tick) => {
          const x = xScale(tick);
          if (x === undefined) return null;
          return (
            <text key={tick} x={x} y={CHART_HEIGHT - 8} textAnchor="middle" fontSize={10} fill="#6b7280">
              {tick}
            </text>
          );
        })}
      </svg>

      {card.visualization.legend !== false && (
        <ChartLegend
          items={model.seriesKeys.map((key) => ({ key, color: color(key) }))}
        />
      )}
    </div>
  );
}

function StackedTimeseriesChart({ card, source }: { card: DashboardCardSpec; source: DashboardSourceResult }) {
  const rows = toRows(source.data);
  if (!rows.length) return <EmptyChart message="No points in selected range." />;

  const xField = card.visualization.x_field || "bucket_start";
  const yField = card.visualization.y_field || card.visualization.value_field || "count";
  const seriesField = card.visualization.series_field || "series";

  const model = buildCartesianSeriesModel(rows, xField, yField, seriesField);
  if (!model.xDomain.length || !model.seriesKeys.length) {
    return <EmptyChart message="No stacked points for selected fields." />;
  }

  const stackedInput = model.xDomain.map((_, index) => {
    const point: Record<string, number> = { __x_index: index };
    for (const seriesKey of model.seriesKeys) {
      point[seriesKey] = model.valuesBySeries.get(seriesKey)?.[index] ?? 0;
    }
    return point;
  });

  const layers = stack<Record<string, number>>().keys(model.seriesKeys)(stackedInput);
  const yMax = max(layers, (layer) => max(layer, (segment) => segment[1])) || 1;

  const xScale = scalePoint<string>()
    .domain(model.xDomain)
    .range([MARGIN.left, CHART_WIDTH - MARGIN.right]);
  const yScale = scaleLinear()
    .domain([0, yMax])
    .nice()
    .range([CHART_HEIGHT - MARGIN.bottom, MARGIN.top]);
  const color = createSeriesColorScale(model.seriesKeys);

  const areaFactory = area<SeriesPoint<Record<string, number>>>()
    .x((_, index) => xScale(model.xDomain[index]) ?? 0)
    .y0((segment) => yScale(segment[0]))
    .y1((segment) => yScale(segment[1]));

  return (
    <div className="h-full flex flex-col">
      <svg viewBox={`0 0 ${CHART_WIDTH} ${CHART_HEIGHT}`} className="w-full h-full min-h-36">
        <line
          x1={MARGIN.left}
          x2={CHART_WIDTH - MARGIN.right}
          y1={CHART_HEIGHT - MARGIN.bottom}
          y2={CHART_HEIGHT - MARGIN.bottom}
          stroke="#e5e7eb"
        />
        {layers.map((layer) => (
          <path
            key={layer.key}
            d={areaFactory(layer) || ""}
            fill={color(layer.key)}
            fillOpacity={0.8}
            stroke="#ffffff"
            strokeWidth={0.5}
          />
        ))}
        {tickValues(model.xDomain).map((tick) => {
          const x = xScale(tick);
          if (x === undefined) return null;
          return (
            <text key={tick} x={x} y={CHART_HEIGHT - 8} textAnchor="middle" fontSize={10} fill="#6b7280">
              {tick}
            </text>
          );
        })}
      </svg>

      {card.visualization.legend !== false && (
        <ChartLegend
          items={model.seriesKeys.map((key) => ({ key, color: color(key) }))}
        />
      )}
    </div>
  );
}

function GaugeChart({ card, source }: { card: DashboardCardSpec; source: DashboardSourceResult }) {
  const rows = toRows(source.data);
  if (!rows.length) return <EmptyChart message="No gauge value in selected range." />;

  const valueField = card.visualization.value_field || source.meta.ordering[0] || "value";
  const numericValue = asNumber(rows[0]?.[valueField]);
  if (numericValue === null) {
    return <EmptyChart message="Gauge value is not numeric." />;
  }

  const minValue = card.visualization.min ?? 0;
  const maxValue = card.visualization.max ?? 100;
  const span = Math.max(1, maxValue - minValue);
  const clamped = Math.max(minValue, Math.min(maxValue, numericValue));

  const bands =
    card.visualization.bands && card.visualization.bands.length > 0
      ? card.visualization.bands
      : [
          { from: minValue, to: minValue + span * 0.6, level: "good" },
          { from: minValue + span * 0.6, to: minValue + span * 0.85, level: "warning" },
          { from: minValue + span * 0.85, to: maxValue, level: "bad" },
        ];

  const angleScale = scaleLinear().domain([minValue, maxValue]).range([-Math.PI, 0]);

  const arcFactory = arc<{ startAngle: number; endAngle: number }>()
    .innerRadius(62)
    .outerRadius(84)
    .cornerRadius(2);

  const valueAngle = angleScale(clamped);
  const needleLength = 88;
  const needleX = Math.cos(valueAngle) * needleLength;
  const needleY = Math.sin(valueAngle) * needleLength;

  return (
    <div className="h-full flex flex-col items-center justify-center gap-2">
      <svg viewBox="-120 -110 240 140" className="w-full h-full min-h-32">
        {bands.map((band, index) => {
          const startAngle = angleScale(band.from);
          const endAngle = angleScale(Math.min(maxValue, band.to));
          const d = arcFactory({ startAngle, endAngle }) || "";
          const fill = band.color || getLevelColor(band.level, "#d1d5db");
          return <path key={`${band.level}-${index}`} d={d} fill={fill} opacity={0.95} />;
        })}

        <line x1={0} y1={0} x2={needleX} y2={needleY} stroke="#111827" strokeWidth={3} strokeLinecap="round" />
        <circle cx={0} cy={0} r={5} fill="#111827" />
      </svg>

      <div className="text-center">
        <p className="text-2xl font-semibold text-gray-900">
          {formatValue(clamped, card.visualization.format, source.meta.unit_hints[valueField])}
        </p>
        <p className="text-xs text-gray-500 mt-1">
          {valueField} ({minValue}–{maxValue})
        </p>
      </div>
    </div>
  );
}

function BarChart({ card, source }: { card: DashboardCardSpec; source: DashboardSourceResult }) {
  const rows = toRows(source.data);
  if (!rows.length) return <EmptyChart message="No bars in selected range." />;

  const xField = card.visualization.x_field || source.meta.ordering[0] || "category";
  const yField = card.visualization.y_field || card.visualization.value_field || "count";
  const seriesField = card.visualization.series_field;

  const model = buildCartesianSeriesModel(rows, xField, yField, seriesField);
  if (!model.xDomain.length) return <EmptyChart message="No categorical values available." />;

  const totals = model.xDomain.map((_, index) =>
    sum(model.seriesKeys, (seriesKey) => model.valuesBySeries.get(seriesKey)?.[index] ?? 0),
  );
  const yMax = Math.max(1, ...totals, model.maxY);

  const xScale = scaleBand<string>()
    .domain(model.xDomain)
    .range([MARGIN.left, CHART_WIDTH - MARGIN.right])
    .padding(0.2);
  const yScale = scaleLinear()
    .domain([0, yMax])
    .nice()
    .range([CHART_HEIGHT - MARGIN.bottom, MARGIN.top]);

  const color = createSeriesColorScale(model.seriesKeys);
  const subgroup = scaleBand<string>()
    .domain(model.seriesKeys)
    .range([0, xScale.bandwidth()])
    .padding(0.12);

  return (
    <div className="h-full flex flex-col">
      <svg viewBox={`0 0 ${CHART_WIDTH} ${CHART_HEIGHT}`} className="w-full h-full min-h-36">
        <line
          x1={MARGIN.left}
          x2={CHART_WIDTH - MARGIN.right}
          y1={CHART_HEIGHT - MARGIN.bottom}
          y2={CHART_HEIGHT - MARGIN.bottom}
          stroke="#e5e7eb"
        />

        {model.xDomain.map((xKey, xIndex) => {
          const x = xScale(xKey);
          if (x === undefined) return null;

          return (
            <g key={xKey} transform={`translate(${x},0)`}>
              {model.seriesKeys.map((seriesKey) => {
                const value = model.valuesBySeries.get(seriesKey)?.[xIndex];
                if (value === null || value === undefined) return null;
                const barHeight = yScale(0) - yScale(value);
                const xInner = subgroup(seriesKey);
                if (xInner === undefined) return null;
                return (
                  <rect
                    key={`${xKey}-${seriesKey}`}
                    x={xInner}
                    y={yScale(value)}
                    width={subgroup.bandwidth()}
                    height={Math.max(0, barHeight)}
                    fill={color(seriesKey)}
                  >
                    <title>
                      {xKey} • {seriesKey} • {formatValue(value, card.visualization.format, source.meta.unit_hints[yField])}
                    </title>
                  </rect>
                );
              })}
            </g>
          );
        })}

        {tickValues(model.xDomain).map((tick) => {
          const x = xScale(tick);
          if (x === undefined) return null;
          return (
            <text key={tick} x={x + xScale.bandwidth() / 2} y={CHART_HEIGHT - 8} textAnchor="middle" fontSize={10} fill="#6b7280">
              {tick}
            </text>
          );
        })}
      </svg>

      {card.visualization.legend !== false && (
        <ChartLegend
          items={model.seriesKeys.map((key) => ({ key, color: color(key) }))}
        />
      )}
    </div>
  );
}

function HeatmapChart({ card, source }: { card: DashboardCardSpec; source: DashboardSourceResult }) {
  const rows = toRows(source.data);
  if (!rows.length) return <EmptyChart message="No heatmap cells in selected range." />;

  const xField = card.visualization.x_field || "x";
  const yField = card.visualization.y_field || "y";
  const valueField = card.visualization.value_field || "value";

  const xDomain: string[] = [];
  const yDomain: string[] = [];
  const xSeen = new Set<string>();
  const ySeen = new Set<string>();

  const values = new Map<string, number>();

  for (const row of rows) {
    const x = toKey(row[xField], "x");
    const y = toKey(row[yField], "y");
    const value = asNumber(row[valueField]);
    if (value === null) continue;

    if (!xSeen.has(x)) {
      xSeen.add(x);
      xDomain.push(x);
    }
    if (!ySeen.has(y)) {
      ySeen.add(y);
      yDomain.push(y);
    }
    values.set(`${x}::${y}`, value);
  }

  if (!xDomain.length || !yDomain.length) {
    return <EmptyChart message="No numeric heatmap values available." />;
  }

  const xScale = scaleBand<string>()
    .domain(xDomain)
    .range([MARGIN.left, CHART_WIDTH - MARGIN.right])
    .padding(0.03);
  const yScale = scaleBand<string>()
    .domain(yDomain)
    .range([MARGIN.top, CHART_HEIGHT - MARGIN.bottom])
    .padding(0.03);

  const valueMax = max(Array.from(values.values())) || 1;
  const color = scaleSequential(interpolateBlues).domain([0, valueMax]);

  return (
    <svg viewBox={`0 0 ${CHART_WIDTH} ${CHART_HEIGHT}`} className="w-full h-full min-h-36">
      {xDomain.flatMap((x) =>
        yDomain.map((y) => {
          const xPos = xScale(x);
          const yPos = yScale(y);
          if (xPos === undefined || yPos === undefined) return null;
          const value = values.get(`${x}::${y}`) ?? 0;
          return (
            <rect
              key={`${x}-${y}`}
              x={xPos}
              y={yPos}
              width={xScale.bandwidth()}
              height={yScale.bandwidth()}
              fill={color(value)}
              rx={2}
            >
              <title>
                {x} • {y} • {formatValue(value, card.visualization.format, source.meta.unit_hints[valueField])}
              </title>
            </rect>
          );
        }),
      )}

      {tickValues(xDomain).map((tick) => {
        const x = xScale(tick);
        if (x === undefined) return null;
        return (
          <text key={tick} x={x + xScale.bandwidth() / 2} y={CHART_HEIGHT - 8} textAnchor="middle" fontSize={10} fill="#6b7280">
            {tick}
          </text>
        );
      })}

      {yDomain.map((tick) => {
        const y = yScale(tick);
        if (y === undefined) return null;
        return (
          <text key={tick} x={MARGIN.left - 6} y={y + yScale.bandwidth() / 2} textAnchor="end" dominantBaseline="middle" fontSize={10} fill="#6b7280">
            {tick}
          </text>
        );
      })}
    </svg>
  );
}

function HistogramChart({ card, source }: { card: DashboardCardSpec; source: DashboardSourceResult }) {
  const rows = toRows(source.data);
  if (!rows.length) return <EmptyChart message="No values in selected range." />;

  const valueField = card.visualization.value_field || card.visualization.y_field || "value";
  const values = rows
    .map((row) => asNumber(row[valueField]))
    .filter((value): value is number => value !== null);

  if (!values.length) {
    return <EmptyChart message="Histogram requires numeric values." />;
  }

  const [minValue, maxValue] = extent(values) as [number, number];
  const binCount = Math.max(4, Math.min(20, Math.round(Math.sqrt(values.length))));
  const width = Math.max((maxValue - minValue) / binCount, 1e-9);
  const bins = Array.from({ length: binCount }, (_, index) => {
    const start = minValue + index * width;
    const end = index === binCount - 1 ? maxValue : start + width;
    const count = values.filter((value) =>
      index === binCount - 1 ? value >= start && value <= end : value >= start && value < end,
    ).length;
    return { start, end, count };
  });

  const xScale = scaleBand<number>()
    .domain(bins.map((_, index) => index))
    .range([MARGIN.left, CHART_WIDTH - MARGIN.right])
    .padding(0.12);
  const yScale = scaleLinear()
    .domain([0, max(bins, (bin) => bin.count) || 1])
    .nice()
    .range([CHART_HEIGHT - MARGIN.bottom, MARGIN.top]);

  return (
    <svg viewBox={`0 0 ${CHART_WIDTH} ${CHART_HEIGHT}`} className="w-full h-full min-h-36">
      {bins.map((bin, index) => {
        const x = xScale(index);
        if (x === undefined) return null;
        const y = yScale(bin.count);
        return (
          <rect
            key={index}
            x={x}
            y={y}
            width={xScale.bandwidth()}
            height={yScale(0) - y}
            fill="#2563eb"
            opacity={0.85}
          >
            <title>
              {`${bin.start.toFixed(2)} – ${bin.end.toFixed(2)}: ${bin.count}`}
            </title>
          </rect>
        );
      })}
    </svg>
  );
}

function FunnelChart({ card, source }: { card: DashboardCardSpec; source: DashboardSourceResult }) {
  const rows = toRows(source.data);
  if (!rows.length) return <EmptyChart message="No funnel stages in selected range." />;

  const stageField = card.visualization.x_field || card.visualization.series_field || "stage";
  const valueField = card.visualization.y_field || card.visualization.value_field || "value";

  const stages = rows
    .map((row) => ({
      label: toKey(row[stageField], "stage"),
      value: asNumber(row[valueField]),
    }))
    .filter((row): row is { label: string; value: number } => row.value !== null);

  if (!stages.length) return <EmptyChart message="No numeric funnel values available." />;

  const maxValue = Math.max(...stages.map((stage) => stage.value), 1);
  const sectionHeight = 180 / stages.length;

  return (
    <svg viewBox="0 0 420 220" className="w-full h-full min-h-36">
      {stages.map((stage, index) => {
        const topWidth = (stage.value / maxValue) * 300;
        const nextValue = stages[index + 1]?.value ?? stage.value;
        const bottomWidth = (nextValue / maxValue) * 300;
        const y = 20 + index * sectionHeight;
        const xTop = 210 - topWidth / 2;
        const xBottom = 210 - bottomWidth / 2;

        const path = `M ${xTop} ${y} L ${xTop + topWidth} ${y} L ${xBottom + bottomWidth} ${y + sectionHeight - 4} L ${xBottom} ${y + sectionHeight - 4} Z`;

        return (
          <g key={stage.label}>
            <path d={path} fill="#2563eb" opacity={0.85 - index * 0.08} />
            <text x={210} y={y + sectionHeight / 2} textAnchor="middle" dominantBaseline="middle" fill="white" fontSize={11} fontWeight={600}>
              {stage.label}: {formatValue(stage.value, card.visualization.format, source.meta.unit_hints[valueField])}
            </text>
          </g>
        );
      })}
    </svg>
  );
}

function TreemapChart({ card, source }: { card: DashboardCardSpec; source: DashboardSourceResult }) {
  const rows = toRows(source.data);
  if (!rows.length) return <EmptyChart message="No treemap nodes in selected range." />;

  const labelField = card.visualization.x_field || card.visualization.series_field || "label";
  const valueField = card.visualization.y_field || card.visualization.value_field || "value";

  const children = rows
    .map((row) => ({ name: toKey(row[labelField], "item"), value: asNumber(row[valueField]) }))
    .filter((item): item is { name: string; value: number } => item.value !== null);

  if (!children.length) return <EmptyChart message="No numeric treemap values available." />;

  interface TreemapNode {
    name: string;
    value?: number;
    children?: TreemapNode[];
  }

  const root = hierarchy<TreemapNode>({
    name: "root",
    children,
  }).sum((node) => node.value ?? 0);
  const treemapLayout = d3Treemap<TreemapNode>()
    .size([CHART_WIDTH - MARGIN.left - MARGIN.right, CHART_HEIGHT - MARGIN.top - MARGIN.bottom])
    .padding(3);
  const layoutRoot = treemapLayout(root);

  const leaves = layoutRoot.leaves();
  const color = createSeriesColorScale(leaves.map((leaf) => leaf.data.name));

  return (
    <svg viewBox={`0 0 ${CHART_WIDTH} ${CHART_HEIGHT}`} className="w-full h-full min-h-36">
      <g transform={`translate(${MARGIN.left},${MARGIN.top})`}>
        {leaves.map((leaf) => (
          <g key={leaf.data.name}>
            <rect
              x={leaf.x0}
              y={leaf.y0}
              width={Math.max(0, leaf.x1 - leaf.x0)}
              height={Math.max(0, leaf.y1 - leaf.y0)}
              fill={color(leaf.data.name)}
              opacity={0.88}
              rx={2}
            >
              <title>
                {leaf.data.name}: {formatValue(leaf.value, card.visualization.format, source.meta.unit_hints[valueField])}
              </title>
            </rect>
            <text x={leaf.x0 + 6} y={leaf.y0 + 14} fill="white" fontSize={10}>
              {leaf.data.name}
            </text>
          </g>
        ))}
      </g>
    </svg>
  );
}

function StatusMatrixChart({ card, source }: { card: DashboardCardSpec; source: DashboardSourceResult }) {
  const rows = toRows(source.data);
  if (!rows.length) return <EmptyChart message="No status cells in selected range." />;

  const xField = card.visualization.x_field || "x";
  const yField = card.visualization.y_field || "y";
  const valueField = card.visualization.value_field || "status";

  const xDomain: string[] = [];
  const yDomain: string[] = [];
  const xSeen = new Set<string>();
  const ySeen = new Set<string>();

  const statusByCell = new Map<string, string>();

  for (const row of rows) {
    const x = toKey(row[xField], "x");
    const y = toKey(row[yField], "y");
    const status = toKey(row[valueField], "unknown");

    if (!xSeen.has(x)) {
      xSeen.add(x);
      xDomain.push(x);
    }
    if (!ySeen.has(y)) {
      ySeen.add(y);
      yDomain.push(y);
    }

    statusByCell.set(`${x}::${y}`, status);
  }

  if (!xDomain.length || !yDomain.length) {
    return <EmptyChart message="No status matrix values available." />;
  }

  const xScale = scaleBand<string>()
    .domain(xDomain)
    .range([MARGIN.left, CHART_WIDTH - MARGIN.right])
    .padding(0.06);
  const yScale = scaleBand<string>()
    .domain(yDomain)
    .range([MARGIN.top, CHART_HEIGHT - MARGIN.bottom])
    .padding(0.06);

  return (
    <svg viewBox={`0 0 ${CHART_WIDTH} ${CHART_HEIGHT}`} className="w-full h-full min-h-36">
      {xDomain.flatMap((x) =>
        yDomain.map((y) => {
          const xPos = xScale(x);
          const yPos = yScale(y);
          if (xPos === undefined || yPos === undefined) return null;
          const status = statusByCell.get(`${x}::${y}`) || "unknown";
          return (
            <rect
              key={`${x}-${y}`}
              x={xPos}
              y={yPos}
              width={xScale.bandwidth()}
              height={yScale.bandwidth()}
              rx={3}
              fill={getLevelColor(status, "#9ca3af")}
            >
              <title>
                {x} • {y} • {status}
              </title>
            </rect>
          );
        }),
      )}
    </svg>
  );
}

export function DashboardChartRenderer({
  card,
  source,
}: {
  card: DashboardCardSpec;
  source: DashboardSourceResult;
}) {
  const type = card.visualization.type;

  if (type === "timeseries") return <TimeseriesChart card={card} source={source} />;
  if (type === "stacked_timeseries") {
    return <StackedTimeseriesChart card={card} source={source} />;
  }
  if (type === "gauge") return <GaugeChart card={card} source={source} />;
  if (type === "bar") return <BarChart card={card} source={source} />;
  if (type === "heatmap") return <HeatmapChart card={card} source={source} />;
  if (type === "histogram") return <HistogramChart card={card} source={source} />;
  if (type === "funnel") return <FunnelChart card={card} source={source} />;
  if (type === "treemap") return <TreemapChart card={card} source={source} />;
  if (type === "status_matrix") {
    return <StatusMatrixChart card={card} source={source} />;
  }

  return <EmptyChart message={`Unsupported visualization type: ${type}`} />;
}
