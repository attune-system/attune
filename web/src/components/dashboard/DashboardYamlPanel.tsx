import { useState } from "react";
import { Check, Copy } from "lucide-react";

interface DashboardYamlPanelProps {
  yamlText: string;
}

export function DashboardYamlPanel({ yamlText }: DashboardYamlPanelProps) {
  const [copied, setCopied] = useState(false);

  const copyYaml = async () => {
    try {
      await navigator.clipboard.writeText(yamlText);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1500);
    } catch {
      setCopied(false);
    }
  };

  return (
    <section className="rounded-lg border border-gray-200 bg-white">
      <div className="flex items-center justify-between border-b border-gray-200 px-4 py-3">
        <div>
          <h2 className="text-sm font-semibold text-gray-900">YAML view</h2>
          <p className="text-xs text-gray-500">
            Deterministic client-side representation of the current draft.
          </p>
        </div>
        <button
          type="button"
          onClick={() => void copyYaml()}
          className="inline-flex items-center gap-2 rounded border border-gray-300 px-3 py-1.5 text-sm text-gray-700 hover:bg-gray-50"
        >
          {copied ? <Check className="h-4 w-4 text-green-600" /> : <Copy className="h-4 w-4" />}
          {copied ? "Copied" : "Copy YAML"}
        </button>
      </div>
      <pre className="max-h-[32rem] overflow-auto bg-gray-950 px-4 py-3 text-xs text-gray-100">
        <code>{yamlText}</code>
      </pre>
    </section>
  );
}
