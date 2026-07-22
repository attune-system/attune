"""
Fixture Helpers for E2E Tests

Provides helper functions for creating test resources like packs,
triggers, actions, rules, etc.
"""

import time
import uuid
from datetime import datetime, timedelta, timezone
from pathlib import Path
import shutil
import tempfile
from typing import Any, Dict, Optional

from .client_wrapper import AttuneClient


def unique_ref(prefix: str = "test") -> str:
    """
    Generate unique reference string

    Args:
        prefix: Prefix for reference

    Returns:
        Unique reference string (e.g., "test_abc123")
    """
    timestamp = int(time.time() * 1000)
    random_part = str(uuid.uuid4())[:8]
    return f"{prefix}_{timestamp}_{random_part}"


def timestamp_now() -> str:
    """Get current timestamp in ISO format"""
    return datetime.now(timezone.utc).isoformat().replace("+00:00", "Z")


def timestamp_future(seconds: int) -> str:
    """
    Get future timestamp in ISO format

    Args:
        seconds: Seconds in the future

    Returns:
        ISO timestamp string
    """
    future = datetime.now(timezone.utc) + timedelta(seconds=seconds)
    return future.isoformat().replace("+00:00", "Z")


# ============================================================================
# Pack Creation
# ============================================================================


def create_test_pack(
    client: AttuneClient,
    pack_ref: Optional[str] = None,
    pack_dir: str = "tests/fixtures/packs/test_pack",
) -> Dict[str, Any]:
    """
    Create or get test pack

    Uses upload (tarball) to work across container boundaries.
    Falls back to register (filesystem path) for local development.

    Args:
        client: AttuneClient instance
        pack_ref: Optional pack reference (generated if not provided)
        pack_dir: Path to pack directory

    Returns:
        Pack data
    """
    # Extract pack_ref from pack_dir if not provided
    if not pack_ref:
        # Default pack ref is "test_pack" for the standard test pack
        pack_ref = "test_pack"

    # Always try to get existing pack first. Cache E2E fixtures include
    # executable action/sensor probes, so a debug stack created before those
    # fixtures changed must be refreshed rather than silently reused.
    existing_pack = client.get_pack_by_ref(pack_ref)
    if existing_pack:
        try:
            cache_probe = client.get_action_by_ref(f"{pack_ref}.cache_read")
            permission_sets = client.get(
                "/api/v1/permissions/sets", params={"pack_ref": pack_ref}
            )
            if permission_sets.status_code != 200:
                raise RuntimeError("Cannot inspect fixture cache permission set")
            permission_data = permission_sets.json()
            permission_items = (
                permission_data.get("data", permission_data)
                if isinstance(permission_data, dict)
                else permission_data
            )
            if isinstance(permission_items, dict):
                permission_items = permission_items.get("items", [])
            cache_reader = any(
                item.get("ref") == f"{pack_ref}.cache_reader"
                for item in permission_items
                if isinstance(item, dict)
            )
        except Exception:
            cache_probe = None
            cache_reader = False
        if cache_probe and cache_reader and (
            pack_ref == "test_pack" or client.list_actions(pack_ref=pack_ref)
        ):
            return existing_pack

    import os

    candidate_dir = None
    if os.path.isdir(pack_dir):
        candidate_dir = pack_dir
    else:
        # Try resolving relative to common locations
        for base in [".", "/app", os.path.dirname(os.path.dirname(__file__))]:
            candidate = os.path.join(base, pack_dir)
            if os.path.isdir(candidate):
                candidate_dir = candidate
                break

    if candidate_dir:
        if pack_ref == "test_pack":
            return client.upload_pack(candidate_dir, force=True)

        with tempfile.TemporaryDirectory(prefix=f"attune-e2e-{pack_ref}-") as tmp:
            isolated_pack = Path(tmp) / "pack"
            shutil.copytree(candidate_dir, isolated_pack)
            _rewrite_pack_ref(isolated_pack, "test_pack", pack_ref)
            return client.upload_pack(str(isolated_pack), force=True)

    raise FileNotFoundError(f"Pack directory not found: {pack_dir}")


def _rewrite_pack_ref(pack_dir: Path, old_ref: str, new_ref: str) -> None:
    """Rewrite the fixture pack ref so xdist workers do not share one pack."""
    for yaml_file in pack_dir.rglob("*.yaml"):
        content = yaml_file.read_text()
        content = re_sub_pack_ref(content, old_ref, new_ref)
        content = content.replace(f"{old_ref}.", f"{new_ref}.")
        if yaml_file.parent.name == "actions":
            content = _ensure_action_ref(content, new_ref)
        yaml_file.write_text(content)


def re_sub_pack_ref(content: str, old_ref: str, new_ref: str) -> str:
    lines = []
    replaced = False
    for line in content.splitlines():
        if not replaced and line.strip() == f"ref: {old_ref}":
            lines.append(line.replace(old_ref, new_ref, 1))
            replaced = True
        else:
            lines.append(line)
    return "\n".join(lines) + ("\n" if content.endswith("\n") else "")


def _ensure_action_ref(content: str, pack_ref: str) -> str:
    lines = content.splitlines()
    if any(line.strip().startswith("ref:") for line in lines):
        return content

    for index, line in enumerate(lines):
        stripped = line.strip()
        if stripped.startswith("name:"):
            action_name = stripped.split(":", 1)[1].strip().strip("\"'")
            if action_name:
                lines.insert(index + 1, f"ref: {pack_ref}.{action_name}")
                return "\n".join(lines) + ("\n" if content.endswith("\n") else "")
    return content


def ensure_core_pack(client: AttuneClient) -> Dict[str, Any]:
    """
    Ensure core pack exists, register it if needed

    Args:
        client: AttuneClient instance

    Returns:
        Core pack data
    """
    # Try to get existing core pack
    try:
        core_pack = client.get_pack_by_ref("core")
        if core_pack:
            return core_pack
    except Exception:
        pass

    # Core pack doesn't exist, register it
    try:
        return client.register_pack("packs/core", force=True, skip_tests=True)
    except Exception as e:
        # If registration fails, try without skip_tests
        try:
            return client.register_pack("packs/core", force=True)
        except Exception as inner_e:
            raise Exception(f"Failed to register core pack: {inner_e}") from e


# ============================================================================
# Trigger Creation
# ============================================================================


def create_interval_timer(
    client: AttuneClient,
    interval_seconds: int = 5,
    name: Optional[str] = None,
    pack_ref: str = "test.test_pack",
    action_ref: Optional[str] = None,
    action_parameters: Optional[Dict[str, Any]] = None,
    trigger_ref: Optional[str] = None,
    interval: Optional[int] = None,
    **kwargs,
) -> Dict[str, Any]:
    """
    Create interval timer trigger configuration.

    The core pack's built-in timer sensor (core.interval_timer_sensor) monitors
    all core.intervaltimer triggers via trigger instances (rules). This fixture
    creates the trigger and a rule so the core sensor will pick it up on its
    next poll cycle.

    Args:
        client: AttuneClient instance
        interval_seconds: Interval in seconds
        name: Timer name (generated if not provided)
        pack_ref: Pack reference
        action_ref: Action to invoke on timer fire (defaults to core.echo)
        action_parameters: Parameters to pass to the action

    Returns:
        Dict with trigger and rule info including the created rule
    """
    if interval is not None:
        interval_seconds = interval
    timer_name = name or trigger_ref or f"interval_{interval_seconds}s_{unique_ref()}"

    # Ensure core pack exists
    ensure_core_pack(client)

    # Get or ensure core.intervaltimer trigger exists
    triggers = client.list_triggers()
    core_trigger = None
    for t in triggers:
        if t.get("ref") == "core.intervaltimer":
            core_trigger = t
            break

    if not core_trigger:
        core_trigger = client.create_trigger(
            ref="core.intervaltimer",
            label="Interval Timer",
            pack_ref="core",
            description="Fires at regular intervals",
        )
    # Resolve action_ref
    if not action_ref:
        actions = client.list_actions()
        for a in actions:
            if a.get("ref") == "core.noop" or a.get("ref") == "core.echo":
                action_ref = a["ref"]
                break

    rule = None
    if action_ref:
        rule = client.create_rule(
            pack_ref=pack_ref,
            name=f"{timer_name}_rule",
            trigger_ref=core_trigger["ref"],
            action_ref=action_ref,
            enabled=True,
            trigger_params={
                "unit": "seconds",
                "interval": interval_seconds,
            },
            action_params=action_parameters or {},
        )

    # Return dict with trigger info (no sensor — core sensor handles it)
    return {
        "id": core_trigger["id"],
        "ref": core_trigger["ref"],
        "label": core_trigger.get("label", timer_name),
        "trigger": core_trigger,
        "sensor": {"id": 0, "ref": "core.interval_timer_sensor", "enabled": True},
        "sensor_id": 0,
        "rule": rule,
    }


def create_date_timer(
    client: AttuneClient,
    fire_at: Optional[str] = None,
    seconds_from_now: int = 5,
    name: Optional[str] = None,
    pack_ref: str = "test.test_pack",
    action_ref: Optional[str] = None,
    action_parameters: Optional[Dict[str, Any]] = None,
    trigger_ref: Optional[str] = None,
    date: Optional[str] = None,
    **kwargs,
) -> Dict[str, Any]:
    """
    Create date timer trigger configuration.

    The core timer sensor monitors core.datetimetimer triggers. This fixture
    creates a rule with trigger_params so the sensor will schedule the one-shot.

    Args:
        client: AttuneClient instance
        fire_at: ISO timestamp when to fire (optional)
        seconds_from_now: Seconds from now to fire (used if fire_at not provided)
        name: Timer name (generated if not provided)
        pack_ref: Pack reference
        action_ref: Action to invoke on timer fire (defaults to core.echo)
        action_parameters: Parameters to pass to the action

    Returns:
        Dict with trigger and rule info including the created rule
    """
    fire_at = fire_at or date
    if not fire_at:
        fire_at = timestamp_future(seconds_from_now)

    timer_name = name or trigger_ref or f"date_{unique_ref()}"

    # Ensure core pack exists
    ensure_core_pack(client)

    # Get or ensure core.datetimetimer trigger exists
    triggers = client.list_triggers()
    core_trigger = None
    for t in triggers:
        if t.get("ref") == "core.datetimetimer":
            core_trigger = t
            break

    if not core_trigger:
        core_trigger = client.create_trigger(
            ref="core.datetimetimer",
            label="Date/Time Timer",
            pack_ref="core",
            description="Fires at a specific date/time",
        )
    # Resolve action_ref
    if not action_ref:
        actions = client.list_actions()
        for a in actions:
            if a.get("ref") == "core.noop" or a.get("ref") == "core.echo":
                action_ref = a["ref"]
                break

    rule = None
    if action_ref:
        rule = client.create_rule(
            pack_ref=pack_ref,
            name=f"{timer_name}_rule",
            trigger_ref=core_trigger["ref"],
            action_ref=action_ref,
            enabled=True,
            trigger_params={
                "fire_at": fire_at,
                "timezone": "UTC",
            },
            action_params=action_parameters or {},
        )

    return {
        "id": core_trigger["id"],
        "ref": core_trigger["ref"],
        "label": core_trigger.get("label", timer_name),
        "trigger": core_trigger,
        "sensor": {"id": 0, "ref": "core.interval_timer_sensor", "enabled": True},
        "sensor_id": 0,
        "fire_at": fire_at,
        "rule": rule,
    }


def create_cron_timer(
    client: AttuneClient,
    cron_expression: str = "*/5 * * * * *",
    name: Optional[str] = None,
    pack_ref: str = "test.test_pack",
    timezone: str = "UTC",
    action_ref: Optional[str] = None,
    action_parameters: Optional[Dict[str, Any]] = None,
) -> Dict[str, Any]:
    """
    Create cron timer trigger configuration.

    The core timer sensor monitors core.crontimer triggers. This fixture
    creates a rule with trigger_params so the sensor will schedule the cron.

    Args:
        client: AttuneClient instance
        cron_expression: Cron expression (6-field with seconds)
        name: Timer name (generated if not provided)
        pack_ref: Pack reference
        timezone: Timezone for cron evaluation
        action_ref: Action to invoke on timer fire (defaults to core.echo)
        action_parameters: Parameters to pass to the action

    Returns:
        Dict with trigger and rule info including the created rule
    """
    timer_name = name or f"cron_{unique_ref()}"

    # Ensure core pack exists
    ensure_core_pack(client)

    # Get or ensure core.crontimer trigger exists
    triggers = client.list_triggers()
    core_trigger = None
    for t in triggers:
        if t.get("ref") == "core.crontimer":
            core_trigger = t
            break

    if not core_trigger:
        core_trigger = client.create_trigger(
            ref="core.crontimer",
            label="Cron Timer",
            pack_ref="core",
            description="Fires based on cron schedule",
        )

    # Resolve action_ref
    if not action_ref:
        actions = client.list_actions()
        for a in actions:
            if a.get("ref") == "core.noop" or a.get("ref") == "core.echo":
                action_ref = a["ref"]
                break

    rule = None
    if action_ref:
        rule = client.create_rule(
            pack_ref=pack_ref,
            name=f"{timer_name}_rule",
            trigger_ref=core_trigger["ref"],
            action_ref=action_ref,
            enabled=True,
            trigger_params={
                "expression": cron_expression,
                "timezone": timezone,
            },
            action_params=action_parameters or {},
        )

    return {
        "id": core_trigger["id"],
        "ref": core_trigger["ref"],
        "label": core_trigger.get("label", timer_name),
        "trigger": core_trigger,
        "sensor": {"id": 0, "ref": "core.interval_timer_sensor", "enabled": True},
        "sensor_id": 0,
        "rule": rule,
    }


def create_webhook_trigger(
    client: AttuneClient,
    name: Optional[str] = None,
    trigger_name: Optional[str] = None,
    pack_ref: str = "test.test_pack",
    trigger_ref: Optional[str] = None,
    description: Optional[str] = None,
    **kwargs,
) -> Dict[str, Any]:
    """
    Create webhook trigger

    Args:
        client: AttuneClient instance
        name: Trigger name (generated if not provided)
        trigger_name: Optional trigger name
        pack_ref: Pack reference

    Returns:
        Created trigger data
    """
    trigger_name = name or trigger_name
    if not trigger_name and trigger_ref:
        trigger_name = trigger_ref.split(".", 1)[1] if "." in trigger_ref else trigger_ref
    trigger_name = trigger_name or f"webhook_{unique_ref()}"
    trigger_ref = trigger_ref or f"{pack_ref}.{trigger_name}"
    if "." not in trigger_ref:
        trigger_ref = f"{pack_ref}.{trigger_ref}"

    trigger = client.create_trigger(
        ref=trigger_ref,
        label=trigger_name.replace("_", " ").title(),
        pack_ref=pack_ref,
        trigger_type="webhook",
        description=description,
        parameters={},
    )

    # Enable webhook and construct webhook_url for tests.
    trigger_ref = trigger.get("ref", f"{pack_ref}.{trigger_name}")
    trigger = client.enable_webhook(trigger_ref=trigger_ref)

    # Ensure webhook_url is present for test convenience
    if "webhook_url" not in trigger and "webhook_key" in trigger:
        trigger["webhook_url"] = f"/api/v1/webhooks/{trigger['webhook_key']}"
    return trigger


# ============================================================================
# Action Creation
# ============================================================================


def create_simple_action(
    client: AttuneClient,
    name: Optional[str] = None,
    pack_ref: str = "test.test_pack",
    runtime_ref: str = "core.python",
    entrypoint: str = "actions/echo.py",
    param_schema: Optional[Dict[str, Any]] = None,
) -> Dict[str, Any]:
    """
    Create simple action

    Args:
        client: AttuneClient instance
        name: Action name (generated if not provided)
        pack_ref: Pack reference
        runtime_ref: Runtime reference
        entrypoint: Entry point path
        param_schema: JSON Schema for parameters

    Returns:
        Created action data
    """
    action_name = name or f"action_{unique_ref()}"

    if param_schema is None:
        param_schema = {
            "message": {"type": "string", "default": "Hello, World!"},
        }

    return client.create_action(
        pack_ref=pack_ref,
        name=action_name,
        runtime_ref=runtime_ref,
        entrypoint=entrypoint,
        param_schema=param_schema,
    )


def create_echo_action(
    client: AttuneClient,
    name: Optional[str] = None,
    action_name: Optional[str] = None,
    pack_ref: str = "test.test_pack",
    echo_message: Optional[str] = None,
    action_ref: Optional[str] = None,
    message: Optional[str] = None,
    suffix: Optional[str] = None,
    description: Optional[str] = None,
    **kwargs,
) -> Dict[str, Any]:
    """
    Create echo action (simple action that echoes input)

    Args:
        client: AttuneClient instance
        name: Action name (generated if not provided)
        action_name: Optional action name
        pack_ref: Pack reference
        echo_message: Default message (used in param_schema default)

    Returns:
        Created action data
    """
    if action_ref and not name and not action_name:
        name = action_ref.split(".", 1)[1] if "." in action_ref else action_ref
    if suffix and not name and not action_name:
        name = f"echo_{unique_ref()}{suffix}"

    return create_simple_action(
        client=client,
        name=name or action_name or f"echo_{unique_ref()}",
        pack_ref=pack_ref,
        runtime_ref="core.shell",
        entrypoint='INPUT=$(cat); printf \'{"success":true,"input":%s}\\n\' "${INPUT:-{}}"',
        param_schema={
            "message": {
                "type": "string",
                "default": echo_message or message or "echo",
            },
            "count": {"type": "integer", "default": 1},
        },
    )


def create_failing_action(
    client: AttuneClient,
    name: Optional[str] = None,
    pack_ref: str = "test.test_pack",
    exit_code: int = 1,
) -> Dict[str, Any]:
    """
    Create action that always fails

    Args:
        client: AttuneClient instance
        name: Action name (generated if not provided)
        pack_ref: Pack reference
        exit_code: Exit code to return (always 1 for shell version)

    Returns:
        Created action data
    """
    action_name = name or f"failing_{unique_ref()}"

    return client.create_action(
        pack_ref=pack_ref,
        name=action_name,
        runtime_ref="core.shell",
        entrypoint=f"echo '{{\"error\":\"Action intentionally failed\",\"exit_code\":{exit_code}}}' >&2; exit {exit_code}",
        param_schema={
            "exit_code": {"type": "integer", "default": exit_code},
        },
    )


def create_sleep_action(
    client: AttuneClient,
    name: Optional[str] = None,
    pack_ref: str = "test.test_pack",
    default_duration: int = 5,
) -> Dict[str, Any]:
    """
    Create action that sleeps for specified duration

    Args:
        client: AttuneClient instance
        name: Action name (generated if not provided)
        pack_ref: Pack reference
        default_duration: Default sleep duration in seconds

    Returns:
        Created action data
    """
    action_name = name or f"sleep_{unique_ref()}"

    return client.create_action(
        pack_ref=pack_ref,
        name=action_name,
        runtime_ref="core.shell",
        entrypoint="INPUT=$(cat); DURATION=$(echo \"$INPUT\" | sed -n 's/.*\"duration\"[[:space:]]*:[[:space:]]*\\([0-9]*\\).*/\\1/p'); DURATION=${DURATION:-%d}; sleep \"$DURATION\"; printf '{\"success\":true,\"slept\":%s}\\n' \"$DURATION\"" % (default_duration, "$DURATION"),
        param_schema={
            "duration": {"type": "integer", "default": default_duration},
        },
    )


# ============================================================================
# Rule Creation
# ============================================================================


def create_rule(
    client: AttuneClient,
    trigger_ref: str,
    action_ref: str,
    name: Optional[str] = None,
    pack_ref: str = "test.test_pack",
    enabled: bool = True,
    conditions: Optional[Dict[str, Any]] = None,
    action_params: Optional[Dict[str, Any]] = None,
    trigger_params: Optional[Dict[str, Any]] = None,
) -> Dict[str, Any]:
    """
    Create rule

    Args:
        client: AttuneClient instance
        trigger_ref: Trigger reference to monitor
        action_ref: Action reference to execute
        name: Rule name (generated if not provided)
        pack_ref: Pack reference
        enabled: Whether rule is enabled
        conditions: Rule condition expression object
        action_params: Parameters to pass to action
        trigger_params: Parameters for the trigger (required if trigger has param_schema)

    Returns:
        Created rule data
    """
    rule_name = name or f"rule_{unique_ref()}"

    if trigger_params is None:
        if trigger_ref == "core.intervaltimer":
            trigger_params = {"unit": "seconds", "interval": 60}
        elif trigger_ref == "core.crontimer":
            trigger_params = {"expression": "0 0 * * * *"}
        elif trigger_ref == "core.datetimetimer":
            trigger_params = {"fire_at": "2099-12-31T23:59:59Z"}

    return client.create_rule(
        name=rule_name,
        pack_ref=pack_ref,
        trigger_ref=trigger_ref,
        action_ref=action_ref,
        enabled=enabled,
        conditions=conditions,
        action_params=action_params or {},
        trigger_params=trigger_params,
    )


def create_timer_automation(
    client: AttuneClient,
    interval_seconds: int = 5,
    pack_ref: str = "test.test_pack",
    action_parameters: Optional[Dict[str, Any]] = None,
) -> Dict[str, Any]:
    """
    Create complete timer automation (trigger + action + rule)

    Args:
        client: AttuneClient instance
        interval_seconds: Timer interval in seconds
        pack_ref: Pack reference
        action_parameters: Parameters to pass to action

    Returns:
        Dictionary with trigger, action, and rule data
    """
    # Create timer trigger
    trigger = create_interval_timer(
        client=client, interval_seconds=interval_seconds, pack_ref=pack_ref
    )

    # Create echo action
    action = create_echo_action(client=client, pack_ref=pack_ref)

    # Create rule linking them
    rule = create_rule(
        client=client,
        trigger_ref=trigger["ref"],
        action_ref=action["ref"],
        pack_ref=pack_ref,
        action_params=action_parameters,
        trigger_params={"unit": "seconds", "interval": interval_seconds},
    )

    return {"trigger": trigger, "action": action, "rule": rule}


def create_webhook_automation(
    client: AttuneClient,
    pack_ref: str = "test.test_pack",
    action_parameters: Optional[Dict[str, Any]] = None,
    conditions: Optional[Dict[str, Any]] = None,
) -> Dict[str, Any]:
    """
    Create complete webhook automation (trigger + action + rule)

    Args:
        client: AttuneClient instance
        pack_ref: Pack reference
        action_parameters: Parameters to pass to action
        conditions: Optional rule conditions

    Returns:
        Dictionary with trigger, action, and rule data
    """
    # Create webhook trigger
    trigger = create_webhook_trigger(client=client, pack_ref=pack_ref)

    # Create echo action
    action = create_echo_action(client=client, pack_ref=pack_ref)

    # Create rule linking them
    rule = create_rule(
        client=client,
        trigger_ref=trigger["ref"],
        action_ref=action["ref"],
        pack_ref=pack_ref,
        action_params=action_parameters,
        conditions=conditions,
    )

    return {"trigger": trigger, "action": action, "rule": rule}


# ============================================================================
# Service Management
# ============================================================================


def restart_sensor_service(wait_seconds: int = 3) -> bool:
    """
    Restart sensor service to reload sensors

    This is needed after creating new sensors so they are loaded
    and can start generating events.

    Works with E2E services managed by start-e2e-services.sh script.
    In a Docker Compose environment the test container cannot restart
    other services, so this is a best-effort no-op that logs a warning.

    Args:
        wait_seconds: Seconds to wait after restart for service to be ready

    Returns:
        True if restart successful, False otherwise
    """
    import os
    import signal
    import subprocess

    try:
        # In Docker Compose, the test container has no control over sibling
        # services. Log a warning and return False so callers can adapt.
        if os.path.exists("/.dockerenv") or os.getenv("DOCKER_ENV"):
            print(
                "Warning: Cannot restart sensor service from inside a Docker "
                "container. Sensors created after service startup will not be "
                "picked up until the sensor service is restarted externally."
            )
            return False

        # For E2E services, use PID file to restart the sensor service
        # Calculate paths relative to tests directory
        helpers_dir = os.path.dirname(os.path.abspath(__file__))  # tests/helpers
        tests_dir = os.path.dirname(helpers_dir)  # tests
        pid_dir = os.path.join(tests_dir, "pids")
        pid_file = os.path.join(pid_dir, "sensor.pid")
        log_dir = os.path.join(tests_dir, "logs")
        log_file = os.path.join(log_dir, "sensor.log")

        if os.path.exists(pid_file):
            # Read PID and stop the service
            with open(pid_file, "r") as f:
                pid_str = f.read().strip()
                if not pid_str:
                    print(f"Warning: PID file {pid_file} is empty")
                    os.remove(pid_file)
                    return False
                pid = int(pid_str)

            # Stop the existing process
            stopped = False
            try:
                # Send SIGTERM for graceful shutdown
                os.kill(pid, signal.SIGTERM)

                # Wait up to 5 seconds for graceful shutdown
                for _ in range(10):
                    try:
                        os.kill(pid, 0)  # Check if process exists
                        time.sleep(0.5)
                    except ProcessLookupError:
                        stopped = True
                        break

                # Force kill if still running
                if not stopped:
                    try:
                        os.kill(pid, signal.SIGKILL)
                        time.sleep(1)
                        stopped = True
                    except ProcessLookupError:
                        stopped = True
            except ProcessLookupError:
                stopped = True  # Process doesn't exist

            # Remove PID file
            if os.path.exists(pid_file):
                os.remove(pid_file)

            if not stopped:
                print(f"Warning: Failed to stop sensor process {pid}")

            # Restart the sensor service
            # Get project root
            project_root = os.path.dirname(tests_dir)  # project root
            binary_path = os.path.join(project_root, "target", "debug", "attune-sensor")
            config_file = os.path.join(project_root, "config.e2e.yaml")

            # Verify binary exists
            if not os.path.exists(binary_path):
                print(f"Error: Sensor binary not found at {binary_path}")
                return False

            env = os.environ.copy()
            env["ATTUNE__ENVIRONMENT"] = "e2e"
            env["ATTUNE_CONFIG"] = config_file

            with open(log_file, "a") as log:
                log.write(f"\n\n=== Sensor service restarted at {time.time()} ===\n\n")
                log.flush()
                process = subprocess.Popen(
                    [binary_path],
                    stdout=log,
                    stderr=subprocess.STDOUT,
                    env=env,
                    start_new_session=True,
                    cwd=project_root,  # Run from project root
                )

            # Write new PID file
            with open(pid_file, "w") as f:
                f.write(str(process.pid))
                f.flush()

            # Wait for service to initialize
            time.sleep(wait_seconds)

            # Verify process is still running
            try:
                os.kill(process.pid, 0)

                # Additional verification: check if log file is being written to
                if os.path.exists(log_file):
                    # Get file size before and after a short wait
                    size_before = os.path.getsize(log_file)
                    time.sleep(1)
                    size_after = os.path.getsize(log_file)

                    if size_after > size_before:
                        print(
                            f"✓ Sensor service restarted successfully (PID: {process.pid})"
                        )
                        return True
                    else:
                        print(
                            f"Warning: Sensor service started but not logging (PID: {process.pid})"
                        )
                        return True  # Still return True as process is running
                else:
                    print(f"✓ Sensor service restarted (PID: {process.pid})")
                    return True
            except ProcessLookupError:
                print(f"✗ Sensor service failed to start (process died immediately)")
                if os.path.exists(pid_file):
                    os.remove(pid_file)
                return False
        else:
            print(
                f"Warning: No PID file found at {pid_file}. Sensor service may not be running."
            )
            time.sleep(wait_seconds)
            return False

    except Exception as e:
        print(f"Warning: Error restarting sensor service: {e}")
        import traceback

        traceback.print_exc()
        time.sleep(wait_seconds)
        return False
