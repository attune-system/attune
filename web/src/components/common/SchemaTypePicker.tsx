import { useEffect, useId, useState } from "react";
import { SchemaTypesService } from "@/api/services/SchemaTypesService";
import type { SchemaTypeResponse } from "@/api/models/SchemaTypeResponse";

const PRIMITIVES = [
  "string",
  "number",
  "integer",
  "boolean",
  "array",
  "object",
  "null",
];

interface Props {
  value: string;
  onChange: (value: string) => void;
  disabled?: boolean;
}

export default function SchemaTypePicker({
  value,
  onChange,
  disabled = false,
}: Props) {
  const id = useId();
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");
  const [page, setPage] = useState(1);
  const [state, setState] = useState<
    | { kind: "loading" }
    | { kind: "error" }
    | { kind: "ready"; types: SchemaTypeResponse[]; hasNext: boolean }
  >({ kind: "loading" });

  useEffect(() => {
    if (!open || disabled) return;
    let active = true;
    const request = SchemaTypesService.listSchemaTypes({
      q: query,
      page,
      pageSize: 20,
    });
    request
      .then((response) => {
        if (active)
          setState({
            kind: "ready",
            types: response.items,
            hasNext: response.pagination.has_next,
          });
      })
      .catch(() => {
        if (active) setState({ kind: "error" });
      });
    return () => {
      active = false;
      request.cancel();
    };
  }, [open, disabled, query, page]);

  return (
    <div className="space-y-2">
      <select
        aria-label="Schema type"
        value={value}
        disabled={disabled}
        className="w-full px-2 py-1 text-sm border rounded"
        onChange={(event) => onChange(event.target.value)}
      >
        {PRIMITIVES.map((type) => (
          <option key={type} value={type}>
            {type}
          </option>
        ))}
        {!PRIMITIVES.includes(value) && (
          <option value={value}>
            {value === "json" ? "Reference, union, or composed schema" : value}
          </option>
        )}
      </select>
      {!disabled && (
        <button
          type="button"
          className="text-sm text-blue-600"
          onClick={() => {
            setState({ kind: "loading" });
            setOpen(!open);
          }}
        >
          Choose installed named type
        </button>
      )}
      {open && !disabled && (
        <div className="border rounded p-2 space-y-2">
          <label htmlFor={id} className="text-xs">
            Search installed types
          </label>
          <input
            id={id}
            value={query}
            className="w-full border rounded p-1"
            onChange={(event) => {
              setState({ kind: "loading" });
              setQuery(event.target.value);
              setPage(1);
            }}
          />
          {state.kind === "loading" && (
            <p className="text-xs">Loading types...</p>
          )}
          {state.kind === "error" && (
            <p role="alert" className="text-xs text-red-700">
              Type discovery is unavailable. Authored references have not
              changed.
            </p>
          )}
          {state.kind === "ready" && (
            <>
              {state.types.length === 0 && (
                <p className="text-xs">No installed types match.</p>
              )}
              {state.types.map((type) => (
                <button
                  key={type.ref}
                  type="button"
                  className="block w-full text-left text-sm hover:bg-gray-50 p-1"
                  onClick={() => {
                    onChange(type.ref);
                    setOpen(false);
                  }}
                >
                  {type.ref} · {type.label}
                </button>
              ))}
              <div className="flex gap-3 text-xs">
                <button
                  type="button"
                  disabled={page <= 1}
                  onClick={() => {
                    setState({ kind: "loading" });
                    setPage(page - 1);
                  }}
                >
                  Previous
                </button>
                <span>Page {page}</span>
                <button
                  type="button"
                  disabled={!state.hasNext}
                  onClick={() => {
                    setState({ kind: "loading" });
                    setPage(page + 1);
                  }}
                >
                  Next
                </button>
              </div>
            </>
          )}
        </div>
      )}
    </div>
  );
}
