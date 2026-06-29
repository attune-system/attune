import { Fragment, useMemo, useState } from "react";
import { useSearchParams } from "react-router-dom";
import { Search, X, ScrollText, ChevronDown, ChevronRight } from "lucide-react";
import { useAuditEvents } from "@/hooks/useAuditEvents";
import { AuditCategory } from "@/api/models/AuditCategory";
import { AuditOutcome } from "@/api/models/AuditOutcome";
import type { AuditEventSummary } from "@/api/models/AuditEventSummary";
import Pagination from "@/components/executions/Pagination";

const CATEGORIES: AuditCategory[] = [
  AuditCategory.API,
  AuditCategory.AUTH,
  AuditCategory.RBAC,
  AuditCategory.SECRET,
  AuditCategory.ADMIN,
  AuditCategory.EXECUTION,
  AuditCategory.PACK,
];

const OUTCOMES: AuditOutcome[] = [
  AuditOutcome.SUCCESS,
  AuditOutcome.FAILURE,
  AuditOutcome.DENIED,
];

const HTTP_METHODS = ["GET", "POST", "PUT", "PATCH", "DELETE"];

function outcomeClass(outcome: string): string {
  switch (outcome) {
    case "success":
      return "bg-green-100 text-green-800";
    case "failure":
      return "bg-red-100 text-red-800";
    case "denied":
      return "bg-amber-100 text-amber-800";
    default:
      return "bg-gray-100 text-gray-800";
  }
}

function categoryClass(category: string): string {
  switch (category) {
    case "auth":
      return "bg-blue-100 text-blue-800";
    case "rbac":
      return "bg-purple-100 text-purple-800";
    case "secret":
      return "bg-rose-100 text-rose-800";
    case "admin":
      return "bg-orange-100 text-orange-800";
    case "execution":
      return "bg-teal-100 text-teal-800";
    case "pack":
      return "bg-indigo-100 text-indigo-800";
    case "api":
      return "bg-slate-100 text-slate-800";
    default:
      return "bg-gray-100 text-gray-800";
  }
}

function formatDate(s: string): string {
  return new Date(s).toLocaleString();
}

const FILTER_INPUT =
  "w-full text-sm rounded border border-gray-300 px-2 py-1.5 bg-white focus:outline-none focus:ring-2 focus:ring-slate-500 focus:border-slate-500";

export default function AuditLogPage() {
  const [searchParams, setSearchParams] = useSearchParams();
  const initial = (k: string) => searchParams.get(k) ?? "";

  const [category, setCategory] = useState<string>(initial("category"));
  const [eventType, setEventType] = useState<string>(initial("event_type"));
  const [outcome, setOutcome] = useState<string>(initial("outcome"));
  const [actorLogin, setActorLogin] = useState<string>(initial("actor_login"));
  const [resourceType, setResourceType] = useState<string>(
    initial("resource_type"),
  );
  const [resourceRef, setResourceRef] = useState<string>(
    initial("resource_ref"),
  );
  const [httpMethod, setHttpMethod] = useState<string>(initial("http_method"));
  const [httpStatus, setHttpStatus] = useState<string>(initial("http_status"));
  const [httpPath, setHttpPath] = useState<string>(initial("http_path"));
  const [requestId, setRequestId] = useState<string>(initial("request_id"));
  const [createdAfter, setCreatedAfter] = useState<string>(
    initial("created_after"),
  );
  const [createdBefore, setCreatedBefore] = useState<string>(
    initial("created_before"),
  );
  const [page, setPage] = useState<number>(Number(initial("page")) || 1);
  const [pageSize, setPageSize] = useState<number>(
    Number(initial("per_page")) || 50,
  );
  const [includeTotal, setIncludeTotal] = useState<boolean>(false);
  const [expanded, setExpanded] = useState<Set<number>>(new Set());

  const queryParams = useMemo(
    () => ({
      category: (category || undefined) as AuditCategory | undefined,
      eventType: eventType || undefined,
      outcome: (outcome || undefined) as AuditOutcome | undefined,
      actorLogin: actorLogin || undefined,
      resourceType: resourceType || undefined,
      resourceRef: resourceRef || undefined,
      httpMethod: httpMethod || undefined,
      httpStatus: httpStatus ? Number(httpStatus) : undefined,
      httpPath: httpPath || undefined,
      requestId: requestId || undefined,
      createdAfter: createdAfter || undefined,
      createdBefore: createdBefore || undefined,
      includeTotal,
      page,
      perPage: pageSize,
    }),
    [
      category,
      eventType,
      outcome,
      actorLogin,
      resourceType,
      resourceRef,
      httpMethod,
      httpStatus,
      httpPath,
      requestId,
      createdAfter,
      createdBefore,
      includeTotal,
      page,
      pageSize,
    ],
  );

  const { data, isLoading, isFetching, error } = useAuditEvents(queryParams);

  const events: AuditEventSummary[] = data?.items ?? [];
  const pagination = data?.pagination;

  const hasActiveFilters =
    !!category ||
    !!eventType ||
    !!outcome ||
    !!actorLogin ||
    !!resourceType ||
    !!resourceRef ||
    !!httpMethod ||
    !!httpStatus ||
    !!httpPath ||
    !!requestId ||
    !!createdAfter ||
    !!createdBefore;

  const applyFilters = () => {
    setPage(1);
    const next = new URLSearchParams();
    if (category) next.set("category", category);
    if (eventType) next.set("event_type", eventType);
    if (outcome) next.set("outcome", outcome);
    if (actorLogin) next.set("actor_login", actorLogin);
    if (resourceType) next.set("resource_type", resourceType);
    if (resourceRef) next.set("resource_ref", resourceRef);
    if (httpMethod) next.set("http_method", httpMethod);
    if (httpStatus) next.set("http_status", httpStatus);
    if (httpPath) next.set("http_path", httpPath);
    if (requestId) next.set("request_id", requestId);
    if (createdAfter) next.set("created_after", createdAfter);
    if (createdBefore) next.set("created_before", createdBefore);
    setSearchParams(next);
  };

  const clearFilters = () => {
    setCategory("");
    setEventType("");
    setOutcome("");
    setActorLogin("");
    setResourceType("");
    setResourceRef("");
    setHttpMethod("");
    setHttpStatus("");
    setHttpPath("");
    setRequestId("");
    setCreatedAfter("");
    setCreatedBefore("");
    setPage(1);
    setSearchParams(new URLSearchParams());
  };

  const toggleExpanded = (id: number) => {
    setExpanded((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  };

  return (
    <div className="p-6 max-w-screen-2xl mx-auto">
      <div className="flex items-center justify-between mb-6">
        <div className="flex items-center gap-3">
          <ScrollText className="w-7 h-7 text-slate-700" />
          <div>
            <h1 className="text-2xl font-semibold text-gray-900">Audit Log</h1>
            <p className="text-sm text-gray-600">
              Searchable, filterable record of security and lifecycle events.
            </p>
          </div>
        </div>
        <label className="flex items-center gap-2 text-sm text-gray-700">
          <input
            type="checkbox"
            checked={includeTotal}
            onChange={(e) => setIncludeTotal(e.target.checked)}
          />
          Show exact totals (slower)
        </label>
      </div>

      <div className="grid grid-cols-1 lg:grid-cols-[280px_1fr] gap-6">
        <aside className="bg-white shadow rounded-lg p-4 space-y-3 self-start lg:sticky lg:top-4">
          <div className="flex items-center justify-between">
            <h2 className="font-semibold text-gray-900">Filters</h2>
            {hasActiveFilters && (
              <button
                onClick={clearFilters}
                className="text-xs text-gray-600 hover:text-gray-900 flex items-center gap-1"
              >
                <X className="w-3 h-3" /> clear
              </button>
            )}
          </div>

          <Field label="Category">
            <select
              value={category}
              onChange={(e) => setCategory(e.target.value)}
              className={FILTER_INPUT}
            >
              <option value="">all</option>
              {CATEGORIES.map((c) => (
                <option key={c} value={c}>
                  {c}
                </option>
              ))}
            </select>
          </Field>

          <Field label="Outcome">
            <select
              value={outcome}
              onChange={(e) => setOutcome(e.target.value)}
              className={FILTER_INPUT}
            >
              <option value="">all</option>
              {OUTCOMES.map((o) => (
                <option key={o} value={o}>
                  {o}
                </option>
              ))}
            </select>
          </Field>

          <Field label="Event type">
            <input
              value={eventType}
              onChange={(e) => setEventType(e.target.value)}
              placeholder="auth.login.success"
              className={FILTER_INPUT}
            />
          </Field>

          <Field label="Actor login">
            <input
              value={actorLogin}
              onChange={(e) => setActorLogin(e.target.value)}
              placeholder="alice"
              className={FILTER_INPUT}
            />
          </Field>

          <Field label="Resource type">
            <input
              value={resourceType}
              onChange={(e) => setResourceType(e.target.value)}
              placeholder="key, execution, pack..."
              className={FILTER_INPUT}
            />
          </Field>

          <Field label="Resource ref">
            <input
              value={resourceRef}
              onChange={(e) => setResourceRef(e.target.value)}
              placeholder="my_pack.my_action"
              className={FILTER_INPUT}
            />
          </Field>

          <Field label="HTTP method">
            <select
              value={httpMethod}
              onChange={(e) => setHttpMethod(e.target.value)}
              className={FILTER_INPUT}
            >
              <option value="">any</option>
              {HTTP_METHODS.map((m) => (
                <option key={m} value={m}>
                  {m}
                </option>
              ))}
            </select>
          </Field>

          <Field label="HTTP status">
            <input
              type="number"
              value={httpStatus}
              onChange={(e) => setHttpStatus(e.target.value)}
              placeholder="401"
              className={FILTER_INPUT}
            />
          </Field>

          <Field label="HTTP path contains">
            <input
              value={httpPath}
              onChange={(e) => setHttpPath(e.target.value)}
              placeholder="/api/v1/keys"
              className={FILTER_INPUT}
            />
          </Field>

          <Field label="Request id">
            <input
              value={requestId}
              onChange={(e) => setRequestId(e.target.value)}
              placeholder="UUID"
              className={FILTER_INPUT}
            />
          </Field>

          <Field label="Created after">
            <input
              type="datetime-local"
              value={createdAfter}
              onChange={(e) => setCreatedAfter(e.target.value)}
              className={FILTER_INPUT}
            />
          </Field>

          <Field label="Created before">
            <input
              type="datetime-local"
              value={createdBefore}
              onChange={(e) => setCreatedBefore(e.target.value)}
              className={FILTER_INPUT}
            />
          </Field>

          <button
            onClick={applyFilters}
            className="w-full mt-2 inline-flex items-center justify-center gap-2 px-3 py-2 bg-slate-800 text-white rounded text-sm hover:bg-slate-900"
          >
            <Search className="w-4 h-4" /> Apply filters
          </button>
        </aside>

        <section>
          {isLoading && events.length === 0 ? (
            <div className="bg-white shadow rounded-lg flex items-center justify-center h-64">
              <div className="animate-spin rounded-full h-10 w-10 border-b-2 border-slate-700" />
              <p className="ml-4 text-gray-600">Loading audit events...</p>
            </div>
          ) : error ? (
            <div className="bg-white shadow rounded-lg p-8 text-center">
              <p className="text-red-600 font-medium">
                Failed to load audit events
              </p>
              <p className="text-sm text-gray-600 mt-2">
                {(error as Error).message}
              </p>
            </div>
          ) : events.length === 0 ? (
            <div className="bg-white shadow rounded-lg p-12 text-center">
              <ScrollText className="w-10 h-10 text-gray-400 mx-auto mb-3" />
              <p className="text-gray-700 font-medium">No audit events found</p>
              <p className="text-sm text-gray-500 mt-2">
                {hasActiveFilters
                  ? "Try clearing some filters."
                  : "No events have been recorded yet."}
              </p>
            </div>
          ) : (
            <div className="bg-white shadow rounded-lg overflow-hidden">
              <table className="w-full">
                <thead className="bg-gray-50 text-xs uppercase text-gray-600">
                  <tr>
                    <th className="px-3 py-2 text-left w-8" />
                    <th className="px-3 py-2 text-left">When</th>
                    <th className="px-3 py-2 text-left">Category</th>
                    <th className="px-3 py-2 text-left">Event</th>
                    <th className="px-3 py-2 text-left">Outcome</th>
                    <th className="px-3 py-2 text-left">Actor</th>
                    <th className="px-3 py-2 text-left">Resource</th>
                    <th className="px-3 py-2 text-left">HTTP</th>
                  </tr>
                </thead>
                <tbody className="divide-y divide-gray-200 text-sm">
                  {events.map((e) => {
                    const isOpen = expanded.has(e.id);
                    return (
                      <Fragment key={e.id}>
                        <tr
                          className="hover:bg-gray-50 cursor-pointer"
                          onClick={() => toggleExpanded(e.id)}
                        >
                          <td className="px-3 py-2">
                            {isOpen ? (
                              <ChevronDown className="w-4 h-4 text-gray-500" />
                            ) : (
                              <ChevronRight className="w-4 h-4 text-gray-500" />
                            )}
                          </td>
                          <td className="px-3 py-2 whitespace-nowrap text-gray-900">
                            {formatDate(e.created)}
                          </td>
                          <td className="px-3 py-2">
                            <span
                              className={`px-2 py-0.5 rounded text-xs font-medium ${categoryClass(
                                e.category,
                              )}`}
                            >
                              {e.category}
                            </span>
                          </td>
                          <td className="px-3 py-2 font-mono text-xs text-gray-800">
                            {e.event_type}
                          </td>
                          <td className="px-3 py-2">
                            <span
                              className={`px-2 py-0.5 rounded text-xs font-medium ${outcomeClass(
                                e.outcome,
                              )}`}
                            >
                              {e.outcome}
                            </span>
                          </td>
                          <td className="px-3 py-2 text-gray-700">
                            {e.actor_login ?? (
                              <span className="text-gray-400">—</span>
                            )}
                          </td>
                          <td className="px-3 py-2 text-gray-700">
                            {e.resource_type ? (
                              <span>
                                <span className="text-gray-500">
                                  {e.resource_type}
                                </span>
                                {e.resource_ref && (
                                  <span className="ml-1 font-mono text-xs">
                                    {e.resource_ref}
                                  </span>
                                )}
                              </span>
                            ) : (
                              <span className="text-gray-400">—</span>
                            )}
                          </td>
                          <td className="px-3 py-2 text-gray-700 font-mono text-xs">
                            {e.http_method && e.http_status
                              ? `${e.http_method} ${e.http_status}`
                              : "—"}
                          </td>
                        </tr>
                        {isOpen && (
                          <tr className="bg-gray-50">
                            <td />
                            <td colSpan={7} className="px-3 py-3">
                              <ExpandedRow event={e} />
                            </td>
                          </tr>
                        )}
                      </Fragment>
                    );
                  })}
                </tbody>
              </table>

              <div className="px-4 py-3 border-t border-gray-200 bg-gray-50">
                <Pagination
                  page={page}
                  setPage={setPage}
                  pageSize={pageSize}
                  itemCount={events.length}
                  total={pagination?.total_items ?? undefined}
                  hasPrevious={pagination?.has_previous}
                  hasNext={pagination?.has_next}
                  itemLabel="audit events"
                />
                <div className="mt-2 text-xs text-gray-500 flex items-center gap-3">
                  <span>
                    Page size:{" "}
                    <select
                      className="text-xs border border-gray-300 rounded px-1 py-0.5"
                      value={pageSize}
                      onChange={(e) => {
                        setPageSize(Number(e.target.value));
                        setPage(1);
                      }}
                    >
                      {[25, 50, 100, 200].map((n) => (
                        <option key={n} value={n}>
                          {n}
                        </option>
                      ))}
                    </select>
                  </span>
                  {isFetching && <span>Refreshing…</span>}
                </div>
              </div>
            </div>
          )}
        </section>
      </div>
    </div>
  );
}

function Field({
  label,
  children,
}: {
  label: string;
  children: React.ReactNode;
}) {
  return (
    <label className="block">
      <span className="block text-xs font-medium text-gray-600 mb-1">
        {label}
      </span>
      {children}
    </label>
  );
}

function ExpandedRow({ event }: { event: AuditEventSummary }) {
  return (
    <div className="text-xs text-gray-700 grid grid-cols-2 md:grid-cols-3 gap-2">
      <KV k="ID" v={event.id.toString()} />
      <KV k="HTTP Method" v={event.http_method ?? "—"} mono />
      <KV k="HTTP Path" v={event.http_path ?? "—"} mono />
      {event.request_id && <KV k="Request ID" v={event.request_id} mono />}
    </div>
  );
}

function KV({ k, v, mono }: { k: string; v: string; mono?: boolean }) {
  return (
    <div>
      <div className="text-gray-500">{k}</div>
      <div className={mono ? "font-mono break-all" : "break-words"}>{v}</div>
    </div>
  );
}
