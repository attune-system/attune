#!/usr/bin/env python3
"""One real Python SDK operation. Install/build the SDK centrally, not here."""
import json
import sys

from interoperability_transports import authored, resource_path, unwrap


def main(request):
    from attune.client import AttuneClient
    from attune.input_approvals import InputApprovalRequest
    client = AttuneClient(api_url=request["api_url"], api_token=request["access_token"])
    operation, pack = request["operation"], request.get("pack")
    if request.get("expect_denial"):
        from attune.errors import APIError
        if operation != "execute":
            raise ValueError("Denial probe must execute the actual invocation")
        try:
            client.post("/api/v1/executions/execute", json=request["body"], redact_errors=True)
        except APIError as error:
            return {"native_denial": error.to_dict()}
        raise ValueError("Native SDK accepted revoked/expired authority")
    if operation in ("runtime_issue", "runtime_replace"):
        from attune.api_client.api.permissions import issue_runtime_approval, replace_runtime_approval
        from attune.api_client.models.issue_runtime_approval_request import IssueRuntimeApprovalRequest
        body = IssueRuntimeApprovalRequest.from_dict(request["body"])
        result = issue_runtime_approval.sync_detailed(pack, client=client.api_client, body=body) if operation == "runtime_issue" else replace_runtime_approval.sync_detailed(pack, request["id"], client=client.api_client, body=body)
        if result.status_code not in (200, 201) or result.parsed is None:
            raise ValueError("Generated runtime approval endpoint failed")
        return result.parsed.to_dict()
    approvals = client.input_approvals
    if operation == "targets":
        return approvals.targets(pack)
    if operation == "issue":
        return approvals.issue(pack, InputApprovalRequest(**request["body"]))
    if operation == "approval_get":
        return approvals.get(pack, request["id"])
    if operation == "approval_revoke":
        return approvals.revoke(pack, request["id"])
    if operation == "queue_grouping":
        path = "/api/v1/queues/" + request["ref"]
        record = unwrap(client.get(path))
        config = dict(record.get("config") or {})
        config["dispatch"] = dict(config.get("dispatch") or {}, separate_requesters=request["grouping"] == "separate")
        return client.put(path, json={"config": config})
    if operation == "permission_import":
        record = unwrap(client.get("/api/v1/permissions/sets/by-ref/" + request["ref"]))
        body = {key: value for key, value in request["definition"].items() if key != "ref"}
        return client.put(f"/api/v1/permissions/sets/{record['id']}", json=body)
    method, path, body = resource_path(operation, request)
    if method == "GET":
        result = client.get(path)
    elif method == "PUT":
        result = client.put(path, json=body)
    else:
        result = client.post(path, json=body)
    return authored(unwrap(result)) if operation == "permission_export" else result


if __name__ == "__main__":
    try:
        print(json.dumps(main(json.loads(sys.stdin.read(1024 * 1024)))))
    except Exception:
        print("Real Python SDK operation failed; response and credentials suppressed", file=sys.stderr)
        sys.exit(1)
