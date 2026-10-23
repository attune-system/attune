"""E-owned actual G fixture composition. No SDK, worker or transport substitute."""
import json
import os
from pathlib import Path
import re
import urllib.parse
import yaml

G_SOURCES = (
    "provision_interoperability.py", "interoperability_probe.py", "interoperability_transports.py",
    "interoperability_python_sdk.py", "interoperability_js_sdk.mjs", "interoperability_browser.mjs",
    "interoperability_worker.py", "InteroperabilityJavaSdk.java",
    "interoperability_observer.py", "interoperability_scenarios.py",
    "interoperability_batch_worker.py", "interoperability_emitter.py",
)
WORKERS = ("interoperability_worker.py", "interoperability_batch_worker.py", "interoperability_emitter.py")


def validate(args, profile, driver, backend=None):
    settings = driver.private_input(profile.get("interoperability_inputs_file", ""))
    driver.require(7200 <= settings.get("timeout_seconds", 0) <= 14400, "G's full mandatory matrix needs a bounded 2..4 hour foreground deadline")
    operator = driver.private_input(profile["operator_credential_file"])
    driver.require(backend in ("SharedFile", "ObjectSegments") and operator.get("kind") == "password" and operator.get("owned_project") == "eaccept-" + args.run_id + "-" + backend.lower(), "G provisioner requires the existing exact-project local operator password, not an Access/integration fallback")
    tool = settings.get("archive_tool", {})
    path = Path(tool.get("binary", ""))
    driver.require(path.is_absolute() and path.is_file() and not path.is_symlink() and os.access(path, os.X_OK) and driver.digest(path) == tool.get("sha256"), "Supply actual centrally built interoperability_acceptance_archive and its measured hash")
    artifacts = settings.get("artifacts", {})
    driver.require(artifacts and set(settings.get("transports", {})) == {"cli", "mcp", "python", "javascript", "java"}, "G needs all actual CLI/MCP/Python/JS/Java artifacts and browser, without skips")
    for name in G_SOURCES:
        source = args.source_root / "scripts/acceptance" / name
        driver.require(source.is_file() and artifacts.get(str(source)) == driver.digest(source), "Missing/unpinned integrated G source: " + name)
    inspector = settings.get("repository_inspector", {})
    path = Path(inspector.get("binary", ""))
    driver.require(path.is_absolute() and path.name == "interoperability_inspector" and path.is_file() and not path.is_symlink() and os.access(path, os.X_OK) and driver.digest(path) == inspector.get("sha256") == artifacts.get(str(path)), "Supply the actual pinned centrally compiled G repository inspector")
    observer = driver.private_input(settings.get("observer_credential_file", ""))
    driver.require((observer.get("run_id"), observer.get("project")) == (args.run_id, "eaccept-" + args.run_id + "-" + backend.lower()), "G observer credential belongs to a different owned database/project")
    driver.require(observer.get("user") not in (None, "", "attune", "postgres") and isinstance(observer.get("password"), str) and observer["password"] and observer.get("database") == "attune" and observer.get("schema") == profile.get("database_schema", "attune"), "Supply an ordinary SELECT-only observer for the actual schema, never app/owner/superuser credentials")
    for name, digest in artifacts.items():
        path = Path(name)
        driver.require(path.is_absolute() and path.is_file() and not path.is_symlink() and driver.digest(path) == digest, "Missing/changed real G SDK/browser/native artifact")
    return settings


def compose(source_root, destination, principals, driver):
    pack = principals["pack_ref"]
    driver.require(re.fullmatch(r"ginterop_[a-z0-9_]+", pack) and re.fullmatch(r"[A-Za-z0-9_.:@-]+", principals["source_key_ref"]), "Fixture must use the actual run pack and sponsor's canonical source Key ref")
    driver.require(not destination.exists(), "Never overwrite a prior interoperability fixture")
    destination.mkdir(mode=0o700)
    for folder in ("actions", "actions/workflows", "permission_sets", "queues", "types", "rules", "triggers"):
        (destination / folder).mkdir()
    def write(name, value):
        (destination / name).write_text(yaml.safe_dump(value, sort_keys=False))
    receipt = {
        "protocol": {"type": "string", "required": True},
        "nonce": {"type": "string", "required": True},
        "api_token_present": {"type": "boolean", "required": True},
        "runtime_read_status": {"type": ["integer", "null"], "required": True},
        "input_sha256": {"type": "string"},
        "runtime_value_sha256": {"type": "string"},
    }
    write("pack.yaml", {"ref": pack, "version": "1.0.0", "label": "Owned real interoperability", "description": "Five-principal transport acceptance; no plaintext source input", "runtime_deps": {"python": []}, "conf_schema": {"interop_counter": {"type": "integer", "default": 5}}, "config": {"interop_counter": 5}})
    for name in WORKERS:
        (destination / "actions" / name).write_bytes((source_root / "scripts/acceptance" / name).read_bytes())
    def named(name, fields, additional=False):
        # Named types are SingleValue; inline boolean required belongs only to
        # FlatDocument occurrences, never to an object's properties.
        schema = {"type": "object", "properties": {key: {k: v for k, v in value.items() if k != "required"} for key, value in fields.items()}, "required": [key for key, value in fields.items() if value.get("required") is True], "additionalProperties": additional}
        write("types/" + name + ".yaml", {"ref": pack + "." + name, "label": name.replace("_", " "), "reference_visibility": "public", "schema": schema})
    public = {"nonce": {"type": "string", "label": "Probe nonce", "required": True}, "runtime_action_ref": {"type": "string", "label": "Runtime action reference", "required": True}}
    batch_payload = {"nonce": {"type": "string", "required": True}, "secret": {"type": "string", "required": True, "secret": True}}
    named("probe_receipt", receipt)
    named("batch_payload", batch_payload)
    named("batch_context", {"id": {"type": "integer", "required": True}, "payload": {"type": pack + ".batch_payload", "required": True}}, additional=True)
    named("batch_receipt_item", {"id": {"type": "integer", "required": True}, "nonce": {"type": "string", "required": True}, "secret_sha256": {"type": "string", "required": True}})
    batch_output = {"protocol": {"type": "string", "enum": ["interop-batch-v1"], "required": True}, "api_token_present": {"type": "boolean", "required": True}, "items": {"type": "array", "items": {"type": pack + ".batch_receipt_item"}, "required": True}}
    def action(name, worker, parameters, refs, output=receipt):
        write("actions/" + name + ".yaml", {"ref": pack + "." + name, "label": name.replace("_", " ").capitalize(), "enabled": True, "runner_type": "python", "entry_point": worker, "parameter_delivery": "stdin", "parameter_format": "json", "output_format": "json", "default_execution_permission_set_refs": refs, "parameters": parameters, "output": output})
    action("probe", WORKERS[0], public, [pack + ".runtime"])
    action("input_probe", WORKERS[0], dict(public, input={"type": "string", "secret": True, "default": "{{ keystore['" + principals["source_key_ref"] + "'] }}"}), [pack + ".runtime"])
    action("external_probe", WORKERS[0], {"nonce": public["nonce"], "runtime_key_ref": {"type": "string", "required": True}}, [])
    action("emit", WORKERS[2], dict(public, trigger_ref={"type": "string", "required": True}), [pack + ".emitter"])
    action("batch_probe", WORKERS[1], {"queue_items": {"type": "array", "items": {"type": pack + ".batch_context"}, "minItems": 1, "maxItems": 2, "required": True}}, [], batch_output)
    write("actions/input_workflow.yaml", {"ref": pack + ".input_workflow", "label": "Input workflow", "enabled": True, "workflow_file": "workflows/input_workflow.workflow.yaml", "parameters": public, "default_execution_permission_set_refs": []})
    write("actions/workflows/input_workflow.workflow.yaml", {"version": "1.0.0", "tasks": [{"name": "read", "action": pack + ".input_probe", "permission_set_refs": [], "input": {key: "{{ parameters." + key + " }}" for key in public}, "next": []}]})
    for name in ("runtime", "runtime_alt", "external_runtime", "emitter"):
        grant = {"resource": "events", "actions": ["create"], "constraints": {"refs": [pack + ".input_tick"]}} if name == "emitter" else {"resource": "actions", "actions": ["read"], "constraints": {"refs": [pack + ".probe"]}}
        write("permission_sets/" + name + ".yaml", {"ref": pack + "." + name, "label": name.replace("_", " "), "scope": "pack", "grants": [grant]})
    write("triggers/input_tick.yaml", {"ref": pack + ".input_tick", "label": "Input tick", "enabled": True, "type": "custom", "parameters": {}, "output": public})
    write("rules/input_rule.yaml", {"ref": pack + ".input_rule", "label": "Input rule", "enabled": True, "trigger_ref": pack + ".input_tick", "action_ref": pack + ".input_probe", "conditions": {}, "trigger_params": {}, "permission_set_refs": [], "action_params": {key: "{{ event.payload." + key + " }}" for key in public}})
    for name, batch in (("inbox", True), ("input_inbox", False)):
        write("queues/" + name + ".yaml", {"ref": pack + "." + name, "label": name.replace("_", " "), "enabled": True, "accepting_new_items": True, "batch_mode": "batch" if batch else "single", "dispatch_action": pack + (".batch_probe" if batch else ".input_probe"), "permission_set_refs": [], "item_schema": batch_payload if batch else public, "action_params": {"queue_items": "{{ queue_items }}"} if batch else {key: "{{ item." + key + " }}" for key in public}, "config": {"dispatch": {"batch_size": {"source": "literal", "value": 2}, "coalescing": {"enabled": False}}} if batch else {}})
    return destination


def stage(stack, context, driver):
    settings = validate(stack.args, stack.profile, driver, stack.backend)
    root = stack.root
    provision = stack.args.source_root / "scripts/acceptance/provision_interoperability.py"
    result = driver.fixture([driver.sys.executable, str(provision), "--execute"], dict(context, operator_credential_file=stack.profile["operator_credential_file"]))
    binding_file = Path(result["principals_binding_file"])
    driver.require(binding_file.parent.resolve() == root.resolve(), "G provisioner returned a foreign binding path")
    principals = driver.private_input(binding_file)
    driver.require((principals["run_id"], principals["project"], principals["api_url"]) == (context["run_id"], context["project"], context["api_url"]), "G provisioned principals belong to a different owned deployment")
    source = compose(stack.args.source_root, root / principals["pack_ref"], principals, driver)
    release = json.loads(driver.command([settings["archive_tool"]["binary"], str(source), str(root / "g-canonical-store")], timeout=900))
    archive = Path(release["archive_path"])
    driver.require(archive.is_relative_to(root) and archive.is_file() and driver.digest(archive) == release["digest"], "G fixture must use the actual measured canonical PackStorage archive")
    pack = principals["pack_ref"]
    observer = driver.private_input(settings["observer_credential_file"])
    template = root / "g-runtime-observer-template.private.json"
    driver.private_json(template, {"run_id": context["run_id"], "project": context["project"], "database_url": "postgresql://" + urllib.parse.quote(observer["user"], safe="") + ":" + urllib.parse.quote(observer["password"], safe="") + "@" + stack.endpoint("postgres", 5432) + "/attune", "schema": observer["schema"]})
    binding = dict(principals, archive_file=str(archive), expected_release_digest=release["digest"], action_ref=pack + ".probe", input_action_ref=pack + ".input_probe", runtime_action_ref=pack + ".probe", queue_ref=pack + ".inbox", schema_ref=pack + ".probe_receipt", permission_ref=pack + ".runtime", default_refs=[pack + ".runtime"], config_field="interop_counter", config_default=5, inspector_binary=settings["repository_inspector"]["binary"], inspector_context_template_file=str(template), inspector_release_digests=[release["digest"]], transports=settings["transports"], browser=dict(settings["browser"], web_url="http://" + stack.endpoint("web", 80)), artifacts=dict(settings["artifacts"], **{str(archive): driver.digest(archive), str(template): driver.digest(template)}))
    path = root / "g-interoperability-binding.private.json"
    driver.require(len(json.dumps(binding).encode()) <= 16384, "G's complete private binding exceeds the shared bounded-input contract")
    driver.private_json(path, binding)
    probe = stack.args.source_root / "scripts/acceptance/interoperability_probe.py"
    outcome = driver.fixture([driver.sys.executable, str(probe), "--execute"], dict(context, interoperability_binding_file=str(path)), timeout=settings["timeout_seconds"])
    driver.require(outcome.get("real_server_matrix_completed") is True and outcome.get("interoperability_criteria_passed") is True and set(outcome.get("lanes", [])) == {"api", "cli", "mcp", "python", "javascript", "java"} and outcome.get("browser") is True and outcome.get("full_sow_acceptance") is False, "G's actual complete matrix/browser must finish; no skipped lanes or full SOW claim")
    return outcome
