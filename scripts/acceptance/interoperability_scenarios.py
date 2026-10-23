"""Required connected scenarios. Every lane is native; no success/skip adapters."""
from datetime import datetime, timedelta, timezone
import hashlib
import json
import secrets
import time
import urllib.error
import urllib.request
from urllib.parse import quote

from fixture_support import require
from interoperability_transports import denial, unwrap
from interoperability_observer import consumed_input_ids, stable_pins

LANES = ("api", "cli", "mcp", "python", "javascript", "java")
KINDS = ("action", "workflow_task", "rule", "queue")
TERMINAL = ("completed", "failed", "cancelled", "timeout", "abandoned")


class Scenarios:
    def __init__(self, probe):
        self.p = probe
        self.pack = probe.b["pack_ref"]

    def invoke(self, lane, role, operation, **args):
        return self.p.call(role, operation, **args) if lane == "api" else self.p.transports[(lane, role)].call(operation, **args)

    def submit(self, lane, body):
        result = unwrap(self.invoke(lane, "runtimeconsumer", "execute", body=body))
        require(type(result.get("id")) is int, "Native transport did not create an actual execution")
        self.p.executions.append(result["id"])
        observed = self.p.observer.execution(result["id"])
        require(observed["binding"]=={"kind":"action","action_ref":body["action_ref"]} and observed["selected_refs"]==body["permission_set_refs"],"Native response identified another action/selection")
        require(observed["attribution"]=={"kind":"human","identity_id":self.p.identities["runtimeconsumer"]} and observed["executor_identity"]==self.p.identities["runtimeconsumer"],"Native execution did not retain the actual invoking principal")
        by_nonce = self.p.observer.call({"kind":"discover","binding_kind":"action","case_nonce":body["parameters"]["nonce"]})
        require(len(by_nonce)==1 and by_nonce[0]["execution_id"]==result["id"],"Client response ID does not match its actual submitted case nonce")
        return result["id"]

    def settle(self, execution_id, receipt=None, binding=None):
        deadline = time.monotonic() + 120
        while True:
            row = self.p.observer.execution(execution_id)
            if row["status"] in TERMINAL:
                break
            require(time.monotonic() < deadline, "Actual native dispatch did not settle")
            time.sleep(0.2)
        require(row["status"] == "completed" and row["worker_id"] is not None, "Required actual action worker did not complete")
        if receipt is not None:
            row = self.p.observer.execution(execution_id, receipt)
            require(row.get("worker_receipt_verified") is True, "Saved worker receipt does not match the nonce-bound case")
        if binding is not None:
            require(row["binding"] == binding, "Worker consumed a different invocation binding")
        self.p.saved_pins[execution_id] = row
        return row

    def input_binding(self, kind):
        binding = {"kind":kind,"action_ref":self.p.b["input_action_ref"]}
        if kind == "workflow_task":
            binding.update(workflow_ref=self.pack + ".input_workflow")
        elif kind == "rule":
            binding["rule_ref"] = self.pack + ".input_rule"
        elif kind == "queue":
            binding["queue_ref"] = self.pack + ".input_inbox"
        return binding

    def target_request(self, lane, kind):
        targets = unwrap(self.invoke(lane, "sponsor", "targets", pack=self.pack))
        require(not targets["pagination"]["has_next"], "Fixture target inventory exceeds the one-page contract")
        binding = self.input_binding(kind)
        matches = [target for target in targets["items"] if all(target["invocation_binding"].get(key)==value for key,value in binding.items())]
        require(len(matches) == 1, "Missing or ambiguous real retained declaration for " + kind)
        target = matches[0]
        if kind=="workflow_task":
            require(set(target["invocation_binding"])==set(binding)|{"task_path"} and target["invocation_binding"]["task_path"].startswith("/tasks/"),"Workflow target must retain its actual canonical task pointer")
        else:
            require(target["invocation_binding"]==binding,"Invocation target has noncanonical extra fields")
        grant = {"resource":"keys","actions":["read","decrypt"],"scope":"external","constraints":{"refs":[self.p.b["source_key_ref"]],"owner_types":["identity"],"owner_refs":[self.p.b["roles"]["sponsor"]["login"]]}}
        body = {key:target[key] for key in ("policy_revision","invocation_binding","declaration_revision","source_path","expression")}
        body["grants"] = [grant]
        return body, target

    def discover_child(self, kind, nonce):
        deadline = time.monotonic() + 120
        while True:
            rows = self.p.observer.call({"kind":"discover","binding_kind":kind,"case_nonce":nonce})
            require(len(rows) <= 1, "One invocation created duplicate native child admissions")
            if rows:
                eid = rows[0]["execution_id"]
                self.p.executions.append(eid)
                return eid
            require(time.monotonic() < deadline, "No actual native child for " + kind)
            time.sleep(0.2)

    def invoke_input(self, lane, kind, approval_id, binding):
        nonce = secrets.token_hex(12)
        parameters = {"nonce":nonce,"runtime_action_ref":self.p.b["runtime_action_ref"]}
        root = None
        item = None
        if kind == "action":
            eid = self.submit(lane, {"action_ref":self.p.b["input_action_ref"],"parameters":parameters,"permission_set_refs":[]})
        elif kind == "workflow_task":
            root = self.submit(lane, {"action_ref":self.pack + ".input_workflow","parameters":parameters,"permission_set_refs":[]})
            eid = self.discover_child(kind, nonce)
        elif kind == "rule":
            producer = self.submit(lane, {"action_ref":self.pack + ".emit","parameters":dict(parameters, trigger_ref=self.pack + ".input_tick"),"permission_set_refs":[self.pack + ".emitter"]})
            self.settle(producer, {"protocol":"interop-v1","nonce":nonce,"api_token_present":True,"runtime_read_status":201}, {"kind":"action","action_ref":self.pack + ".emit"})
            eid = self.discover_child(kind, nonce)
        else:
            item = unwrap(self.invoke(lane,"runtimeconsumer","enqueue",ref=self.pack + ".input_inbox",body={"item_key":nonce,"payload":parameters}))
            require(type(item.get("id")) is int, "Queue input invocation did not enqueue real work")
            eid = self.discover_child(kind, nonce)
        row = self.settle(eid, {"protocol":"interop-v1","nonce":nonce,"api_token_present":False,"runtime_read_status":None,"input_sha256":self.p.b["input_sha256"]}, binding)
        require(row["selected_refs"] == [] and approval_id in consumed_input_ids(row), "Input-only consent was not consumed by the exact immutable binding")
        if root is not None:
            deadline = time.monotonic()+120
            while True:
                outer = self.p.observer.execution(root)
                if outer["status"] in TERMINAL:
                    require(outer["status"]=="completed","Workflow root did not complete its actual input task")
                    self.p.saved_pins[root] = outer
                    break
                require(time.monotonic()<deadline,"Workflow parent settlement deadline expired")
                time.sleep(0.2)
        if item is not None:
            self.settle_items(self.pack + ".input_inbox",[item["id"]])
        return row

    def settle_items(self, queue, ids):
        for item_id in ids:
            deadline = time.monotonic()+120
            while True:
                row = unwrap(self.p.api.call(f"/api/v1/queues/{queue}/items/{item_id}",self.p.tokens["maintenance"]))
                if row["status"] in ("completed","failed","skipped","cancelled"):
                    require(row["status"]=="completed","Real queue item did not complete successfully")
                    break
                require(time.monotonic()<deadline,"Actual queue item ACK did not settle")
                time.sleep(0.2)

    def input_variants(self):
        covered = set()
        for lane in LANES:
            for kind in KINDS:
                body, target = self.target_request(lane, kind)
                approval = unwrap(self.invoke(lane,"sponsor","issue",pack=self.pack,body=body))
                self.p.approvals.append(approval["id"])
                require(approval["purpose"] == {"kind":"declared_input","targets":[target]}, "Native client reclassified an invocation-bound input approval")
                saved = self.invoke_input(lane,kind,approval["id"],body["invocation_binding"])
                revoked = unwrap(self.invoke(lane,"sponsor","approval_revoke",pack=self.pack,id=approval["id"]))
                require(revoked["active"] is False and revoked["purpose"]["kind"] == "declared_input", "Native input revocation did not preserve purpose")
                retained = self.p.observer.execution(saved["execution_id"])
                require(stable_pins(retained) == stable_pins(saved) and approval["id"] in consumed_input_ids(retained), "Revocation rewrote a historical input approval pin")
                if kind == "action":
                    self.native_denial(lane,{"action_ref":self.p.b["input_action_ref"],"parameters":{"nonce":secrets.token_hex(12),"runtime_action_ref":self.p.b["runtime_action_ref"]},"permission_set_refs":[]},"input_revoked",saved)
                covered.add((lane,kind))
                self.p.note("all_input_binding",lane=lane,binding_kind=kind,execution_id=saved["execution_id"],approval_id=approval["id"])
        require(covered == {(lane,kind) for lane in LANES for kind in KINDS}, "Input invocation matrix omitted a mandatory lane/kind")

    def native_denial(self, lane, body, reason, prior=None):
        path = "/api/v1/executions?action_ref=" + quote(body["action_ref"],safe="") + "&page_size=100"
        before = self.p.api.call(path,self.p.tokens["maintenance"])
        require(not before["pagination"]["has_next"],"Denial inventory exceeded bound")
        if lane == "api":
            request = urllib.request.Request(self.p.api.base + "/api/v1/executions/execute",json.dumps(body).encode(),{"Authorization":"Bearer " + self.p.tokens["runtimeconsumer"],"Content-Type":"application/json"},method="POST")
            try:
                response = self.p.api.opener.open(request,timeout=15)
            except urllib.error.HTTPError as response:
                with response:
                    payload = json.loads(response.read(65536))
                    diagnostic = denial({"status":response.code,"code":payload.get("code"),"request_id":response.headers.get("x-request-id")})
            else:
                response.close()
                raise ValueError("Actual API accepted revoked/expired authority")
        else:
            diagnostic = self.p.transports[(lane,"runtimeconsumer")].call("execute",body=body,expect_denial=True)
        after = self.p.api.call(path,self.p.tokens["maintenance"])
        require(not after["pagination"]["has_next"] and {r["id"] for r in after["items"]} == {r["id"] for r in before["items"]},"Native denial still created an execution")
        self.p.note("native_postrevocation_diagnostic",lane=lane,reason=reason,response=diagnostic,actor_identity=self.p.identities["runtimeconsumer"],action_ref=body["action_ref"],prior_saved_context=stable_pins(prior) if prior else None)

    def runtime_approvals(self):
        ref = self.pack + ".external_runtime"
        original = self.p.call("maintenance","permission_export",ref=ref)
        covered = set()
        try:
            for lane in LANES:
                fixed, _ = self.target_request(lane,"action")
                first_body = {"grants":fixed["grants"]}
                first = unwrap(self.invoke(lane,"sponsor","runtime_issue",pack=self.pack,body=first_body))
                self.p.approvals.append(first["id"])
                require(first["purpose"] == {"kind":"runtime"},"Runtime approval became input-only consent")
                definition = dict(original,scope="external",grants=fixed["grants"])
                self.invoke(lane,"maintenance","permission_import",ref=ref,definition=definition)
                nonce = secrets.token_hex(12)
                body = {"action_ref":self.pack + ".external_probe","parameters":{"nonce":nonce,"runtime_key_ref":self.p.b["source_key_ref"]},"permission_set_refs":[ref]}
                eid = self.submit(lane,body)
                receipt = {"protocol":"interop-v1","nonce":nonce,"api_token_present":True,"runtime_read_status":200,"runtime_value_sha256":self.p.b["input_sha256"]}
                saved = self.settle(eid,receipt)
                require({a["id"] for a in saved["runtime_approvals"]} == {first["id"]},"Execution did not retain the actual runtime approval ID")
                expiry = datetime.now(timezone.utc) + timedelta(seconds=60)
                replacement = unwrap(self.invoke(lane,"sponsor","runtime_replace",pack=self.pack,id=first["id"],body={"grants":fixed["grants"],"expires_at":expiry.isoformat()}))
                self.p.approvals.append(replacement["id"])
                require(replacement["id"] != first["id"] and replacement["purpose"] == {"kind":"runtime"},"Runtime replacement lost purpose or identity")
                old = unwrap(self.invoke(lane,"sponsor","approval_get",pack=self.pack,id=first["id"]))
                require(old["active"] is False and old["revoked_at"] is not None,"Replacement did not actually revoke predecessor")
                self.native_denial(lane,body,"runtime_replaced",saved)
                require(stable_pins(self.p.observer.execution(eid)) == stable_pins(saved),"Runtime replacement rewrote historical admission")
                # Re-importing identical grants is intentionally a no-op in the
                # server. Make a real, harmless mixed-scope definition change to
                # readmit B instead of pretending a label/no-op rebuilt policy.
                readmitted = dict(definition,grants=[*fixed["grants"],{"resource":"actions","actions":["read"],"scope":"pack","constraints":{"refs":[self.p.b["runtime_action_ref"]]}}])
                self.invoke(lane,"maintenance","permission_import",ref=ref,definition=readmitted)
                nonce = secrets.token_hex(12); body["parameters"]["nonce"] = nonce
                eid = self.submit(lane,body)
                receipt["nonce"] = nonce
                updated = self.settle(eid,receipt)
                require({a["id"] for a in updated["runtime_approvals"]} == {replacement["id"]},"Readmission silently reused the replaced runtime approval")
                require(updated["policy_revision"] != saved["policy_revision"],"Replacement readmission did not select a new immutable policy")
                deadline = time.monotonic() + 90
                while unwrap(self.invoke(lane,"sponsor","approval_get",pack=self.pack,id=replacement["id"]))["active"]:
                    require(time.monotonic() < deadline,"Server did not observe actual approval expiry")
                    time.sleep(0.2)
                while True:
                    retained = self.p.observer.execution(eid)
                    pinned = [approval for approval in retained["runtime_approvals"] if approval["id"]==replacement["id"]]
                    require(len(pinned)==1,"Replacement execution lost its exact fixed approval")
                    if pinned[0]["expired"] is True:
                        break
                    require(time.monotonic()<deadline,"Actual database approval deadline did not expire")
                    time.sleep(0.2)
                self.native_denial(lane,body,"runtime_expired",updated)
                require(stable_pins(self.p.observer.execution(eid)) == stable_pins(updated),"Expiry rewrote the saved runtime pins")
                self.invoke(lane,"maintenance","permission_import",ref=ref,definition=original)
                self.invoke(lane,"sponsor","approval_revoke",pack=self.pack,id=replacement["id"])
                covered.add(lane)
                self.p.note("runtime_replace_expiry",lane=lane,predecessor=first["id"],replacement=replacement["id"],execution_id=eid)
        finally:
            self.p.call("maintenance","permission_import",ref=ref,definition=original)
        require(covered == set(LANES),"Runtime replacement/expiry omitted a native lane")

    def queue_batches(self):
        queue = self.p.b["queue_ref"]
        path = "/api/v1/queues/" + queue
        baseline = unwrap(self.p.call("maintenance","queue_get",ref=queue))
        require(baseline["accepting_new_items"] is True,"Owned queue must accept both requesters while dispatch is disabled")
        covered = set()
        try:
            for lane in LANES:
                for grouping in ("aggregate","separate"):
                    self.p.api.call(path,self.p.tokens["maintenance"],{"enabled":False},"PUT")
                    self.invoke(lane,"maintenance","queue_grouping",ref=queue,grouping=grouping)
                    items = {}
                    for role in ("runtimeconsumer","sponsor"):
                        nonce = secrets.token_hex(12)
                        value = secrets.token_urlsafe(32)
                        payload = {"nonce":nonce,"secret":value}
                        item = unwrap(self.invoke(lane,role,"enqueue",ref=queue,body={"item_key":nonce,"payload":payload}))
                        require(item["requested_by_identity"] == self.p.identities[role],"Enqueue replaced the authenticated requester")
                        items[item["id"]] = {"id":item["id"],"nonce":nonce,"secret_sha256":hashlib.sha256(json.dumps(value,separators=(",", ":")).encode()).hexdigest(),"identity":self.p.identities[role]}
                    require(len(items)==2,"Queue deduplication merged distinct requesters")
                    self.p.api.call(path,self.p.tokens["maintenance"],{"enabled":True},"PUT")
                    deadline = time.monotonic()+120
                    while True:
                        rows = self.p.observer.call({"kind":"queue_batch","queue_ref":queue,"item_ids":sorted(items)})
                        observed = {int(item) for row in rows for item in row["attribution"]["items"]}
                        if observed == set(items) and all(row["status"] in TERMINAL for row in rows):
                            break
                        require(time.monotonic()<deadline,"Actual queue batch dispatch did not cover both requesters")
                        time.sleep(0.2)
                    require(len(rows)==(1 if grouping=="aggregate" else 2),"Actual batch cardinality violates requester grouping")
                    for row in rows:
                        self.p.executions.append(row["execution_id"])
                        attribution = {int(item):actor for item,actor in row["attribution"]["items"].items()}
                        require(attribution and all(items[item]["identity"] == actor for item,actor in attribution.items()),"Saved batch attribution crossed requester identities")
                        require(len(row["queue_contributors"])==len(attribution) and {r["item_id"] for r in row["queue_contributors"]}==set(attribution) and all(r["expected_items"]==len(attribution) and r["identity_id"]==attribution[r["item_id"]] and r["retained_item_authority_match"] is True for r in row["queue_contributors"]),"Immutable contributor authorities were unioned, lost, replaced or copied from another actor")
                        if grouping=="aggregate":
                            require(row["executor_identity"] is None,"Mixed-requester batch inherited one human's executor identity")
                        else:
                            require(len(attribution)==1 and row["executor_identity"]==next(iter(attribution.values())),"Separate batch attributed another requester's executor")
                        expected = {"protocol":"interop-batch-v1","api_token_present":False,"items":[{key:items[item][key] for key in ("id","nonce","secret_sha256")} for item in sorted(attribution)]}
                        self.settle(row["execution_id"],expected)
                        # An Access reader without the contributor closure must
                        # never get a mixed/foreign execution payload or logs.
                        for role in ("reader","runtimeconsumer"):
                            if role=="runtimeconsumer" and set(attribution.values())=={self.p.identities[role]}:
                                continue
                            for suffix in ("", "/logs/stdout/stream"):
                                self.p.api.call(f"/api/v1/executions/{row['execution_id']}"+suffix,self.p.tokens[role],expected=(403,404))
                    ids = list(items)
                    self.p.api.call(path+f"/items/{ids[1]}",self.p.tokens["runtimeconsumer"],expected=(403,404))
                    self.p.api.call(path+f"/items/{ids[0]}",self.p.tokens["sponsor"],expected=(403,404))
                    self.settle_items(queue,ids)
                    covered.add((lane,grouping))
                    self.p.note("real_multi_requester_batch",lane=lane,grouping=grouping,item_ids=sorted(items),execution_ids=[r["execution_id"] for r in rows],confidentiality_checked=True)
        finally:
            self.p.api.call(path,self.p.tokens["maintenance"],{"enabled":baseline["enabled"],"config":baseline["config"]},"PUT")
        require(covered=={(lane,g) for lane in LANES for g in ("aggregate","separate")},"Real queue matrix omitted a lane/grouping")

    def browser_readmission(self, original):
        old = next(entry for entry in original["executions"] if entry["mode"]=="empty")
        old_refs = next(entry for entry in original["executions"] if entry["mode"]=="explicit")
        saved = self.p.observer.execution(old["id"])
        saved_refs = self.p.observer.execution(old_refs["id"])
        action = self.p.b["action_ref"]
        path = "/api/v1/actions/" + action
        refs = [self.pack + ".runtime_alt"]
        try:
            self.p.api.call(path,self.p.tokens["maintenance"],{"default_execution_permission_set_refs":refs},"PUT")
            options = unwrap(self.p.api.call(path+"/execution-options",self.p.tokens["runtimeconsumer"]))
            require(options["default_permission_set_refs"]==refs and options["policy_revision_id"]!=saved["policy_revision"],"Management edit did not actually readmit changed defaults")
            result = self.p.browser("rerun",parameters={"runtime_action_ref":self.p.b["runtime_action_ref"]},policy_revision_id=options["policy_revision_id"],default_refs=refs,old_execution_id=old["id"],old_nonce=old["nonce"],old_explicit_execution_id=old_refs["id"],old_explicit_nonce=old_refs["nonce"])
            require(len(result["executions"])==3,"Browser must submit new-default plus historical empty and explicit-ref reruns")
            for entry in result["executions"]:
                expected = refs if entry["mode"]=="default" else self.p.b["default_refs"] if entry["mode"]=="old_explicit_refs" else []
                require(entry["id"] not in (old["id"],old_refs["id"]),"Rerun returned an old execution ID instead of a real new admission")
                self.p.wait("runtimeconsumer",{"id":entry["id"]},expected,entry["nonce"])
                if entry["mode"]=="default":
                    require(self.p.saved_pins[entry["id"]]["policy_revision"]==options["policy_revision_id"],"New browser execution did not retain the readmitted policy")
            retained = self.p.observer.execution(old["id"])
            require(stable_pins(retained)==stable_pins(saved),"Readmission rewrote old browser execution pins/explicit selection")
            require(stable_pins(self.p.observer.execution(old_refs["id"]))==stable_pins(saved_refs),"Readmission rewrote historical explicit nonempty selection pins")
            self.p.note("browser_historical_rerun_readmission",original_execution=old["id"],original_explicit_execution=old_refs["id"],old_policy=saved["policy_revision"],new_policy=options["policy_revision_id"])
        finally:
            self.p.api.call(path,self.p.tokens["maintenance"],{"default_execution_permission_set_refs":self.p.b["default_refs"]},"PUT")
