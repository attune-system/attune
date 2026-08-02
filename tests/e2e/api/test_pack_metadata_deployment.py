"""
E2E Test: Pack Metadata Deployment

Verifies that uploading a pack with all supported metadata types
(permission sets, runtimes, triggers, actions, workflow actions,
queues, rules, sensors) results in every component being correctly
registered and queryable via the API.
"""

import uuid
from pathlib import Path
from tempfile import TemporaryDirectory

import pytest


def _uid():
    return uuid.uuid4().hex[:8]


def _get(client, path, expected=200):
    """Helper: GET with status assertion."""
    resp = client.session.get(f"{client.base_url}{path}", timeout=15)
    assert resp.status_code == expected, (
        f"GET {path} returned {resp.status_code}: {resp.text}"
    )
    return resp.json().get("data", resp.json())


@pytest.mark.api
class TestPackMetadataDeployment:
    """
    Upload a synthetic pack containing every metadata type and verify
    each component is accessible through API endpoints.
    """

    def _build_pack(self, pack_dir: Path, pack_ref: str):
        """Build a synthetic pack directory with all metadata types."""
        # ── directories ──
        (pack_dir / "permission_sets").mkdir()
        (pack_dir / "runtimes").mkdir()
        (pack_dir / "triggers").mkdir()
        (pack_dir / "actions" / "workflows").mkdir(parents=True)
        (pack_dir / "queues").mkdir()
        (pack_dir / "rules").mkdir()
        (pack_dir / "sensors").mkdir()

        # ── pack.yaml ──
        (pack_dir / "pack.yaml").write_text(
            f"""\
ref: {pack_ref}
name: {pack_ref}
label: Metadata E2E Pack
description: Synthetic pack for pack metadata deployment e2e test
version: 1.0.0
is_standard: false
"""
        )

        # ── permission_sets/executor.yaml ──
        (pack_dir / "permission_sets" / "executor.yaml").write_text(
            f"""\
ref: {pack_ref}.executor
label: Executor Permission Set
description: Grants action execution and key read access
grants:
  - resource: actions
    actions:
      - execute
    constraints:
      pack_refs:
        - {pack_ref}
  - resource: keys
    actions:
      - read
    constraints:
      owner_types:
        - pack
"""
        )

        # ── runtimes/custom_shell.yaml ──
        (pack_dir / "runtimes" / "custom_shell.yaml").write_text(
            f"""\
ref: {pack_ref}.custom_shell
name: Custom Shell
description: Custom shell runtime for e2e testing
execution_config:
  interpreter:
    binary: /bin/bash
    arguments:
      - "-e"
  file_extension: sh
  environment:
    create_command: ""
    install_command: ""
"""
        )

        # ── triggers/item_seen.yaml ──
        (pack_dir / "triggers" / "item_seen.yaml").write_text(
            f"""\
ref: {pack_ref}.item_seen
label: Item Seen
description: Fires when a new item is detected
enabled: true
parameters:
  item_id:
    type: string
    required: true
    description: Unique item identifier
  source:
    type: string
    description: Source system
"""
        )

        # ── actions/fetch.yaml + fetch.sh ──
        (pack_dir / "actions" / "fetch.yaml").write_text(
            f"""\
ref: {pack_ref}.fetch
name: fetch
label: Fetch Item
description: Fetches an item by ID
enabled: true
runner_type: shell
entry_point: fetch.sh
output_format: json
parameters:
  item_id:
    type: string
    required: true
    description: Item to fetch
default_execution_permission_set_refs:
  - {pack_ref}.executor
"""
        )
        (pack_dir / "actions" / "fetch.sh").write_text(
            '#!/usr/bin/env bash\nINPUT=$(cat)\necho \'{"status":"ok"}\'\n'
        )

        # ── actions/process.yaml + process.sh (dispatch target for queue) ──
        (pack_dir / "actions" / "process.yaml").write_text(
            f"""\
ref: {pack_ref}.process
name: process
label: Process Item
description: Processes a queued item
enabled: true
runner_type: shell
entry_point: process.sh
output_format: json
"""
        )
        (pack_dir / "actions" / "process.sh").write_text(
            '#!/usr/bin/env bash\nINPUT=$(cat)\necho \'{"processed":true}\'\n'
        )

        # ── workflow action: actions/deploy.yaml + actions/workflows/deploy.workflow.yaml ──
        (pack_dir / "actions" / "deploy.yaml").write_text(
            f"""\
ref: {pack_ref}.deploy
name: deploy
label: Deploy Workflow
description: Two-step workflow that fetches then processes
enabled: true
workflow_file: workflows/deploy.workflow.yaml
parameters:
  item_id:
    type: string
    required: true
    description: Item to deploy
output:
  status:
    type: string
    description: Deployment status
"""
        )
        (pack_dir / "actions" / "workflows" / "deploy.workflow.yaml").write_text(
            f"""\
version: "1.0"
vars:
  deploy_status: pending
tasks:
  - name: fetch_item
    action: {pack_ref}.fetch
    input:
      item_id: "{{{{ parameters.item_id }}}}"
    next:
      - when: "{{{{ succeeded() }}}}"
        publish:
          - deploy_status: fetched
        do:
          - process_item

  - name: process_item
    action: {pack_ref}.process
    input:
      item_id: "{{{{ parameters.item_id }}}}"
    next:
      - when: "{{{{ succeeded() }}}}"
        publish:
          - deploy_status: deployed

output_map:
  status: "{{{{ workflow.deploy_status }}}}"
"""
        )

        # ── queues/items.yaml ──
        (pack_dir / "queues" / "items.yaml").write_text(
            f"""\
ref: {pack_ref}.items
label: Items Queue
description: Queue for processing incoming items
enabled: false
accepting_new_items: true
dispatch_action: {pack_ref}.process
default_priority: 0
batch_mode: single
action_params:
  item: "{{{{ item }}}}"
config:
  dispatch:
    concurrency:
      source: literal
      value: 1
"""
        )

        # ── rules/on_item_seen.yaml ──
        (pack_dir / "rules" / "on_item_seen.yaml").write_text(
            f"""\
ref: on_item_seen
label: On Item Seen
description: When an item is seen, fetch it
trigger_ref: item_seen
action_ref: fetch
enabled: true
action_params:
  item_id: "{{{{ event.payload.item_id }}}}"
conditions: {{}}
"""
        )

        # ── sensors/poller.yaml + poller.sh ──
        (pack_dir / "sensors" / "poller.yaml").write_text(
            f"""\
ref: {pack_ref}.poller
label: Item Poller
description: Polls for new items
enabled: false
runner_type: shell
entry_point: poller.sh
trigger_types:
  - {pack_ref}.item_seen
worker_selector:
  zone: e2e
worker_tolerations:
  - key: dedicated
    operator: equal
    value: sensors
    effect: no_schedule
worker_affinity:
  required:
    - match_labels:
        storage: ssd
  preferred:
    - weight: 50
      preference:
        match_labels:
          zone: e2e
  anti_affinity:
    - match_labels:
        overloaded: "true"
parameters:
  interval:
    type: integer
    description: Polling interval in seconds
    default: 60
"""
        )
        (pack_dir / "sensors" / "poller.sh").write_text(
            '#!/usr/bin/env bash\necho "polling..."\n'
        )

    def test_all_metadata_deployed(self, client):
        """Upload pack and verify every metadata type via API."""
        uid = _uid()
        pack_ref = f"meta_e2e_{uid}"

        with TemporaryDirectory(prefix="attune-meta-e2e-") as tmp:
            pack_dir = Path(tmp) / pack_ref
            pack_dir.mkdir()
            self._build_pack(pack_dir, pack_ref)

            # Upload
            result = client.upload_pack(str(pack_dir), force=True)
            assert result["ref"] == pack_ref, f"Pack ref mismatch: {result}"

            # ── 1. Pack exists ──
            pack = _get(client, f"/api/v1/packs/{pack_ref}")
            assert pack["ref"] == pack_ref
            assert pack["version"] == "1.0.0"
            assert pack["description"] == "Synthetic pack for pack metadata deployment e2e test"

            # ── 2. Permission set exists ──
            perm_sets_resp = client.session.get(
                f"{client.base_url}/api/v1/permissions/sets",
                params={"pack_ref": pack_ref},
                timeout=15,
            )
            assert perm_sets_resp.status_code == 200, perm_sets_resp.text
            perm_data = perm_sets_resp.json()
            # API may return bare list or {"data": [...]}
            if isinstance(perm_data, list):
                perm_sets = perm_data
            else:
                perm_sets = perm_data.get("data", perm_data)
            if isinstance(perm_sets, list):
                matching = [
                    ps for ps in perm_sets
                    if ps.get("ref") == f"{pack_ref}.executor"
                ]
            else:
                matching = []
            assert len(matching) >= 1, (
                f"Permission set '{pack_ref}.executor' not found in {perm_sets}"
            )
            ps = matching[0]
            assert ps["pack_ref"] == pack_ref
            assert isinstance(ps["grants"], list)
            assert len(ps["grants"]) >= 2

            # ── 3. Runtime exists ──
            runtime = _get(client, f"/api/v1/runtimes/{pack_ref}.custom_shell")
            assert runtime["ref"] == f"{pack_ref}.custom_shell"
            assert runtime["name"] == "Custom Shell"
            assert runtime["pack_ref"] == pack_ref
            exec_config = runtime.get("execution_config", {})
            assert exec_config.get("interpreter", {}).get("binary") == "/bin/bash"

            # ── 4. Trigger exists ──
            trigger = _get(client, f"/api/v1/triggers/{pack_ref}.item_seen")
            assert trigger["ref"] == f"{pack_ref}.item_seen"
            assert trigger["label"] == "Item Seen"
            assert trigger["pack_ref"] == pack_ref
            assert trigger["enabled"] is True
            assert trigger.get("param_schema") is not None
            assert "item_id" in trigger["param_schema"]

            # ── 5. Regular action exists ──
            fetch_action = _get(client, f"/api/v1/actions/{pack_ref}.fetch")
            assert fetch_action["ref"] == f"{pack_ref}.fetch"
            assert fetch_action["label"] == "Fetch Item"
            assert fetch_action["pack_ref"] == pack_ref
            assert fetch_action.get("workflow_def") is None
            assert fetch_action.get("entrypoint") == "fetch.sh"
            assert f"{pack_ref}.executor" in fetch_action.get(
                "default_execution_permission_set_refs", []
            )

            # ── 6. Workflow action + definition exist ──
            deploy_action = _get(client, f"/api/v1/actions/{pack_ref}.deploy")
            assert deploy_action["ref"] == f"{pack_ref}.deploy"
            assert deploy_action["label"] == "Deploy Workflow"
            assert deploy_action.get("workflow_def") is not None, (
                "Workflow action should have a workflow_def ID"
            )

            workflow = _get(client, f"/api/v1/workflows/{pack_ref}.deploy")
            assert workflow["ref"] == f"{pack_ref}.deploy"
            definition = workflow.get("definition", {})
            tasks = definition.get("tasks", [])
            task_names = [t.get("name") for t in tasks]
            assert "fetch_item" in task_names, f"Expected fetch_item in tasks: {task_names}"
            assert "process_item" in task_names

            # ── 7. Queue exists ──
            queue = _get(client, f"/api/v1/queues/{pack_ref}.items")
            assert queue["ref"] == f"{pack_ref}.items"
            assert queue["label"] == "Items Queue"
            assert queue["dispatch_action_ref"] == f"{pack_ref}.process"
            assert queue["pack_ref"] == pack_ref
            assert queue["is_adhoc"] is False
            assert queue["enabled"] is False
            assert queue["batch_mode"] == "single"

            # ── 8. Rule exists ──
            rule = _get(client, f"/api/v1/rules/{pack_ref}.on_item_seen")
            assert rule["ref"] == f"{pack_ref}.on_item_seen"
            assert rule["label"] == "On Item Seen"
            assert rule["trigger_ref"] == f"{pack_ref}.item_seen"
            assert rule["action_ref"] == f"{pack_ref}.fetch"
            assert rule["is_adhoc"] is False
            assert rule["enabled"] is True
            assert rule.get("action_params", {}).get("item_id") == (
                "{{ event.payload.item_id }}"
            )

            # ── 9. Sensor exists ──
            sensor = _get(client, f"/api/v1/sensors/{pack_ref}.poller")
            assert sensor["ref"] == f"{pack_ref}.poller"
            assert sensor["label"] == "Item Poller"
            assert sensor["pack_ref"] == pack_ref
            assert sensor["enabled"] is False
            assert sensor.get("entrypoint") == "poller.sh"
            assert sensor.get("worker_selector") == {"zone": "e2e"}
            assert sensor.get("worker_tolerations") == [
                {
                    "key": "dedicated",
                    "operator": "equal",
                    "value": "sensors",
                    "effect": "no_schedule",
                }
            ]
            assert sensor.get("worker_affinity", {}).get("required", [])[0].get(
                "match_labels"
            ) == {"storage": "ssd"}
            assert sensor.get("worker_affinity", {}).get("preferred", [])[0].get(
                "weight"
            ) == 50
            assert sensor.get("worker_affinity", {}).get("anti_affinity", [])[0].get(
                "match_labels"
            ) == {"overloaded": "true"}

            # ── 10. Trigger-sensor linkage ──
            # Refresh the trigger to check sensor linkage
            trigger2 = _get(client, f"/api/v1/triggers/{pack_ref}.item_seen")
            assert trigger2.get("sensor_ref") == f"{pack_ref}.poller", (
                f"Trigger should be linked to sensor, got sensor_ref={trigger2.get('sensor_ref')}"
            )

        # ── Cleanup ──
        resp = client.session.delete(
            f"{client.base_url}/api/v1/packs/{pack_ref}", timeout=15
        )
        assert resp.status_code in (200, 204, 404), (
            f"Cleanup failed: {resp.status_code} {resp.text}"
        )

    def test_pack_reload_cleans_stale_rules(self, client):
        """
        Upload pack with a rule, then re-upload without that rule.
        The stale declarative rule should be removed while the pack survives.
        """
        uid = _uid()
        pack_ref = f"stale_e2e_{uid}"

        with TemporaryDirectory(prefix="attune-stale-rule-") as tmp:
            pack_dir = Path(tmp) / pack_ref
            pack_dir.mkdir()

            # Minimal pack with trigger + action + rule
            (pack_dir / "triggers").mkdir()
            (pack_dir / "actions").mkdir()
            (pack_dir / "rules").mkdir()

            (pack_dir / "pack.yaml").write_text(
                f"""\
ref: {pack_ref}
name: {pack_ref}
label: Stale Rule Test
version: 1.0.0
is_standard: false
"""
            )
            (pack_dir / "triggers" / "evt.yaml").write_text(
                f"""\
ref: {pack_ref}.evt
label: Event
enabled: true
"""
            )
            (pack_dir / "actions" / "noop.yaml").write_text(
                f"""\
ref: {pack_ref}.noop
name: noop
label: No-Op
enabled: true
runner_type: shell
entry_point: noop.sh
"""
            )
            (pack_dir / "actions" / "noop.sh").write_text(
                "#!/usr/bin/env bash\necho ok\n"
            )
            (pack_dir / "rules" / "stale.yaml").write_text(
                f"""\
ref: stale_rule
label: Stale Rule
trigger_ref: evt
action_ref: noop
enabled: true
"""
            )

            # First upload — rule should exist
            client.upload_pack(str(pack_dir), force=True)
            rule = _get(client, f"/api/v1/rules/{pack_ref}.stale_rule")
            assert rule["ref"] == f"{pack_ref}.stale_rule"

            # Remove the rule file and re-upload
            (pack_dir / "rules" / "stale.yaml").unlink()
            client.upload_pack(str(pack_dir), force=True)

            # Rule should be gone
            resp = client.session.get(
                f"{client.base_url}/api/v1/rules/{pack_ref}.stale_rule",
                timeout=15,
            )
            assert resp.status_code == 404, (
                f"Stale rule should have been deleted, got {resp.status_code}: {resp.text}"
            )

        # Cleanup
        client.session.delete(
            f"{client.base_url}/api/v1/packs/{pack_ref}", timeout=15
        )
