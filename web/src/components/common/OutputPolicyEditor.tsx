import { useId } from "react";
import type { OutputPolicy } from "@/api";

export default function OutputPolicyEditor({
  value,
  onChange,
  structured = true,
}: {
  value: OutputPolicy;
  onChange: (policy: OutputPolicy) => void;
  structured?: boolean;
}) {
  const id = useId();
  return (
    <fieldset className="space-y-3 rounded-lg border border-gray-200 p-3">
      <legend className="px-1 text-sm font-semibold text-gray-900">
        Output visibility
      </legend>
      <p className="text-xs text-gray-600">
        Mark secret fields in the output schema to hide only those values.
        Secret input parameters do not make public output secret.
      </p>
      {(["stdout", "stderr", "error"] as const).map((channel) => (
        <div key={channel}>
          <label
            htmlFor={`${id}-${channel}`}
            className="mb-1 block text-xs font-medium text-gray-700"
          >
            {channel === "error"
              ? "Error text"
              : channel === "stdout"
                ? "Stdout"
                : "Stderr"}
          </label>
          <select
            id={`${id}-${channel}`}
            value={value[channel] ?? ""}
            onChange={(event) => {
              const next = { ...value, [channel]: event.target.value };
              if (!event.target.value) delete next[channel];
              onChange(next);
            }}
            className="w-full rounded-md border border-gray-300 px-2 py-2 text-sm"
          >
            <option value="">Default from output schema</option>
            <option value="public">Public</option>
            <option value="secret">Secret — reveal on request</option>
            {channel === "stdout" && (
              <option value="sanitized" disabled={!structured}>
                Sanitized — hide schema secret fields
              </option>
            )}
          </select>
        </div>
      ))}
      <p className="text-xs text-gray-500">
        Without secret output fields, defaults are public. With secret fields,
        structured stdout defaults to sanitized and stderr/error text to secret.
        Raw public stdout is rejected when it would duplicate secret output.
        Sanitized stdout requires JSON, YAML, or JSONL output.
      </p>
    </fieldset>
  );
}
