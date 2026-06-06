#!/usr/bin/env python3
"""
Pack Loader for Attune

This script loads a pack from the filesystem into the database.
It reads pack.yaml, permission set definitions, action definitions, trigger,
rule, and sensor definitions and creates all necessary database entries.

Usage:
    python3 scripts/load_core_pack.py [--database-url URL] [--pack-dir DIR] [--pack-name NAME]

Environment Variables:
    DATABASE_URL: PostgreSQL connection string (default: from config or localhost)
    ATTUNE_PACKS_DIR: Base directory for packs (default: ./packs)
"""

import argparse
import json
import os
import re
import sys
from pathlib import Path
from typing import Any, Dict, List, Optional, Tuple

import psycopg2
import psycopg2.extras
from psycopg2 import sql
import yaml

# Default configuration
DEFAULT_DATABASE_URL = "postgresql://postgres:postgres@localhost:5432/attune"
DEFAULT_PACKS_DIR = "./packs"
SCHEMA_RE = re.compile(r"^[A-Za-z_][A-Za-z0-9_]*$")


def generate_label(name: str) -> str:
    """Generate a human-readable label from a name.

    Examples:
        'crontimer' -> 'Crontimer'
        'http_request' -> 'Http Request'
        'datetime_timer' -> 'Datetime Timer'
    """
    # Replace underscores with spaces and capitalize each word
    return " ".join(word.capitalize() for word in name.replace("_", " ").split())


def extract_version_components(
    version: str,
) -> tuple[Optional[int], Optional[int], Optional[int]]:
    """Extract major/minor/patch integers from a version string.

    Accepts lenient semver-style inputs like ``3``, ``3.12``, ``3.12.1``,
    and ignores any suffix after the numeric prefix.
    """

    match = re.match(r"^\s*(\d+)(?:\.(\d+))?(?:\.(\d+))?", version)
    if not match:
        return None, None, None

    major = int(match.group(1)) if match.group(1) is not None else None
    minor = int(match.group(2)) if match.group(2) is not None else None
    patch = int(match.group(3)) if match.group(3) is not None else None
    return major, minor, patch


class PackLoader:
    """Loads a pack into the database"""

    def __init__(
        self, database_url: str, packs_dir: Path, pack_name: str, schema: str = "attune"
    ):
        self.database_url = database_url
        self.packs_dir = packs_dir
        self.pack_name = pack_name
        self.pack_dir = packs_dir / pack_name
        self.schema = schema
        self.conn = None
        self.pack_id = None
        self.pack_ref = None

    def connect(self):
        """Connect to the database"""
        print(f"Connecting to database...")
        self.conn = psycopg2.connect(self.database_url)
        self.conn.autocommit = False

        # Set search_path to use the correct schema
        if not SCHEMA_RE.match(self.schema):
            raise ValueError(f"Invalid schema name: {self.schema}")
        cursor = self.conn.cursor()
        # nosemgrep: python.sqlalchemy.security.sqlalchemy-execute-raw-query.sqlalchemy-execute-raw-query -- This uses psycopg2.sql.Identifier for safe identifier composition after schema-name validation.
        cursor.execute(
            sql.SQL("SET search_path TO {}, public").format(sql.Identifier(self.schema))
        )
        cursor.close()
        self.conn.commit()

        print(f"✓ Connected to database (schema: {self.schema})")

    def close(self):
        """Close database connection"""
        if self.conn:
            self.conn.close()

    def load_yaml(self, file_path: Path) -> Dict[str, Any]:
        """Load and parse YAML file"""
        with open(file_path, "r") as f:
            return yaml.safe_load(f)

    def resolve_pack_relative_path(self, base_dir: Path, relative_path: str) -> Path:
        """Resolve a pack-owned relative path and reject traversal outside the pack."""
        candidate = (base_dir / relative_path).resolve()
        pack_root = self.pack_dir.resolve()
        if not candidate.is_relative_to(pack_root):
            raise ValueError(
                f"Resolved path '{candidate}' escapes pack root '{pack_root}'"
            )
        return candidate

    def upsert_pack(self) -> int:
        """Create or update the pack"""
        print("\n→ Loading pack metadata...")

        pack_yaml_path = self.pack_dir / "pack.yaml"
        if not pack_yaml_path.exists():
            raise FileNotFoundError(f"pack.yaml not found at {pack_yaml_path}")

        pack_data = self.load_yaml(pack_yaml_path)

        cursor = self.conn.cursor()

        # Prepare pack data
        ref = pack_data["ref"]
        self.pack_ref = ref
        label = pack_data["label"]
        description = pack_data.get("description", "")
        version = pack_data["version"]
        conf_schema = json.dumps(pack_data.get("conf_schema", {}))
        config = json.dumps(pack_data.get("config", {}))
        meta = json.dumps(pack_data.get("meta", {}))
        tags = pack_data.get("tags", [])
        runtime_deps = pack_data.get("runtime_deps", [])
        is_standard = pack_data.get("system", False)

        # Upsert pack
        cursor.execute(
            """
            INSERT INTO pack (
                ref, label, description, version,
                conf_schema, config, meta, tags, runtime_deps, is_standard
            )
            VALUES (%s, %s, %s, %s, %s, %s, %s, %s, %s, %s)
            ON CONFLICT (ref) DO UPDATE SET
                label = EXCLUDED.label,
                description = EXCLUDED.description,
                version = EXCLUDED.version,
                conf_schema = EXCLUDED.conf_schema,
                config = EXCLUDED.config,
                meta = EXCLUDED.meta,
                tags = EXCLUDED.tags,
                runtime_deps = EXCLUDED.runtime_deps,
                is_standard = EXCLUDED.is_standard,
                updated = NOW()
            RETURNING id
        """,
            (
                ref,
                label,
                description,
                version,
                conf_schema,
                config,
                meta,
                tags,
                runtime_deps,
                is_standard,
            ),
        )

        self.pack_id = cursor.fetchone()[0]
        cursor.close()

        print(f"✓ Pack '{ref}' loaded (ID: {self.pack_id})")
        return self.pack_id

    def upsert_permission_sets(self) -> Dict[str, int]:
        """Load permission set definitions from permission_sets/*.yaml."""
        print("\n→ Loading permission sets...")

        permission_sets_dir = self.pack_dir / "permission_sets"
        if not permission_sets_dir.exists():
            print("  No permission_sets directory found")
            return {}

        permission_set_ids = {}
        cursor = self.conn.cursor()

        for yaml_file in sorted(permission_sets_dir.glob("*.yaml")):
            permission_set_data = self.load_yaml(yaml_file)
            if not permission_set_data:
                continue

            ref = permission_set_data.get("ref")
            if not ref:
                print(
                    f"  ⚠ Permission set YAML {yaml_file.name} missing 'ref' field, skipping"
                )
                continue

            label = permission_set_data.get("label")
            description = permission_set_data.get("description")
            grants = permission_set_data.get("grants", [])

            if not isinstance(grants, list):
                print(
                    f"  ⚠ Permission set '{ref}' has non-array grants, skipping"
                )
                continue

            cursor.execute(
                """
                INSERT INTO permission_set (
                    ref, pack, pack_ref, label, description, grants
                )
                VALUES (%s, %s, %s, %s, %s, %s)
                ON CONFLICT (ref) DO UPDATE SET
                    label = EXCLUDED.label,
                    description = EXCLUDED.description,
                    grants = EXCLUDED.grants,
                    updated = NOW()
                RETURNING id
            """,
                (
                    ref,
                    self.pack_id,
                    self.pack_ref,
                    label,
                    description,
                    json.dumps(grants),
                ),
            )

            permission_set_id = cursor.fetchone()[0]
            permission_set_ids[ref] = permission_set_id
            print(f"  ✓ Permission set '{ref}' (ID: {permission_set_id})")

        cursor.close()
        return permission_set_ids

    def upsert_triggers(self) -> Dict[str, int]:
        """Load trigger definitions"""
        print("\n→ Loading triggers...")

        triggers_dir = self.pack_dir / "triggers"
        if not triggers_dir.exists():
            print("  No triggers directory found")
            return {}

        trigger_ids = {}
        cursor = self.conn.cursor()

        for yaml_file in sorted(triggers_dir.glob("*.yaml")):
            trigger_data = self.load_yaml(yaml_file)

            # Use ref from YAML (new format) or construct from name (old format)
            ref = trigger_data.get("ref")
            if not ref:
                # Fallback for old format - should not happen with new pack format
                ref = f"{self.pack_ref}.{trigger_data['name']}"

            # Extract name from ref for label generation
            name = ref.split(".")[-1] if "." in ref else ref
            label = trigger_data.get("label") or generate_label(name)
            description = trigger_data.get("description", "")
            enabled = trigger_data.get("enabled", True)
            param_schema = json.dumps(trigger_data.get("parameters", {}))
            out_schema = json.dumps(trigger_data.get("output", {}))

            cursor.execute(
                """
                INSERT INTO trigger (
                    ref, pack, pack_ref, label, description,
                    enabled, param_schema, out_schema, is_adhoc
                )
                VALUES (%s, %s, %s, %s, %s, %s, %s, %s, %s)
                ON CONFLICT (ref) DO UPDATE SET
                    label = EXCLUDED.label,
                    description = EXCLUDED.description,
                    enabled = EXCLUDED.enabled,
                    param_schema = EXCLUDED.param_schema,
                    out_schema = EXCLUDED.out_schema,
                    updated = NOW()
                RETURNING id
            """,
                (
                    ref,
                    self.pack_id,
                    self.pack_ref,
                    label,
                    description,
                    enabled,
                    param_schema,
                    out_schema,
                    False,  # Pack-installed triggers are not ad-hoc
                ),
            )

            trigger_id = cursor.fetchone()[0]
            trigger_ids[ref] = trigger_id
            print(f"  ✓ Trigger '{ref}' (ID: {trigger_id})")

        cursor.close()
        return trigger_ids

    def upsert_runtimes(self) -> Tuple[Dict[str, int], int]:
        """Load runtime definitions from runtimes/*.yaml"""
        print("\n→ Loading runtimes...")

        runtime_ids, pack_runtime_ids = self.load_runtime_lookup()
        runtimes_dir = self.pack_dir / "runtimes"
        if not runtimes_dir.exists():
            print("  No runtimes directory found")
            return runtime_ids, 0

        cursor = self.conn.cursor()

        for yaml_file in sorted(runtimes_dir.glob("*.yaml")):
            runtime_data = self.load_yaml(yaml_file)
            if not runtime_data:
                continue

            ref = runtime_data.get("ref")
            if not ref:
                print(
                    f"  ⚠ Runtime YAML {yaml_file.name} missing 'ref' field, skipping"
                )
                continue

            name = runtime_data.get("name", ref.split(".")[-1])
            description = runtime_data.get("description", "")
            aliases = [alias.lower() for alias in runtime_data.get("aliases", [])]
            distributions = json.dumps(runtime_data.get("distributions", {}))
            installation = json.dumps(runtime_data.get("installation", {}))
            execution_config = json.dumps(runtime_data.get("execution_config", {}))

            cursor.execute(
                """
                INSERT INTO runtime (
                    ref, pack, pack_ref, name, description,
                    aliases, distributions, installation, execution_config
                )
                VALUES (%s, %s, %s, %s, %s, %s, %s, %s, %s)
                ON CONFLICT (ref) DO UPDATE SET
                    name = EXCLUDED.name,
                    description = EXCLUDED.description,
                    aliases = EXCLUDED.aliases,
                    distributions = EXCLUDED.distributions,
                    installation = EXCLUDED.installation,
                    execution_config = EXCLUDED.execution_config,
                    updated = NOW()
                RETURNING id
            """,
                (
                    ref,
                    self.pack_id,
                    self.pack_ref,
                    name,
                    description,
                    aliases,
                    distributions,
                    installation,
                    execution_config,
                ),
            )

            runtime_id = cursor.fetchone()[0]
            runtime_ids[ref] = runtime_id
            pack_runtime_ids.add(runtime_id)
            # Also index by lowercase name for easy lookup by runner_type
            runtime_ids[name.lower()] = runtime_id
            for alias in aliases:
                runtime_ids[alias] = runtime_id
            print(f"  ✓ Runtime '{ref}' (ID: {runtime_id})")

            self.upsert_runtime_versions(cursor, runtime_id, ref, runtime_data)

        cursor.close()
        return runtime_ids, len(pack_runtime_ids)

    def load_runtime_lookup(self) -> Tuple[Dict[str, int], set[int]]:
        """Load existing runtime refs, names, and aliases from the database."""
        cursor = self.conn.cursor()
        cursor.execute("SELECT id, ref, name, aliases, pack_ref FROM runtime")

        runtime_ids: Dict[str, int] = {}
        pack_runtime_ids = set()

        for runtime_id, runtime_ref, name, aliases, pack_ref in cursor.fetchall():
            runtime_ids[runtime_ref] = runtime_id
            if isinstance(name, str) and name:
                runtime_ids[name.lower()] = runtime_id
            if aliases:
                for alias in aliases:
                    if isinstance(alias, str) and alias:
                        runtime_ids[alias.lower()] = runtime_id
            if pack_ref == self.pack_ref:
                pack_runtime_ids.add(runtime_id)

        cursor.close()
        return runtime_ids, pack_runtime_ids

    def upsert_runtime_versions(
        self, cursor, runtime_id: int, runtime_ref: str, runtime_data: Dict[str, Any]
    ) -> int:
        """Load version-specific runtime definitions from a runtime YAML."""
        versions = runtime_data.get("versions")
        if versions is None:
            return 0

        if not isinstance(versions, list):
            print(
                f"  ⚠ Runtime '{runtime_ref}' has non-array 'versions' field, skipping version load"
            )
            return 0

        declared_versions: List[str] = []

        for entry in versions:
            if not isinstance(entry, dict):
                print(
                    f"  ⚠ Runtime '{runtime_ref}' has invalid version entry (expected object), skipping"
                )
                continue

            version = entry.get("version")
            if not isinstance(version, str) or not version.strip():
                print(
                    f"  ⚠ Runtime '{runtime_ref}' has a version entry without a valid 'version' field, skipping"
                )
                continue

            version = version.strip()
            declared_versions.append(version)
            version_major, version_minor, version_patch = extract_version_components(version)
            execution_config = json.dumps(entry.get("execution_config", {}))
            distributions = json.dumps(entry.get("distributions", {}))
            is_default = bool(entry.get("is_default", False))
            meta = json.dumps(entry.get("meta", {}))

            cursor.execute(
                """
                INSERT INTO runtime_version (
                    runtime, runtime_ref, version,
                    version_major, version_minor, version_patch,
                    execution_config, distributions,
                    is_default, available, meta
                )
                VALUES (%s, %s, %s, %s, %s, %s, %s, %s, %s, %s, %s)
                ON CONFLICT (runtime, version) DO UPDATE SET
                    runtime_ref = EXCLUDED.runtime_ref,
                    version_major = EXCLUDED.version_major,
                    version_minor = EXCLUDED.version_minor,
                    version_patch = EXCLUDED.version_patch,
                    execution_config = EXCLUDED.execution_config,
                    distributions = EXCLUDED.distributions,
                    is_default = EXCLUDED.is_default,
                    meta = EXCLUDED.meta,
                    updated = NOW()
                RETURNING id
            """,
                (
                    runtime_id,
                    runtime_ref,
                    version,
                    version_major,
                    version_minor,
                    version_patch,
                    execution_config,
                    distributions,
                    is_default,
                    True,
                    meta,
                ),
            )
            version_id = cursor.fetchone()[0]
            print(
                f"    ✓ Runtime version '{runtime_ref}' {version} (ID: {version_id})"
            )

        if declared_versions:
            cursor.execute(
                """
                DELETE FROM runtime_version
                WHERE runtime = %s
                  AND NOT (version = ANY(%s))
            """,
                (runtime_id, declared_versions),
            )
        else:
            cursor.execute("DELETE FROM runtime_version WHERE runtime = %s", (runtime_id,))

        return len(declared_versions)

    def resolve_action_runtime(
        self, action_data: Dict, runtime_ids: Dict[str, int]
    ) -> Optional[int]:
        """Resolve the runtime ID for an action based on runner_type or entrypoint."""
        runner_type = action_data.get("runner_type", "").lower()

        if not runner_type:
            # Try to infer from entrypoint extension
            entrypoint = action_data.get("entry_point", "")
            if entrypoint.endswith(".py"):
                runner_type = "python"
            elif entrypoint.endswith(".js"):
                runner_type = "node.js"
            else:
                runner_type = "shell"

        # Map runner_type names to runtime refs/names
        lookup_keys = {
            "shell": ["shell", "core.shell"],
            "python": ["python", "core.python"],
            "python3": ["python", "core.python"],
            "node": ["node.js", "nodejs", "core.nodejs"],
            "nodejs": ["node.js", "nodejs", "core.nodejs"],
            "node.js": ["node.js", "nodejs", "core.nodejs"],
            "native": ["native", "core.native"],
        }

        keys_to_try = lookup_keys.get(runner_type, [runner_type])
        for key in keys_to_try:
            if key in runtime_ids:
                return runtime_ids[key]

        print(f"  ⚠ Could not resolve runtime for runner_type '{runner_type}'")
        return None

    def upsert_workflow_definition(
        self,
        cursor,
        workflow_file_path: str,
        action_ref: str,
        action_data: Dict[str, Any],
    ) -> Optional[int]:
        """Load a workflow definition file and upsert it in the database.

        When an action YAML contains a `workflow_file` field, this method reads
        the referenced workflow YAML, creates or updates the corresponding
        `workflow_definition` row, and returns its ID so the action can be linked
        via the `workflow_def` FK.

        The action YAML's `parameters` and `output` fields take precedence over
        the workflow file's own schemas (allowing the action to customise the
        exposed interface without touching the workflow graph).

        Args:
            cursor: Database cursor.
            workflow_file_path: Path to the workflow file relative to the
                ``actions/`` directory (e.g. ``workflows/deploy.workflow.yaml``).
            action_ref: The ref of the action that references this workflow.
            action_data: The parsed action YAML dict (used for schema overrides).

        Returns:
            The database ID of the workflow_definition row, or None on failure.
        """
        actions_dir = self.pack_dir / "actions"
        full_path = self.resolve_pack_relative_path(actions_dir, workflow_file_path)
        if not full_path.exists():
            print(f"  ⚠ Workflow file '{workflow_file_path}' not found at {full_path}")
            return None

        try:
            workflow_data = self.load_yaml(full_path)
        except Exception as e:
            print(f"  ⚠ Failed to parse workflow file '{workflow_file_path}': {e}")
            return None

        # The action YAML is authoritative for action-level metadata.
        # Fall back to the workflow file's own values only when present
        # (standalone workflow files in workflows/ still carry them).
        workflow_ref = workflow_data.get("ref") or action_ref
        label = workflow_data.get("label") or action_data.get("label", "")
        description = workflow_data.get("description") or action_data.get(
            "description", ""
        )
        version = workflow_data.get("version", "1.0.0")
        tags = workflow_data.get("tags") or action_data.get("tags", [])

        # The action YAML is authoritative for param_schema / out_schema.
        # Fall back to the workflow file's own schemas only if the action
        # YAML doesn't define them.
        param_schema = action_data.get("parameters") or workflow_data.get("parameters")
        out_schema = action_data.get("output") or workflow_data.get("output")

        param_schema_json = json.dumps(param_schema) if param_schema else None
        out_schema_json = json.dumps(out_schema) if out_schema else None

        # Store the full workflow definition as JSON
        definition_json = json.dumps(workflow_data)
        tags_list = tags if isinstance(tags, list) else []

        cursor.execute(
            """
            INSERT INTO workflow_definition (
                ref, pack, pack_ref, label, description, version,
                param_schema, out_schema, definition, tags
            )
            VALUES (%s, %s, %s, %s, %s, %s, %s, %s, %s, %s)
            ON CONFLICT (ref) DO UPDATE SET
                label = EXCLUDED.label,
                description = EXCLUDED.description,
                version = EXCLUDED.version,
                param_schema = EXCLUDED.param_schema,
                out_schema = EXCLUDED.out_schema,
                definition = EXCLUDED.definition,
                tags = EXCLUDED.tags,
                updated = NOW()
            RETURNING id
        """,
            (
                workflow_ref,
                self.pack_id,
                self.pack_ref,
                label,
                description,
                version,
                param_schema_json,
                out_schema_json,
                definition_json,
                tags_list,
            ),
        )

        workflow_def_id = cursor.fetchone()[0]
        print(f"    ✓ Workflow definition '{workflow_ref}' (ID: {workflow_def_id})")
        return workflow_def_id

    def upsert_actions(self, runtime_ids: Dict[str, int]) -> Dict[str, int]:
        """Load action definitions.

        When an action YAML contains a ``workflow_file`` field, the loader reads
        the referenced workflow definition, upserts a ``workflow_definition``
        record, and links the action to it via ``action.workflow_def``.  This
        allows the action YAML to control action-level metadata independently
        of the workflow graph, and lets multiple actions share a workflow file.
        """
        print("\n→ Loading actions...")

        actions_dir = self.pack_dir / "actions"
        if not actions_dir.exists():
            print("  No actions directory found")
            return {}

        action_ids = {}
        workflow_count = 0
        cursor = self.conn.cursor()

        for yaml_file in sorted(actions_dir.glob("*.yaml")):
            action_data = self.load_yaml(yaml_file)

            # Use ref from YAML (new format) or construct from name (old format)
            ref = action_data.get("ref")
            if not ref:
                # Fallback for old format - should not happen with new pack format
                ref = f"{self.pack_ref}.{action_data['name']}"

            # Extract name from ref for label generation and entrypoint detection
            name = ref.split(".")[-1] if "." in ref else ref
            label = action_data.get("label") or generate_label(name)
            description = action_data.get("description", "")

            # ── Workflow file handling ───────────────────────────────────
            workflow_file = action_data.get("workflow_file")
            workflow_def_id: Optional[int] = None

            if workflow_file:
                workflow_def_id = self.upsert_workflow_definition(
                    cursor, workflow_file, ref, action_data
                )
                if workflow_def_id is not None:
                    workflow_count += 1

            # For workflow actions the entrypoint is the workflow file path;
            # for regular actions it comes from entry_point in the YAML.
            if workflow_file:
                entrypoint = workflow_file
            else:
                entrypoint = action_data.get("entry_point", "")
                if not entrypoint:
                    # Try to find corresponding script file
                    for ext in [".sh", ".py"]:
                        script_path = actions_dir / f"{name}{ext}"
                        if script_path.exists():
                            entrypoint = str(script_path.relative_to(self.packs_dir))
                            break

            # Resolve runtime ID (workflow actions have no runtime)
            if workflow_file:
                runtime_id = None
            else:
                runtime_id = self.resolve_action_runtime(action_data, runtime_ids)

            runtime_version_constraint = action_data.get("runtime_version")

            # Optional default execution timeout (seconds). Must be positive when set.
            timeout_seconds = action_data.get("timeout_seconds")
            if timeout_seconds is not None:
                try:
                    timeout_seconds = int(timeout_seconds)
                    if timeout_seconds <= 0:
                        print(
                            f"  ⚠ Invalid timeout_seconds '{timeout_seconds}' for '{ref}', ignoring"
                        )
                        timeout_seconds = None
                except (TypeError, ValueError):
                    print(
                        f"  ⚠ Invalid timeout_seconds '{timeout_seconds}' for '{ref}', ignoring"
                    )
                    timeout_seconds = None

            param_schema = json.dumps(action_data.get("parameters", {}))
            out_schema = json.dumps(action_data.get("output", {}))

            # Parameter delivery and format (defaults: stdin + json for security)
            parameter_delivery = action_data.get("parameter_delivery", "stdin").lower()
            parameter_format = action_data.get("parameter_format", "json").lower()

            # Output format (defaults: text for no parsing)
            output_format = action_data.get("output_format", "text").lower()

            # Validate parameter delivery method (only stdin and file allowed)
            if parameter_delivery not in ["stdin", "file"]:
                print(
                    f"  ⚠ Invalid parameter_delivery '{parameter_delivery}' for '{ref}', defaulting to 'stdin'"
                )
                parameter_delivery = "stdin"

            # Validate parameter format
            if parameter_format not in ["dotenv", "json", "yaml"]:
                print(
                    f"  ⚠ Invalid parameter_format '{parameter_format}' for '{ref}', defaulting to 'json'"
                )
                parameter_format = "json"

            # Validate output format
            if output_format not in ["text", "json", "yaml", "jsonl"]:
                print(
                    f"  ⚠ Invalid output_format '{output_format}' for '{ref}', defaulting to 'text'"
                )
                output_format = "text"

            cursor.execute(
                """
                INSERT INTO action (
                    ref, pack, pack_ref, label, description,
                    entrypoint, runtime, runtime_version_constraint,
                    param_schema, out_schema, is_adhoc,
                    parameter_delivery, parameter_format, output_format,
                    timeout_seconds
                )
                VALUES (%s, %s, %s, %s, %s, %s, %s, %s, %s, %s, %s, %s, %s, %s, %s)
                ON CONFLICT (ref) DO UPDATE SET
                    label = EXCLUDED.label,
                    description = EXCLUDED.description,
                    entrypoint = EXCLUDED.entrypoint,
                    runtime = EXCLUDED.runtime,
                    runtime_version_constraint = EXCLUDED.runtime_version_constraint,
                    param_schema = EXCLUDED.param_schema,
                    out_schema = EXCLUDED.out_schema,
                    parameter_delivery = EXCLUDED.parameter_delivery,
                    parameter_format = EXCLUDED.parameter_format,
                    output_format = EXCLUDED.output_format,
                    timeout_seconds = EXCLUDED.timeout_seconds,
                    updated = NOW()
                RETURNING id
            """,
                (
                    ref,
                    self.pack_id,
                    self.pack_ref,
                    label,
                    description,
                    entrypoint,
                    runtime_id,
                    runtime_version_constraint,
                    param_schema,
                    out_schema,
                    False,  # Pack-installed actions are not ad-hoc
                    parameter_delivery,
                    parameter_format,
                    output_format,
                    timeout_seconds,
                ),
            )

            action_id = cursor.fetchone()[0]
            action_ids[ref] = action_id

            # Link action to workflow definition if present
            if workflow_def_id is not None:
                cursor.execute(
                    """
                    UPDATE action SET workflow_def = %s, updated = NOW()
                    WHERE id = %s
                """,
                    (workflow_def_id, action_id),
                )
                print(
                    f"  ✓ Action '{ref}' (ID: {action_id}) → workflow def {workflow_def_id}"
                )
            else:
                print(f"  ✓ Action '{ref}' (ID: {action_id})")

        cursor.close()
        if workflow_count > 0:
            print(f"  ({workflow_count} workflow definition(s) registered)")
        return action_ids

    def upsert_sensors(
        self, trigger_ids: Dict[str, int], runtime_ids: Dict[str, int]
    ) -> Dict[str, int]:
        """Load sensor definitions"""
        print("\n→ Loading sensors...")

        sensors_dir = self.pack_dir / "sensors"
        if not sensors_dir.exists():
            print("  No sensors directory found")
            return {}

        sensor_ids = {}
        cursor = self.conn.cursor()

        # Runtime name mapping: runner_type values to core runtime refs
        runner_type_to_ref = {
            "native": "core.native",
            "standalone": "core.native",
            "builtin": "core.native",
            "shell": "core.shell",
            "bash": "core.shell",
            "sh": "core.shell",
            "python": "core.python",
            "python3": "core.python",
            "node": "core.nodejs",
            "nodejs": "core.nodejs",
            "node.js": "core.nodejs",
        }

        for yaml_file in sorted(sensors_dir.glob("*.yaml")):
            sensor_data = self.load_yaml(yaml_file)

            # Use ref from YAML (new format) or construct from name (old format)
            ref = sensor_data.get("ref")
            if not ref:
                # Fallback for old format - should not happen with new pack format
                ref = f"{self.pack_ref}.{sensor_data['name']}"

            # Extract name from ref for label generation and entrypoint detection
            name = ref.split(".")[-1] if "." in ref else ref
            label = sensor_data.get("label") or generate_label(name)
            description = sensor_data.get("description", "")
            enabled = sensor_data.get("enabled", True)

            # Get trigger reference (handle both trigger_type and trigger_types)
            trigger_types = sensor_data.get("trigger_types", [])
            if not trigger_types:
                # Fallback to singular trigger_type
                trigger_type = sensor_data.get("trigger_type", "")
                trigger_types = [trigger_type] if trigger_type else []

            # Use the first trigger type (sensors currently support one trigger)
            trigger_ref = None
            trigger_id = None
            if trigger_types:
                # Check if it's already a full ref or just the type name
                first_trigger = trigger_types[0]
                if "." in first_trigger:
                    trigger_ref = first_trigger
                else:
                    trigger_ref = f"{self.pack_ref}.{first_trigger}"
                trigger_id = trigger_ids.get(trigger_ref)

            # Resolve sensor runtime from YAML runner_type field
            # Defaults to "native" (compiled binary, no interpreter)
            runner_type = sensor_data.get("runner_type", "native").lower()
            runtime_ref = runner_type_to_ref.get(runner_type, runner_type)
            # Look up runtime ID: try the mapped ref, then the raw runner_type
            sensor_runtime_id = runtime_ids.get(runtime_ref)
            if not sensor_runtime_id:
                # Try looking up by the short name (e.g., "python" key in runtime_ids)
                sensor_runtime_id = runtime_ids.get(runner_type)
            if not sensor_runtime_id:
                raise ValueError(
                    f"Sensor '{ref}' declares runner_type '{runner_type}' "
                    f"but no matching runtime was found (expected ref: {runtime_ref})"
                )

            # Determine entrypoint
            entry_point = sensor_data.get("entry_point", "")
            if not entry_point:
                for ext in [".py", ".sh"]:
                    script_path = sensors_dir / f"{name}{ext}"
                    if script_path.exists():
                        entry_point = str(script_path.relative_to(self.packs_dir))
                        break

            config = json.dumps(sensor_data.get("config", {}))
            runtime_version_constraint = sensor_data.get("runtime_version")

            cursor.execute(
                """
                INSERT INTO sensor (
                    ref, pack, pack_ref, label, description,
                    entrypoint, runtime, runtime_ref,
                    runtime_version_constraint, enabled, config
                )
                VALUES (%s, %s, %s, %s, %s, %s, %s, %s, %s, %s, %s)
                ON CONFLICT (ref) DO UPDATE SET
                    label = EXCLUDED.label,
                    description = EXCLUDED.description,
                    entrypoint = EXCLUDED.entrypoint,
                    runtime = EXCLUDED.runtime,
                    runtime_ref = EXCLUDED.runtime_ref,
                    runtime_version_constraint = EXCLUDED.runtime_version_constraint,
                    enabled = EXCLUDED.enabled,
                    config = EXCLUDED.config,
                    updated = NOW()
                RETURNING id
            """,
                (
                    ref,
                    self.pack_id,
                    self.pack_ref,
                    label,
                    description,
                    entry_point,
                    sensor_runtime_id,
                    runtime_ref,
                    runtime_version_constraint,
                    enabled,
                    config,
                ),
            )

            sensor_id = cursor.fetchone()[0]
            sensor_ids[ref] = sensor_id

            # Link triggers to this sensor (trigger→sensor relationship)
            for ttype in trigger_types:
                tref = ttype if "." in ttype else f"{self.pack_ref}.{ttype}"
                tid = trigger_ids.get(tref)
                if tid:
                    cursor.execute(
                        """
                        UPDATE trigger SET sensor = %s, sensor_ref = %s
                        WHERE id = %s
                        """,
                        (sensor_id, ref, tid),
                    )
                    print(f"    → Linked trigger '{tref}' to sensor '{ref}'")
                else:
                    print(f"    ⚠ Trigger '{tref}' not found for sensor '{ref}'")

            print(f"  ✓ Sensor '{ref}' (ID: {sensor_id})")

        cursor.close()
        return sensor_ids

    def upsert_rules(
        self, trigger_ids: Dict[str, int], action_ids: Dict[str, int]
    ) -> Dict[str, int]:
        """Load declarative rule definitions from rules/*.yaml."""
        print("\n→ Loading rules...")

        rules_dir = self.pack_dir / "rules"
        if not rules_dir.exists():
            print("  No rules directory found")
            return {}

        rule_ids = {}
        cursor = self.conn.cursor()

        def qualify(ref: str) -> str:
            return ref if "." in ref else f"{self.pack_ref}.{ref}"

        def find_entity_id(table: str, ref: str) -> Optional[int]:
            if table == "trigger":
                cursor.execute("SELECT id FROM trigger WHERE ref = %s", (ref,))
            elif table == "action":
                cursor.execute("SELECT id FROM action WHERE ref = %s", (ref,))
            else:
                raise ValueError(f"Unsupported lookup table: {table}")
            row = cursor.fetchone()
            return row[0] if row else None

        for yaml_file in sorted(rules_dir.glob("*.yaml")):
            rule_data = self.load_yaml(yaml_file)
            if not rule_data:
                continue

            ref = rule_data.get("ref")
            if not ref:
                print(f"  ⚠ Rule YAML {yaml_file.name} missing 'ref' field, skipping")
                continue
            ref = qualify(ref)

            trigger_ref = rule_data.get("trigger_ref")
            action_ref = rule_data.get("action_ref")
            if not trigger_ref or not action_ref:
                print(
                    f"  ⚠ Rule '{ref}' must define both 'trigger_ref' and 'action_ref', skipping"
                )
                continue

            trigger_ref = qualify(trigger_ref)
            action_ref = qualify(action_ref)
            trigger_id = trigger_ids.get(trigger_ref) or find_entity_id("trigger", trigger_ref)
            action_id = action_ids.get(action_ref) or find_entity_id("action", action_ref)

            if not trigger_id:
                print(f"  ⚠ Rule '{ref}' references unknown trigger '{trigger_ref}', skipping")
                continue
            if not action_id:
                print(f"  ⚠ Rule '{ref}' references unknown action '{action_ref}', skipping")
                continue

            name = ref.split(".")[-1] if "." in ref else ref
            label = rule_data.get("label") or generate_label(name)
            description = rule_data.get("description", "")
            enabled = rule_data.get("enabled", True)
            conditions = json.dumps(rule_data.get("conditions", {}))
            action_params = json.dumps(rule_data.get("action_params", {}))
            trigger_params = json.dumps(rule_data.get("trigger_params", {}))

            cursor.execute(
                """
                INSERT INTO rule (
                    ref, pack, pack_ref, label, description,
                    action, action_ref, trigger, trigger_ref,
                    conditions, action_params, trigger_params,
                    enabled, is_adhoc, owner_identity
                )
                VALUES (%s, %s, %s, %s, %s, %s, %s, %s, %s, %s, %s, %s, %s, false, NULL)
                ON CONFLICT (ref) DO UPDATE SET
                    pack = EXCLUDED.pack,
                    pack_ref = EXCLUDED.pack_ref,
                    label = EXCLUDED.label,
                    description = EXCLUDED.description,
                    action = EXCLUDED.action,
                    action_ref = EXCLUDED.action_ref,
                    trigger = EXCLUDED.trigger,
                    trigger_ref = EXCLUDED.trigger_ref,
                    conditions = EXCLUDED.conditions,
                    action_params = EXCLUDED.action_params,
                    trigger_params = EXCLUDED.trigger_params,
                    enabled = EXCLUDED.enabled,
                    is_adhoc = false,
                    owner_identity = NULL,
                    updated = NOW()
                RETURNING id
            """,
                (
                    ref,
                    self.pack_id,
                    self.pack_ref,
                    label,
                    description,
                    action_id,
                    action_ref,
                    trigger_id,
                    trigger_ref,
                    conditions,
                    action_params,
                    trigger_params,
                    enabled,
                ),
            )

            rule_id = cursor.fetchone()[0]
            rule_ids[ref] = rule_id
            print(f"  ✓ Rule '{ref}' (ID: {rule_id})")

        cursor.close()
        return rule_ids

    def load_pack(self):
        """Main loading process.

        Components are loaded in dependency order:
        1. Permission sets (no dependencies)
        2. Runtimes (no dependencies)
        3. Triggers (no dependencies)
        4. Actions (depend on runtime; workflow actions also create
           workflow_definition records)
        5. Rules (depend on triggers and actions)
        6. Sensors (depend on triggers and runtime)
        """
        print("=" * 60)
        print(f"Pack Loader - {self.pack_name}")
        print("=" * 60)

        if not self.pack_dir.exists():
            raise FileNotFoundError(f"Pack directory not found: {self.pack_dir}")

        try:
            self.connect()

            # Load pack metadata
            self.upsert_pack()

            # Load permission sets first (authorization metadata)
            permission_set_ids = self.upsert_permission_sets()

            # Load runtimes (actions and sensors depend on them)
            runtime_ids, runtime_count = self.upsert_runtimes()

            # Load triggers
            trigger_ids = self.upsert_triggers()

            # Load actions (with runtime resolution + workflow definitions)
            action_ids = self.upsert_actions(runtime_ids)

            # Load rules
            rule_ids = self.upsert_rules(trigger_ids, action_ids)

            # Load sensors
            sensor_ids = self.upsert_sensors(trigger_ids, runtime_ids)

            # Commit all changes
            self.conn.commit()

            print("\n" + "=" * 60)
            print(f"✓ Pack '{self.pack_name}' loaded successfully!")
            print("=" * 60)
            print(f"  Pack ID: {self.pack_id}")
            print(f"  Permission sets: {len(permission_set_ids)}")
            print(f"  Runtimes: {runtime_count}")
            print(f"  Triggers: {len(trigger_ids)}")
            print(f"  Actions: {len(action_ids)}")
            print(f"  Rules: {len(rule_ids)}")
            print(f"  Sensors: {len(sensor_ids)}")
            print()

        except Exception as e:
            if self.conn:
                self.conn.rollback()
            print(f"\n✗ Error loading pack '{self.pack_name}': {e}")
            import traceback

            traceback.print_exc()
            sys.exit(1)
        finally:
            self.close()


def main():
    parser = argparse.ArgumentParser(description="Load a pack into the Attune database")
    parser.add_argument(
        "--database-url",
        default=os.getenv("DATABASE_URL", DEFAULT_DATABASE_URL),
        help=f"PostgreSQL connection string (default: {DEFAULT_DATABASE_URL})",
    )
    parser.add_argument(
        "--pack-dir",
        type=Path,
        default=Path(os.getenv("ATTUNE_PACKS_DIR", DEFAULT_PACKS_DIR)),
        help=f"Base directory for packs (default: {DEFAULT_PACKS_DIR})",
    )
    parser.add_argument(
        "--pack-name",
        default="core",
        help="Name of the pack to load (default: core)",
    )
    parser.add_argument(
        "--schema",
        default=os.getenv("DB_SCHEMA", "attune"),
        help="Database schema to use (default: attune)",
    )
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="Print what would be done without making changes",
    )

    args = parser.parse_args()

    if args.dry_run:
        print("DRY RUN MODE: No changes will be made")
        print()

    loader = PackLoader(args.database_url, args.pack_dir, args.pack_name, args.schema)
    loader.load_pack()


if __name__ == "__main__":
    main()
