import { useEffect, useState } from "react";
import { SchemaTypesService } from "@/api/services/SchemaTypesService";
import { SchemaPreviewContext } from "@/api/models/SchemaPreviewContext";
import { ContractValueRows } from "./ContractDataCard";
import type { ParamSchema } from "./ParamSchemaForm";
export type { ParamSchema };

interface ParamSchemaDisplayProps {
  schema: ParamSchema;
  values: Record<string, unknown>;
  className?: string;
  emptyMessage?: string;
}

/** Mutable rule selectors use a current authoring preview, not a runtime binding. */
export default function ParamSchemaDisplay({
  schema,
  values,
  className = "",
  emptyMessage = "No parameters configured",
}: ParamSchemaDisplayProps) {
  const [resolution, setResolution] = useState<
    | { schema: ParamSchema; kind: "resolved"; contract: unknown }
    | { schema: ParamSchema; kind: "failed" }
  >();
  useEffect(() => {
    let current = true;
    const request = SchemaTypesService.previewSchema({
      requestBody: { context: SchemaPreviewContext.FLAT_DOCUMENT, schema },
    });
    request
      .then((response) => {
        if (current)
          setResolution({
            schema,
            kind: "resolved",
            contract: response.data.contract,
          });
      })
      .catch(() => {
        if (current) setResolution({ schema, kind: "failed" });
      });
    return () => {
      current = false;
      request.cancel();
    };
  }, [schema]);
  if (resolution?.schema !== schema) {
    return (
      <p className={className}>
        Loading schema annotations. Values remain hidden.
      </p>
    );
  }
  if (resolution.kind === "failed") {
    return (
      <p role="alert" className={className}>
        Schema annotations are unavailable. Values remain hidden.
      </p>
    );
  }
  return (
    <div className={className}>
      <ContractValueRows
        contract={resolution.contract}
        values={values}
        emptyMessage={emptyMessage}
        maskSecrets
      />
    </div>
  );
}
