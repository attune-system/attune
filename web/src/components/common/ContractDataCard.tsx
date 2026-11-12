import type { ReactNode } from "react";
import { JsonValueDisplay } from "./CuratedDataPanel";
import { isJsonObject } from "./curatedDataUtils";

import { DisplayGraph } from "./contractSchema";

function renderBoundValue(
  graph: DisplayGraph,
  value: unknown,
  schemas: unknown[],
  depth: number,
  maskSecrets: boolean,
  inheritedSecret = false,
): ReactNode {
  graph.visit(depth);
  const views = graph.views(schemas);
  const secret = inheritedSecret || views.some((node) => node.secret === true);
  if (
    (isJsonObject(value) && value.$attune_secret === true) ||
    (maskSecrets && secret && value !== undefined)
  )
    return <span className="text-sm text-gray-500">[REDACTED]</span>;
  if (Array.isArray(value)) {
    if (!value.length) return <JsonValueDisplay value={value} />;
    return (
      <ol className="space-y-2 pl-3 border-l">
        {value.map((item, index) => {
          const children = views.flatMap((node) => {
            const prefix = Array.isArray(node.prefixItems)
              ? node.prefixItems[index]
              : undefined;
            return [prefix ?? node.items, node.contains].filter(
              (child) => child !== undefined,
            );
          });
          return (
            <li key={index}>
              <span className="text-xs text-gray-500">[{index}]</span>
              {renderBoundValue(
                graph,
                item,
                children,
                depth + 1,
                maskSecrets,
                secret,
              )}
            </li>
          );
        })}
      </ol>
    );
  }
  if (isJsonObject(value)) {
    const keys = new Set(Object.keys(value));
    for (const node of views)
      if (isJsonObject(node.properties))
        for (const key of Object.keys(node.properties)) keys.add(key);
    if (!keys.size) return <JsonValueDisplay value={value} />;
    const fields = [...keys]
      .map((key, order) => {
        graph.visit(depth + 1);
        const children = views.flatMap((node) => {
          const properties = node.properties;
          const child =
            isJsonObject(properties) && Object.hasOwn(properties, key)
              ? properties[key]
              : node.additionalProperties;
          return child === undefined ? [] : [child];
        });
        const childViews = graph.views(children);
        const position = childViews.find(
          (node) => typeof node.position === "number",
        )?.position;
        return {
          key,
          order,
          children,
          childViews,
          position: typeof position === "number" ? position : Infinity,
        };
      })
      .sort(
        (left, right) =>
          left.position - right.position || left.order - right.order,
      );
    return (
      <dl className="divide-y divide-gray-100">
        {fields.map(({ key, children, childViews }) => {
          const title = childViews.find(
            (node) => typeof node.title === "string",
          )?.title;
          const description = childViews.find(
            (node) => typeof node.description === "string",
          )?.description;
          const types = childViews.flatMap((node) =>
            typeof node.type === "string"
              ? [node.type]
              : Array.isArray(node.type)
                ? node.type.filter(
                    (type: unknown): type is string => typeof type === "string",
                  )
                : [],
          );
          const childSecret =
            secret || childViews.some((node) => node.secret === true);
          return (
            <div key={key} className="py-2">
              <dt className="flex items-center gap-2 text-sm">
                <span className="font-mono">{key}</span>
                {typeof title === "string" && title !== key && title.trim() && (
                  <span className="font-medium">{title}</span>
                )}
                {types.length > 0 && (
                  <span className="text-xs text-gray-500">
                    {[...new Set(types)].join(" | ")}
                  </span>
                )}
                {childSecret && (
                  <span className="text-xs text-purple-700">secret</span>
                )}
              </dt>
              {typeof description === "string" && (
                <p className="text-xs text-gray-500">{description}</p>
              )}
              <dd className="pl-3">
                {renderBoundValue(
                  graph,
                  Object.hasOwn(value, key) ? value[key] : undefined,
                  children,
                  depth + 1,
                  maskSecrets,
                  childSecret,
                )}
              </dd>
            </div>
          );
        })}
      </dl>
    );
  }
  return <JsonValueDisplay value={value} />;
}

export function ContractValueRows({
  contract,
  values,
  emptyMessage,
  maskSecrets = false,
}: {
  contract: unknown;
  values: unknown;
  emptyMessage: string;
  maskSecrets?: boolean;
}) {
  let content: ReactNode;
  if (values == null)
    content = <p className="text-sm text-gray-500">{emptyMessage}</p>;
  else if (!isJsonObject(contract) || !isJsonObject(contract.annotated_schema))
    content = <JsonValueDisplay value={values} />;
  else {
    try {
      const graph = new DisplayGraph(contract.annotated_schema);
      content = renderBoundValue(
        graph,
        values,
        [contract.annotated_schema],
        0,
        maskSecrets,
      );
    } catch {
      content = (
        <p role="alert" className="text-sm text-gray-500">
          The stored schema cannot be rendered within the display limits.
        </p>
      );
    }
  }
  return <>{content}</>;
}

export default function ContractDataCard({
  title,
  ...props
}: Parameters<typeof ContractValueRows>[0] & { title: string }) {
  return (
    <section className="bg-white rounded-lg shadow">
      <h2 className="px-6 py-4 border-b text-lg font-semibold">{title}</h2>
      <div className="px-6 py-4">
        <ContractValueRows {...props} />
      </div>
    </section>
  );
}
