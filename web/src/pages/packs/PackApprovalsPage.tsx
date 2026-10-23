import { useState } from "react";
import { Link, useParams } from "react-router-dom";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { PermissionsService } from "@/api/services/PermissionsService";
import type { IssueRuntimeApprovalRequest } from "@/api/models/IssueRuntimeApprovalRequest";
import type { PolicyInputApprovalRequest } from "@/api/models/PolicyInputApprovalRequest";
import type { DeclaredInputApprovalTarget } from "@/api/models/DeclaredInputApprovalTarget";
import { InputApprovalDiscovery } from "./InputApprovalDiscovery";
import { approvalErrorText } from "./approval-errors";
import {
  parseApprovalRequest,
  parseInputConsentRequest,
} from "./approval-request";
import {
  approvalPurpose,
  approvalPurposeLabel,
  type ApprovalPurposeView,
} from "./approval-purpose";

type EditMode =
  | { kind: "runtime_issue" }
  | {
      kind: "input_issue";
      selection:
        | { kind: "none" }
        | { kind: "selected"; target: DeclaredInputApprovalTarget };
    }
  | { kind: "runtime_replace"; id: number }
  | {
      kind: "input_replace";
      id: number;
      targets: Extract<
        ApprovalPurposeView,
        { kind: "declared_input" }
      >["targets"];
    };

type SaveRequest =
  | { kind: "runtime_issue"; body: IssueRuntimeApprovalRequest }
  | { kind: "input_issue"; body: PolicyInputApprovalRequest }
  | { kind: "replace"; id: number; body: IssueRuntimeApprovalRequest };

const EMPTY_DRAFT = '{\n  "grants": [],\n  "expires_at": null\n}';

function ApprovalControls({ packRef }: { packRef: string }) {
  const client = useQueryClient();
  const [page, setPage] = useState(1);
  const [mode, setMode] = useState<EditMode>({ kind: "runtime_issue" });
  const [confirmed, setConfirmed] = useState(false);
  const [revokeApproval, setRevokeApproval] = useState<{
    id: number;
    purpose: ApprovalPurposeView["kind"];
  } | null>(null);
  const [text, setText] = useState(EMPTY_DRAFT);
  const [validationError, setValidationError] = useState<string | null>(null);
  const approvals = useQuery({
    queryKey: ["packApprovals", packRef, page],
    queryFn: () =>
      PermissionsService.listRuntimeApprovals({ packRef, page, pageSize: 25 }),
  });
  const refresh = () =>
    client.invalidateQueries({ queryKey: ["packApprovals", packRef] });
  const save = useMutation({
    mutationFn: (request: SaveRequest) => {
      switch (request.kind) {
        case "runtime_issue":
          return PermissionsService.issueRuntimeApproval({
            packRef,
            requestBody: request.body,
          });
        case "input_issue":
          return PermissionsService.issueDeclaredInputApproval({
            packRef,
            requestBody: request.body,
          });
        case "replace":
          return PermissionsService.replaceRuntimeApproval({
            packRef,
            id: request.id,
            requestBody: request.body,
          });
      }
    },
    onSuccess: () => {
      setMode({ kind: "runtime_issue" });
      setConfirmed(false);
      setText(EMPTY_DRAFT);
      void refresh();
    },
  });
  const revoke = useMutation({
    mutationFn: (id: number) =>
      PermissionsService.revokeRuntimeApproval({ packRef, id }),
    onSuccess: () => {
      setRevokeApproval(null);
      void refresh();
    },
  });
  const replacing =
    mode.kind === "runtime_replace" || mode.kind === "input_replace";
  const inputOnly =
    mode.kind === "input_issue" || mode.kind === "input_replace";
  const busy = save.isPending || revoke.isPending;
  const start = (next: EditMode, draft = EMPTY_DRAFT) => {
    setMode(next);
    setText(draft);
    setConfirmed(false);
    setValidationError(null);
    save.reset();
  };
  const submit = () => {
    setValidationError(null);
    try {
      if (replacing && !confirmed)
        throw new Error(
          "Confirm revocation of the previous approval before replacing it.",
        );
      const body = inputOnly
        ? parseInputConsentRequest(text)
        : parseApprovalRequest(text);
      switch (mode.kind) {
        case "runtime_issue":
          save.mutate({ kind: "runtime_issue", body });
          break;
        case "runtime_replace":
        case "input_replace":
          save.mutate({ kind: "replace", id: mode.id, body });
          break;
        case "input_issue": {
          if (mode.selection.kind !== "selected")
            throw new Error(
              "Select one authorized retained input target before issuing consent.",
            );
          const target = mode.selection.target;
          save.mutate({
            kind: "input_issue",
            body: {
              grants: body.grants,
              expires_at: body.expires_at,
              policy_revision: target.policy_revision,
              invocation_binding: target.invocation_binding,
              declaration_revision: target.declaration_revision,
              source_path: target.source_path,
              expression: target.expression,
            },
          });
          break;
        }
      }
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
      <h1 className="text-2xl font-semibold">External approvals</h1>
      <p>
        Approvals have fixed scope and optional expiry. Disabling an issuer's
        login does not revoke them. Issuing or changing authority requires
        current covering grants or permission-administration authority.
      </p>
      <p className="text-amber-800">
        Replacing an approval explicitly revokes its predecessor. Existing
        executions do not automatically acquire the new approval. Input-only
        consent never grants runtime API authority.
      </p>
      {approvals.isPending && <p>Loading approvals...</p>}
      {approvals.error && (
        <p role="alert">{approvalErrorText(approvals.error)}</p>
      )}
      <ul className="space-y-3">
        {approvals.data?.items.map((approval) => {
          const purpose = approvalPurpose(approval.purpose);
          return (
            <li key={approval.id} className="border rounded p-4 space-y-2">
              <h2 className="font-semibold">
                Approval {approval.id}:{" "}
                {approval.active
                  ? "active"
                  : approval.revoked_at
                    ? "revoked"
                    : "expired"}
              </h2>
              <p>Purpose: {approvalPurposeLabel(purpose.kind)}</p>
              {purpose.kind === "declared_input" && (
                <section
                  aria-label={`Input targets for approval ${approval.id}`}
                >
                  <h3 className="font-semibold">
                    Visible immutable input targets
                  </h3>
                  {!purpose.metadataAvailable ? (
                    <p>
                      Target metadata is unavailable. This consent remains
                      input-only.
                    </p>
                  ) : purpose.targets.length === 0 ? (
                    <p>
                      No currently visible retained targets. Targets may be
                      collected or hidden by current visibility. This consent
                      remains input-only.
                    </p>
                  ) : (
                    <ul>
                      {purpose.targets.map((target, index) => (
                        <li key={index}>
                          <pre className="overflow-auto">
                            {JSON.stringify(target, null, 2)}
                          </pre>
                        </li>
                      ))}
                    </ul>
                  )}
                  <p>
                    These are authored declarations, not evaluated values or
                    invocation permits. New consent requires authorized target
                    discovery; it cannot be issued from this ledger row.
                  </p>
                </section>
              )}
              {purpose.kind === "unavailable" && (
                <p>
                  Purpose metadata is unavailable. Runtime replacement is
                  disabled.
                </p>
              )}
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
                  {(purpose.kind === "runtime" ||
                    (purpose.kind === "declared_input" &&
                      purpose.metadataAvailable &&
                      purpose.targets.length > 0)) && (
                    <button
                      type="button"
                      className="text-blue-600"
                      disabled={busy}
                      onClick={() => {
                        const next: EditMode =
                          purpose.kind === "declared_input"
                            ? {
                                kind: "input_replace",
                                id: approval.id,
                                targets: purpose.targets,
                              }
                            : { kind: "runtime_replace", id: approval.id };
                        start(
                          next,
                          JSON.stringify(
                            {
                              grants: approval.grants,
                              expires_at: approval.expires_at ?? null,
                            },
                            null,
                            2,
                          ),
                        );
                      }}
                    >
                      Replace approval {approval.id}
                    </button>
                  )}
                  {purpose.kind === "declared_input" &&
                    (!purpose.metadataAvailable ||
                      purpose.targets.length === 0) && (
                      <p>
                        Cannot replace without verified visible retained target
                        metadata. Discover a target to issue separate new
                        consent instead.
                      </p>
                    )}
                  <button
                    type="button"
                    className="text-red-700"
                    disabled={busy}
                    onClick={() =>
                      setRevokeApproval({
                        id: approval.id,
                        purpose: purpose.kind,
                      })
                    }
                  >
                    Revoke approval {approval.id}
                  </button>
                </div>
              )}
            </li>
          );
        })}
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
      <nav aria-label="New approval purpose" className="flex gap-3">
        <button
          type="button"
          disabled={busy}
          onClick={() => start({ kind: "runtime_issue" })}
        >
          New runtime approval
        </button>
        <button
          type="button"
          disabled={busy}
          onClick={() =>
            start({ kind: "input_issue", selection: { kind: "none" } })
          }
        >
          New input-only consent
        </button>
      </nav>
      {mode.kind === "input_issue" && (
        <InputApprovalDiscovery
          packRef={packRef}
          disabled={busy}
          onSelect={(target) => {
            setMode({
              kind: "input_issue",
              selection: { kind: "selected", target },
            });
            setValidationError(null);
            save.reset();
          }}
        />
      )}
      <section className="border rounded p-4 space-y-3">
        <h2 className="text-xl font-semibold">
          {mode.kind === "runtime_issue"
            ? "Issue runtime approval"
            : mode.kind === "input_issue"
              ? "Issue input-only consent"
              : mode.kind === "input_replace"
                ? `Replace input-only approval ${mode.id}`
                : `Replace runtime approval ${mode.id}`}
        </h2>
        <p>
          {inputOnly
            ? "Input-only consent never selects execution API permissions. Fixed grants cover only the retained input read; they are not pack association, SDK permission sets or an invocation permit. The server validates issuer coverage and the fixed consent scope."
            : "This editor grants runtime API authority only."}
        </p>
        {mode.kind === "input_issue" &&
          (mode.selection.kind === "none" ? (
            <p>No input target selected.</p>
          ) : (
            <section aria-label="Selected input target">
              <h3>Selected immutable target</h3>
              <pre className="overflow-auto">
                {JSON.stringify(mode.selection.target, null, 2)}
              </pre>
              <p>
                This selection stays pinned even if the current policy changes.
                Choose a newly discovered target for readmission to a new
                declaration.
              </p>
            </section>
          ))}
        {mode.kind === "input_replace" && (
          <section aria-label="Replacement retained targets">
            <h3>Unchanged visible retained input targets</h3>
            {mode.targets.map((target, index) => (
              <pre className="overflow-auto" key={index}>
                {JSON.stringify(target, null, 2)}
              </pre>
            ))}
            <p>
              Replacement preserves these declarations. Only fixed grants and
              expiry can change; it cannot rebind to a current policy, renderer
              or new target. Fresh consent requires authorized discovery.
            </p>
          </section>
        )}
        <label className="block">
          Fixed grants and optional expiry
          <textarea
            className="block border rounded w-full font-mono p-3"
            rows={10}
            disabled={busy}
            value={text}
            onChange={(event) => setText(event.target.value)}
          />
        </label>
        {replacing && (
          <label className="block">
            <input
              type="checkbox"
              checked={confirmed}
              disabled={busy}
              onChange={(event) => setConfirmed(event.target.checked)}
            />{" "}
            {inputOnly
              ? "I confirm revocation of the previous input-only consent and its admitted input reads."
              : "I confirm revocation of the previous approval and its admitted effects."}
          </label>
        )}
        {validationError && <p role="alert">{validationError}</p>}
        {save.error && <p role="alert">{approvalErrorText(save.error)}</p>}
        <button
          type="button"
          disabled={
            busy ||
            (replacing && !confirmed) ||
            (mode.kind === "input_issue" && mode.selection.kind === "none")
          }
          onClick={submit}
          className="bg-blue-600 text-white rounded px-4 py-2"
        >
          {save.isPending
            ? "Saving..."
            : replacing
              ? "Replace and revoke previous"
              : inputOnly
                ? "Issue input-only consent"
                : "Issue approval"}
        </button>
        {replacing && (
          <button
            type="button"
            disabled={busy}
            onClick={() => start({ kind: "runtime_issue" })}
          >
            Cancel replacement
          </button>
        )}
      </section>
      {revokeApproval !== null && (
        <div
          role="dialog"
          aria-modal="true"
          aria-label="Confirm approval revocation"
          className="border border-red-700 rounded p-4 space-y-3"
        >
          <p>
            Revoke approval {revokeApproval.id}? Purpose:{" "}
            {approvalPurposeLabel(revokeApproval.purpose)}.
            {revokeApproval.purpose === "declared_input"
              ? " This revokes its input-only consent, not a runtime permission selection."
              : revokeApproval.purpose === "runtime"
                ? " This stops its admitted runtime effects."
                : " This revokes the fixed approval regardless of its unavailable purpose metadata."}
            The operation cannot be undone.
          </p>
          {revoke.error && (
            <p role="alert">{approvalErrorText(revoke.error)}</p>
          )}
          <button
            type="button"
            disabled={revoke.isPending}
            onClick={() => revoke.mutate(revokeApproval.id)}
            className="text-red-700"
          >
            Confirm revocation
          </button>
          <button
            type="button"
            disabled={revoke.isPending}
            onClick={() => setRevokeApproval(null)}
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
  return <ApprovalControls key={packRef} packRef={packRef} />;
}
