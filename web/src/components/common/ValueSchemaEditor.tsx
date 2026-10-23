import { useId, useRef, useState } from "react";
import { SchemaTypesService } from "@/api/services/SchemaTypesService";
import { SchemaPreviewContext } from "@/api/models/SchemaPreviewContext";
import SchemaTypePicker from "./SchemaTypePicker";

interface Props {
  value: unknown;
  onChange: (value: unknown) => void;
  onError: (message: string | null) => void;
  onDigest: (digest: string | undefined) => void;
}

export default function ValueSchemaEditor({
  value,
  onChange,
  onError,
  onDigest,
}: Props) {
  const id = useId();
  const [text, setText] = useState(() =>
    value == null ? "" : JSON.stringify(value, null, 2),
  );
  const [preview, setPreview] = useState<unknown>(undefined);
  const [loading, setLoading] = useState(false);
  const generation = useRef(0);
  const change = (next: string) => {
    generation.current += 1;
    setText(next);
    setPreview(undefined);
    onDigest(undefined);
    onError(null);
    if (!next.trim()) {
      onChange(null);
      return;
    }
    try {
      const schema: unknown = JSON.parse(next);
      if (
        typeof schema !== "boolean" &&
        (typeof schema !== "object" || schema === null || Array.isArray(schema))
      )
        throw new Error("A value schema must be an object or boolean");
      onChange(schema);
    } catch {
      onError(
        "Enter a valid single-value JSON schema, or leave it empty to remove the schema.",
      );
    }
  };
  const type =
    typeof value === "object" &&
    value !== null &&
    "type" in value &&
    typeof value.type === "string"
      ? value.type
      : "json";
  const inspect = async () => {
    const selectedGeneration = generation.current;
    setLoading(true);
    onError(null);
    try {
      const schema: unknown = JSON.parse(text);
      const response = await SchemaTypesService.previewSchema({
        requestBody: { context: SchemaPreviewContext.SINGLE_VALUE, schema },
      });
      if (generation.current === selectedGeneration) {
        setPreview(response.data.contract);
        onDigest(response.data.semantic_digest);
      }
    } catch {
      if (generation.current === selectedGeneration)
        onError(
          "Schema preview failed. Check its references and constraints before saving.",
        );
    } finally {
      setLoading(false);
    }
  };
  return (
    <div className="space-y-2">
      <label htmlFor={id} className="block text-sm font-medium">
        Schema for value
      </label>
      <p className="text-xs text-gray-600">
        Optional single-value schema. Empty means no schema. Named types remain
        authored references; the server selects revisions when assigning or
        rebinding.
      </p>
      <SchemaTypePicker
        value={type}
        onChange={(type) => change(JSON.stringify({ type }, null, 2))}
      />
      <textarea
        id={id}
        rows={5}
        value={text}
        onChange={(event) => change(event.target.value)}
        className="w-full border rounded p-2 font-mono text-xs"
      />
      <button
        type="button"
        disabled={loading || !text.trim()}
        onClick={inspect}
        className="text-sm text-blue-600"
      >
        {loading ? "Resolving schema..." : "Preview installed contract"}
      </button>
      {preview !== undefined && (
        <details>
          <summary className="text-sm">Resolved graph preview</summary>
          <pre className="max-h-64 overflow-auto text-xs">
            {JSON.stringify(preview, null, 2)}
          </pre>
        </details>
      )}
    </div>
  );
}
