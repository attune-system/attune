import { useState } from "react";
import { useQuery } from "@tanstack/react-query";
import type { DeclaredInputApprovalTarget } from "@/api/models/DeclaredInputApprovalTarget";
import { PermissionsService } from "@/api/services/PermissionsService";
import { approvalPurpose } from "./approval-purpose";
import { approvalErrorText } from "./approval-errors";

type PolicySelection = { kind: "current" } | { kind: "retained"; id: number };

export function InputApprovalDiscovery({
  packRef,
  disabled,
  onSelect,
}: {
  packRef: string;
  disabled: boolean;
  onSelect: (target: DeclaredInputApprovalTarget) => void;
}) {
  const [policy, setPolicy] = useState<PolicySelection>({ kind: "current" });
  const [policyText, setPolicyText] = useState("");
  const [page, setPage] = useState(1);
  const [discovery, setDiscovery] = useState(0);
  const [error, setError] = useState<string | null>(null);
  const targets = useQuery({
    queryKey: ["inputApprovalTargets", packRef, policy, page, discovery],
    queryFn: () =>
      PermissionsService.listInputApprovalTargets({
        packRef,
        page,
        pageSize: 20,
        ...(policy.kind === "retained" ? { policyRevision: policy.id } : {}),
      }),
  });
  return (
    <section
      aria-label="Discover input targets"
      className="border rounded p-4 space-y-3"
    >
      <h3>Authorized retained input declarations</h3>
      <p>
        Pack association and discovery are not consent or a grant. Issuance
        separately checks current issuer coverage and the exact retained
        declaration. Choose one target. A new release or declaration requires
        fresh discovery and consent.
      </p>
      <p>
        Renderer issuance is not offered: this API discovers policy targets, not
        authorized renderer bindings.
      </p>
      <label>
        Retained policy revision (blank for current)
        <input
          value={policyText}
          disabled={disabled}
          onChange={(event) => setPolicyText(event.target.value)}
        />
      </label>
      <button
        type="button"
        disabled={disabled}
        onClick={() => {
          if (policyText.trim() === "") {
            setPolicy({ kind: "current" });
            setPage(1);
            setDiscovery((value) => value + 1);
            setError(null);
            return;
          }
          const id = Number(policyText);
          if (
            !/^\d+$/.test(policyText) ||
            !Number.isSafeInteger(id) ||
            id <= 0
          ) {
            setError("Use a positive, exactly representable policy revision.");
            return;
          }
          setError(null);
          setPolicy({ kind: "retained", id });
          setPage(1);
          setDiscovery((value) => value + 1);
        }}
      >
        Discover targets
      </button>
      <p>
        Showing{" "}
        {policy.kind === "current"
          ? "current policy"
          : `retained policy ${policy.id}`}
        , target page {page}.
      </p>
      {error && <p role="alert">{error}</p>}
      {targets.isPending && <p>Loading authorized targets...</p>}
      {targets.error && <p role="alert">{approvalErrorText(targets.error)}</p>}
      {targets.data?.items.length === 0 && (
        <p>
          No currently visible retained targets. Nothing can be selected or
          issued from this page.
        </p>
      )}
      <ul>
        {targets.data?.items.map((target, index) => {
          const view = approvalPurpose({
            kind: "declared_input",
            targets: [target],
          });
          if (view.kind !== "declared_input" || !view.metadataAvailable)
            return (
              <li key={index}>
                Target metadata cannot be verified. Selection is disabled.
              </li>
            );
          const [verified] = view.targets;
          if (!verified) return null;
          return (
            <li key={index} className="border p-3">
              <pre className="overflow-auto">
                {JSON.stringify(verified, null, 2)}
              </pre>
              <button
                type="button"
                disabled={disabled || targets.isFetching}
                onClick={() => onSelect(verified)}
              >
                Select input target {index + 1}
              </button>
            </li>
          );
        })}
      </ul>
      <button
        type="button"
        disabled={disabled || targets.isFetching || page === 1}
        onClick={() => setPage((value) => value - 1)}
      >
        Previous target page
      </button>
      <button
        type="button"
        disabled={
          disabled || targets.isFetching || !targets.data?.pagination.has_next
        }
        onClick={() => setPage((value) => value + 1)}
      >
        Next target page
      </button>
    </section>
  );
}
