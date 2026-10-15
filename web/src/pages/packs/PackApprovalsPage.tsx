import { useState } from "react";
import { Link, useParams } from "react-router-dom";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { PermissionsService } from "@/api/services/PermissionsService";
import type { IssueRuntimeApprovalRequest } from "@/api/models/IssueRuntimeApprovalRequest";

function isObject(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

export function parseApprovalRequest(
  text: string,
): IssueRuntimeApprovalRequest {
  const value: unknown = JSON.parse(text);
  if (
    !isObject(value) ||
    Object.keys(value).some((key) => key !== "grants" && key !== "expires_at")
  ) {
    throw new Error("Use an object containing grants and optional expires_at.");
  }
  if (
    !Array.isArray(value.grants) ||
    value.grants.length === 0 ||
    value.grants.length > 256 ||
    !value.grants.every(isObject)
  ) {
    throw new Error("Provide 1 to 256 grant objects.");
  }
  for (const grant of value.grants) {
    if (grant.scope !== undefined && grant.scope !== "external") {
      throw new Error(
        "External approvals cannot contain pack-local scope declarations.",
      );
    }
    if (
      typeof grant.resource !== "string" ||
      !Array.isArray(grant.actions) ||
      grant.actions.length === 0 ||
      !grant.actions.every((action: unknown) => typeof action === "string")
    ) {
      throw new Error(
        "Each grant requires a resource and a nonempty actions array.",
      );
    }
  }
  const expiresAt = value.expires_at;
  if (expiresAt === undefined || expiresAt === null)
    return { grants: value.grants, expires_at: null };
  if (
    typeof expiresAt !== "string" ||
    !/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:Z|[+-]\d{2}:\d{2})$/.test(
      expiresAt,
    ) ||
    !Number.isFinite(Date.parse(expiresAt)) ||
    Date.parse(expiresAt) <= Date.now()
  ) {
    throw new Error("expires_at must be a future RFC3339 timestamp or null.");
  }
  return { grants: value.grants, expires_at: expiresAt };
}

type EditMode = { kind: "issue" } | { kind: "replace"; id: number };

function ApprovalControls({ packRef }: { packRef: string }) {
  const client = useQueryClient();
  const [page, setPage] = useState(1);
  const [mode, setMode] = useState<EditMode>({ kind: "issue" });
  const [confirmed, setConfirmed] = useState(false);
  const [revokeId, setRevokeId] = useState<number | null>(null);
  const [text, setText] = useState(
    '{\n  "grants": [],\n  "expires_at": null\n}',
  );
  const [validationError, setValidationError] = useState<string | null>(null);
  const approvals = useQuery({
    queryKey: ["packApprovals", packRef, page],
    queryFn: () =>
      PermissionsService.listRuntimeApprovals({ packRef, page, pageSize: 25 }),
  });
  const refresh = () =>
    client.invalidateQueries({ queryKey: ["packApprovals", packRef] });
  const save = useMutation({
    mutationFn: (requestBody: IssueRuntimeApprovalRequest) =>
      mode.kind === "issue"
        ? PermissionsService.issueRuntimeApproval({ packRef, requestBody })
        : PermissionsService.replaceRuntimeApproval({
            packRef,
            id: mode.id,
            requestBody,
          }),
    onSuccess: () => {
      setMode({ kind: "issue" });
      setConfirmed(false);
      void refresh();
    },
  });
  const revoke = useMutation({
    mutationFn: (id: number) =>
      PermissionsService.revokeRuntimeApproval({ packRef, id }),
    onSuccess: () => {
      setRevokeId(null);
      void refresh();
    },
  });
  const submit = () => {
    setValidationError(null);
    try {
      if (mode.kind === "replace" && !confirmed)
        throw new Error(
          "Confirm revocation of the previous approval before replacing it.",
        );
      save.mutate(parseApprovalRequest(text));
    } catch (error) {
      setValidationError(
        error instanceof Error ? error.message : "Invalid approval JSON.",
      );
    }
  };
  return (
    <div className="p-6 space-y-6">
      <Link
        to={`/packs/${encodeURIComponent(packRef)}`}
        className="text-blue-600"
      >
        Back to pack
      </Link>
      <h1 className="text-2xl font-semibold">External runtime approvals</h1>
      <p>
        Approvals have fixed scope and optional expiry. Disabling an issuer's
        login does not revoke them. Issuing or changing authority requires
        current covering grants or permission-administration authority.
      </p>
      <p className="text-amber-800">
        Replacing an approval explicitly revokes its predecessor. Existing
        executions do not automatically acquire the new approval.
      </p>
      {approvals.isPending && <p>Loading approvals...</p>}
      {approvals.error && <p role="alert">{approvals.error.message}</p>}
      <ul className="space-y-3">
        {approvals.data?.items.map((approval) => (
          <li key={approval.id} className="border rounded p-4 space-y-2">
            <h2 className="font-semibold">
              Approval {approval.id}:{" "}
              {approval.active
                ? "active"
                : approval.revoked_at
                  ? "revoked"
                  : "expired"}
            </h2>
            <p>
              Issued by {approval.issuer_login}. Expires{" "}
              {approval.expires_at ?? "never"}.
            </p>
            <details>
              <summary>Fixed scope and digest</summary>
              <pre className="overflow-auto">
                {JSON.stringify(approval.grants, null, 2)}
              </pre>
              <p className="break-all">{approval.approval_digest}</p>
            </details>
            {!approval.revoked_at && (
              <div className="flex gap-3">
                <button
                  type="button"
                  className="text-blue-600"
                  onClick={() => {
                    setMode({ kind: "replace", id: approval.id });
                    setConfirmed(false);
                    setValidationError(null);
                    setText(
                      JSON.stringify(
                        { grants: approval.grants, expires_at: null },
                        null,
                        2,
                      ),
                    );
                  }}
                >
                  Replace approval {approval.id}
                </button>
                <button
                  type="button"
                  className="text-red-700"
                  onClick={() => setRevokeId(approval.id)}
                >
                  Revoke approval {approval.id}
                </button>
              </div>
            )}
          </li>
        ))}
      </ul>
      {approvals.data && (
        <div className="flex gap-4">
          <button
            type="button"
            disabled={page === 1}
            onClick={() => setPage((current) => current - 1)}
          >
            Previous page
          </button>
          <span>Page {page}</span>
          <button
            type="button"
            disabled={!approvals.data.pagination.has_next}
            onClick={() => setPage((current) => current + 1)}
          >
            Next page
          </button>
        </div>
      )}
      <section className="border rounded p-4 space-y-3">
        <h2 className="text-xl font-semibold">
          {mode.kind === "issue"
            ? "Issue approval"
            : `Replace approval ${mode.id}`}
        </h2>
        <label className="block">
          Fixed grants and optional expiry
          <textarea
            className="block border rounded w-full font-mono p-3"
            rows={10}
            value={text}
            onChange={(event) => setText(event.target.value)}
          />
        </label>
        {mode.kind === "replace" && (
          <label className="block">
            <input
              type="checkbox"
              checked={confirmed}
              onChange={(event) => setConfirmed(event.target.checked)}
            />{" "}
            I confirm revocation of the previous approval and its admitted
            effects.
          </label>
        )}
        {validationError && <p role="alert">{validationError}</p>}
        {save.error && <p role="alert">{save.error.message}</p>}
        <button
          type="button"
          disabled={save.isPending || (mode.kind === "replace" && !confirmed)}
          onClick={submit}
          className="bg-blue-600 text-white rounded px-4 py-2"
        >
          {save.isPending
            ? "Saving..."
            : mode.kind === "issue"
              ? "Issue approval"
              : "Replace and revoke previous"}
        </button>
        {mode.kind === "replace" && (
          <button
            type="button"
            onClick={() => {
              setMode({ kind: "issue" });
              setConfirmed(false);
            }}
          >
            Cancel replacement
          </button>
        )}
      </section>
      {revokeId !== null && (
        <div
          role="dialog"
          aria-modal="true"
          aria-label="Confirm approval revocation"
          className="border border-red-700 rounded p-4 space-y-3"
        >
          <p>
            Revoke approval {revokeId}? This stops its admitted runtime effects.
            The operation cannot be undone.
          </p>
          {revoke.error && <p role="alert">{revoke.error.message}</p>}
          <button
            type="button"
            disabled={revoke.isPending}
            onClick={() => revoke.mutate(revokeId)}
            className="text-red-700"
          >
            Confirm revocation
          </button>
          <button
            type="button"
            disabled={revoke.isPending}
            onClick={() => setRevokeId(null)}
          >
            Cancel
          </button>
        </div>
      )}
    </div>
  );
}

export default function PackApprovalsPage() {
  const { ref: packRef } = useParams<{ ref: string }>();
  if (!packRef) return <p role="alert">Pack reference is required.</p>;
  return <ApprovalControls packRef={packRef} />;
}
