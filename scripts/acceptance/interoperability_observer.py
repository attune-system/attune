"""Nonce-bound local repository observations. Never an HTTP/IAM fallback."""
import hashlib
import hmac
import json
from pathlib import Path
import secrets

from fixture_support import read_secret, require, write_private
from interoperability_transports import environment, foreground
from interoperability_transports import unwrap


def canonical(value):
    return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False)


class Observer:
    def __init__(self, probe, pack_id):
        self.p = probe
        self.key = secrets.token_hex(32)
        template = json.loads(read_secret(probe.b["inspector_context_template_file"]))
        require(template.get("run_id") == probe.ctx["run_id"] and template.get("project") == probe.ctx["project"], "Observer template belongs to a different owned deployment")
        self.path = probe.root / "runtime-inspector.private.json"
        access = {"run_id": probe.ctx["run_id"], "project": probe.ctx["project"],
                  "pack_id": pack_id, "pack_ref": probe.b["pack_ref"],
                  "requester_ids": sorted(probe.identities.values()),
                  "release_digests": probe.b["inspector_release_digests"], "receipt_key": self.key}
        write_private(self.path, {"database_url": template["database_url"], "schema": template["schema"], "access": access})

    def call(self, operation):
        request = {"run_id": self.p.ctx["run_id"], "project": self.p.ctx["project"], "nonce": secrets.token_hex(12), "operation": operation}
        env = environment(self.p.root)
        env["ATTUNE_INTEROPERABILITY_INSPECTOR_CONTEXT"] = str(self.path)
        result = json.loads(foreground([self.p.b["inspector_binary"], "--execute"], json.dumps(request), env, timeout=30))
        require(result.get("protocol") == 1 and result.get("request") == request, "Repository observer returned a foreign case/nonce")
        signed = canonical([1, request, result["observations"]]).encode()
        expected = hmac.new(self.key.encode(), signed, hashlib.sha256).hexdigest()
        require(hmac.compare_digest(expected, result.get("mac", "")), "Repository observer MAC mismatch; no unsigned success adapters")
        if result["observations"] and all(row["status"] in ("completed","failed","cancelled","timeout","abandoned") for row in result["observations"]):
            self.p.note("repository_receipt", receipt=result)
        return result["observations"]

    def execution(self, execution_id, expected_receipt=None):
        operation = {"kind": "execution", "execution_id": execution_id, "expected_receipt": None}
        rows = self.call(operation)
        require(len(rows) == 1 and rows[0]["execution_id"] == execution_id, "Missing exact saved execution admission")
        action = rows[0]["binding"]["action_ref"].removeprefix(self.p.b["pack_ref"]+".")
        sources = {"probe":"interoperability_worker.py","input_probe":"interoperability_worker.py","external_probe":"interoperability_worker.py","batch_probe":"interoperability_batch_worker.py","emit":"interoperability_emitter.py"}
        if action in sources:
            require(rows[0]["entrypoint"] in (sources[action],"actions/"+sources[action]),"Saved executable did not select the reviewed archive worker source")
        else:
            require(action=="input_workflow","Observer encountered an undeclared fixture action")
        row = dict(rows[0])
        if expected_receipt is not None:
            # This is a separate, explicit authorized output witness, not an
            # operation/denial fallback for any client lane. The native inspector
            # never reads ciphertext or receives a decryption key. C protects
            # output data even when a program only prints hashes.
            witness = unwrap(self.p.api.call(f"/api/v1/executions/{execution_id}?include_secret_values=true",self.p.tokens["maintenance"]))
            require(witness["id"]==execution_id,"Output witness returned another execution")
            require(isinstance(witness.get("result"),dict) and witness["result"].get("data")==expected_receipt,"Actual protected worker JSON receipt did not match the nonce/hash case")
            require(witness["result"].get("succeeded") is True and witness["result"].get("exit_code")==0 and "output_validation_warning" not in witness["result"],"Worker result was not a clean schema-valid successful completion")
            row["worker_receipt_verified"] = True
            self.p.note("authorized_worker_receipt",execution_id=execution_id,authority_digest=row["authority_digest"],receipt_sha256=hashlib.sha256(canonical(expected_receipt).encode()).hexdigest())
        return row

    def close(self):
        self.path.unlink()


def stable_pins(row):
    return {key: row[key] for key in ("execution_id", "admission_id", "policy_revision", "policy_digest", "authority_digest", "binding", "selected_refs", "release_id", "release_digest", "schema_binding_digest", "entrypoint")}


def consumed_input_ids(row):
    return {approval["id"] for receipt in row["input_receipts"] for approval in receipt["input_approvals"]}
