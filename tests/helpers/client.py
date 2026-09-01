"""
AttuneClient Helper

Provides a high-level client for interacting with the Attune API
during end-to-end testing.
"""

import os
import time
from typing import Any, Dict, List, Optional

import psycopg
import requests
from requests.adapters import HTTPAdapter
from requests.models import Response
from urllib3.util.retry import Retry


class AttuneClient:
    """High-level client for Attune API testing"""

    def __init__(
        self,
        base_url: Optional[str] = None,
        timeout: int = 30,
        auto_login: bool = True,
    ):
        """
        Initialize Attune API client

        Args:
            base_url: API base URL (defaults to ATTUNE_API_URL env var or localhost)
            timeout: Default request timeout in seconds
            auto_login: Automatically login if token not present
        """
        self.base_url = (
            base_url or os.getenv("ATTUNE_API_URL", "http://localhost:8080")
        ).rstrip("/")
        self.timeout = timeout
        self.auto_login = auto_login
        self.session = requests.Session()
        self.token: Optional[str] = None
        self.user_id: Optional[int] = None
        self.tenant_id: Optional[int] = None

        # Configure retry strategy for flaky network conditions
        retry_strategy = Retry(
            total=3,
            backoff_factor=0.5,
            status_forcelist=[429, 500, 502, 503, 504],
            allowed_methods=[
                "HEAD",
                "GET",
                "PUT",
                "DELETE",
                "OPTIONS",
                "TRACE",
                "POST",
            ],
        )
        adapter = HTTPAdapter(max_retries=retry_strategy)
        self.session.mount("http://", adapter)
        self.session.mount("https://", adapter)

    # ========================================================================
    # Authentication
    # ========================================================================

    def register(
        self,
        login: str = "test@attune.local",
        password: str = "TestPass123!",
        display_name: str = "Test User",
    ) -> Dict[str, Any]:
        """
        Register a new user

        Args:
            login: User login (email or username)
            password: User password
            display_name: User's display name

        Returns:
            Registration response with user data
        """
        response = self.session.post(
            f"{self.base_url}/auth/register",
            json={
                "login": login,
                "password": password,
                "display_name": display_name,
            },
            timeout=self.timeout,
        )
        response.raise_for_status()
        return response.json()

    def login(
        self,
        login: str = "test@attune.local",
        password: str = "TestPass123!",
        create_if_missing: bool = True,
    ) -> str:
        """
        Authenticate and get JWT token

        Args:
            login: User login
            password: User password
            create_if_missing: Auto-register if user doesn't exist

        Returns:
            JWT access token
        """
        try:
            response = self.session.post(
                f"{self.base_url}/auth/login",
                json={"login": login, "password": password},
                timeout=self.timeout,
            )
            response.raise_for_status()
            data = response.json()

            self.token = data["data"]["access_token"]
            self.user_id = data["data"].get("user_id")
            self.tenant_id = data["data"].get("tenant_id")

            # Update session headers
            self.session.headers.update({"Authorization": f"Bearer {self.token}"})

            return self.token

        except requests.exceptions.HTTPError as e:
            if e.response.status_code in [401, 404] and create_if_missing:
                # User doesn't exist, try to register
                self.register(login, password)
                # Retry login
                return self.login(login, password, create_if_missing=False)
            raise

    def logout(self):
        """Clear authentication token"""
        self.token = None
        self.user_id = None
        self.tenant_id = None
        if "Authorization" in self.session.headers:
            del self.session.headers["Authorization"]

    # ========================================================================
    # Core Request Methods
    # ========================================================================

    def _request(
        self, method: str, path: str, auto_auth: bool = True, **kwargs
    ) -> Dict[str, Any]:
        """
        Make authenticated API request

        Args:
            method: HTTP method
            path: API path (relative to base_url)
            auto_auth: Automatically login if not authenticated
            **kwargs: Additional request arguments

        Returns:
            JSON response data
        """
        # Auto-login if needed
        if (
            auto_auth
            and not self.token
            and path not in ["/auth/login", "/auth/register"]
        ):
            if self.auto_login:
                self.login()
            else:
                raise RuntimeError(
                    "Not authenticated. Call login() first or set auto_login=True"
                )

        # Build full URL
        url = f"{self.base_url}{path}"

        # Set default timeout if not provided
        if "timeout" not in kwargs:
            kwargs["timeout"] = self.timeout

        # Make request
        response = self.session.request(method, url, **kwargs)
        response.raise_for_status()

        # Parse JSON response
        return response.json()

    def get(self, path: str, **kwargs) -> Dict[str, Any]:
        """GET request"""
        return self._request("GET", path, **kwargs)

    def post(self, path: str, **kwargs) -> Dict[str, Any]:
        """POST request"""
        return self._request("POST", path, **kwargs)

    def put(self, path: str, **kwargs) -> Dict[str, Any]:
        """PUT request"""
        return self._request("PUT", path, **kwargs)

    def patch(self, path: str, **kwargs) -> Dict[str, Any]:
        """PATCH request"""
        return self._request("PATCH", path, **kwargs)

    def delete(self, path: str, **kwargs) -> Dict[str, Any]:
        """DELETE request"""
        return self._request("DELETE", path, **kwargs)

    # ========================================================================
    # Health Check
    # ========================================================================

    def health(self) -> Dict[str, Any]:
        """Check API health"""
        return self.get("/health", auto_auth=False)

    # ========================================================================
    # Pack Management
    # ========================================================================

    def list_packs(self) -> List[Dict[str, Any]]:
        """List all packs"""
        response = self.get("/api/v1/packs")
        return response["data"]

    def get_pack(self, pack_id: int) -> Dict[str, Any]:
        """Get pack by ID"""
        response = self.get(f"/api/v1/packs/{pack_id}")
        return response["data"]

    def get_pack_by_ref(self, pack_ref: str) -> Optional[Dict[str, Any]]:
        """Get pack by reference (namespace.name)"""
        packs = self.list_packs()
        for pack in packs:
            if pack["ref"] == pack_ref:
                return pack
        return None

    def create_pack(
        self,
        pack_data: Dict[str, Any] = None,
        ref: str = None,
        label: str = None,
        version: str = "1.0.0",
        description: str = None,
        conf_schema: Dict[str, Any] = None,
        config: Dict[str, Any] = None,
        meta: Dict[str, Any] = None,
        tags: List[str] = None,
        **kwargs,
    ) -> Dict[str, Any]:
        """
        Create a new pack

        Args:
            pack_data: Dict containing pack data (alternative to keyword args)
            ref: Unique reference identifier (e.g., "slack", "aws")
            label: Human-readable label
            version: Pack version (default: "1.0.0")
            description: Pack description
            conf_schema: Configuration schema (JSON Schema)
            config: Pack configuration values
            meta: Pack metadata
            tags: Tags for categorization

        Returns:
            Created pack data
        """
        # If pack_data dict is provided, use it as base
        if pack_data:
            payload = {
                "ref": pack_data.get("ref", ref),
                "label": pack_data.get("label")
                or pack_data.get("name")
                or pack_data.get("ref", ref),
                "version": pack_data.get("version", version),
                "conf_schema": pack_data.get("conf_schema", {}),
                "config": pack_data.get("config", {}),
                "meta": pack_data.get("meta", {}),
                "tags": pack_data.get("tags", []),
            }
            if "description" in pack_data:
                payload["description"] = pack_data["description"]
        else:
            # Use keyword arguments
            payload = {
                "ref": ref,
                "label": label or kwargs.get("name") or ref,
                "version": version,
                "conf_schema": conf_schema or {},
                "config": config or {},
                "meta": meta or {},
                "tags": tags or [],
            }
            if description:
                payload["description"] = description

        response = self.post("/api/v1/packs", json=payload)
        return response["data"]

    def register_pack(
        self, pack_dir: str, skip_tests: bool = True, force: bool = False
    ) -> Dict[str, Any]:
        """
        Register pack from directory

        Args:
            pack_dir: Path to pack directory containing pack.yaml
            skip_tests: Skip running pack tests during registration (default: True)
            force: Force re-registration if pack already exists (default: False)

        Returns:
            Created pack data
        """
        response = self.post(
            "/api/v1/packs/register",
            json={"path": pack_dir, "skip_tests": skip_tests, "force": force},
        )
        return response["data"]

    def reload_pack(self, pack_id: int) -> Dict[str, Any]:
        """Reload pack (refresh metadata and actions)"""
        response = self.post(f"/api/v1/packs/{pack_id}/reload")
        return response["data"]

    def delete_pack(self, pack_id: int) -> Dict[str, Any]:
        """Delete pack"""
        return self.delete(f"/api/v1/packs/{pack_id}")

    # ========================================================================
    # Runtime Management
    # ========================================================================

    def list_runtimes(self) -> List[Dict[str, Any]]:
        """
        List all runtimes

        Returns:
            List of runtimes
        """
        response = self.get("/api/v1/runtimes")
        return response["data"]

    # ========================================================================
    # Action Management
    # ========================================================================

    def list_actions(self, pack_ref: Optional[str] = None) -> List[Dict[str, Any]]:
        """
        List actions

        Args:
            pack_ref: Optional filter by pack reference

        Returns:
            List of actions
        """
        params = {}
        if pack_ref:
            params["pack_ref"] = pack_ref

        response = self.get("/api/v1/actions", params=params)
        return response["data"]

    def get_action(self, action_id: int) -> Dict[str, Any]:
        """Get action by ID"""
        response = self.get(f"/api/v1/actions/{action_id}")
        return response["data"]

    def get_action_by_ref(self, action_ref: str) -> Optional[Dict[str, Any]]:
        """Get action by reference (pack.action_name)"""
        actions = self.list_actions()
        for action in actions:
            if action["ref"] == action_ref:
                return action
        return None

    def create_action(
        self,
        pack_ref: str = None,
        name: str = None,
        runner_type: str = None,
        entrypoint: str = "",
        param_schema: Optional[Dict[str, Any]] = None,
        ref: str = None,
        label: str = None,
        description: str = None,
        runtime: int = None,
        runtime_ref: str = None,
        out_schema: Optional[Dict[str, Any]] = None,
        **kwargs,
    ) -> Dict[str, Any]:
        """
        Create action

        Args:
            pack_ref: Pack reference (namespace.pack_name)
            name: Action name (legacy, maps to label)
            runner_type: Runner type (legacy, maps to runtime_ref)
            entrypoint: Entry point (e.g., actions/echo.py)
            param_schema: JSON Schema for parameters
            ref: Unique reference identifier (preferred)
            label: Human-readable label
            description: Action description
            runtime: Runtime ID (preferred over runtime_ref)
            runtime_ref: Runtime reference (e.g., "python3")
            out_schema: Output schema (JSON Schema)
            **kwargs: Additional action fields

        Returns:
            Created action data
        """
        # Handle legacy parameters
        if not ref and name:
            # Generate ref from pack_ref and name
            if pack_ref and "." not in name:
                ref = f"{pack_ref}.{name}"
            else:
                ref = name

        if not label:
            label = name or ref or "Unnamed Action"

        if not description:
            description = label

        # Convert runner_type to runtime ID if needed
        if not runtime and not runtime_ref:
            runtime_ref = runner_type or "python3"

        if not runtime and runtime_ref:
            # Try to look up runtime by reference
            # If endpoint doesn't exist, runtime field is optional, so we can skip it
            try:
                runtimes = self.list_runtimes()
                for rt in runtimes:
                    if rt.get("ref") == runtime_ref:
                        runtime = rt["id"]
                        break

                # If not found, try common mappings
                if not runtime:
                    if runtime_ref in ["python3", "python"]:
                        runtime_ref = "core.action.python3"
                    elif runtime_ref in ["shell", "bash"]:
                        runtime_ref = "core.action.shell"
                    elif runtime_ref == "http":
                        runtime_ref = "core.action.http"

                    # Try lookup again with mapped ref
                    for rt in runtimes:
                        if rt.get("ref") == runtime_ref:
                            runtime = rt["id"]
                            break
            except Exception as e:
                # Runtime endpoint doesn't exist or failed - runtime is optional, so continue without it
                pass

        payload = {
            "ref": ref,
            "pack_ref": pack_ref,
            "label": label,
            "description": description,
            "entrypoint": entrypoint or f"actions/{name or 'action'}.py",
        }

        if runtime:
            payload["runtime"] = runtime
        if param_schema:
            payload["param_schema"] = param_schema
        if out_schema:
            payload["out_schema"] = out_schema

        # Merge any additional kwargs
        payload.update(kwargs)

        response = self.post("/api/v1/actions", json=payload)
        return response["data"]

    def delete_action(self, action_id: int) -> Dict[str, Any]:
        """Delete action"""
        return self.delete(f"/api/v1/actions/{action_id}")

    # ========================================================================
    # Trigger Management
    # ========================================================================

    def list_triggers(self) -> List[Dict[str, Any]]:
        """List all triggers"""
        response = self.get("/api/v1/triggers")
        return response["data"]

    def get_trigger(self, trigger_id: int) -> Dict[str, Any]:
        """Get trigger by ID"""
        response = self.get(f"/api/v1/triggers/{trigger_id}")
        return response["data"]

    def create_trigger(
        self,
        pack_ref: str = None,
        name: str = None,
        trigger_type: str = None,
        ref: str = None,
        label: str = None,
        description: str = None,
        param_schema: Optional[Dict[str, Any]] = None,
        out_schema: Optional[Dict[str, Any]] = None,
        enabled: bool = True,
        parameters: Optional[Dict[str, Any]] = None,
        **kwargs,
    ) -> Dict[str, Any]:
        """
        Create trigger

        Args:
            pack_ref: Pack reference (optional)
            name: Trigger name (legacy, maps to label)
            trigger_type: Type (legacy, not used in API)
            ref: Unique reference identifier (e.g., "core.webhook")
            label: Human-readable label
            description: Trigger description
            param_schema: Parameter schema (JSON Schema)
            out_schema: Output schema (JSON Schema)
            enabled: Whether the trigger is enabled
            parameters: Trigger-specific parameters (legacy, not used)
            **kwargs: Additional trigger fields

        Returns:
            Created trigger data
        """
        # Handle legacy name/trigger_type parameters
        if not ref and name:
            # Generate ref from pack_ref and name
            if pack_ref and "." not in name:
                ref = f"{pack_ref}.{name}"
            else:
                ref = name

        if not label:
            label = name or ref

        payload = {
            "ref": ref,
            "label": label,
            "enabled": enabled,
        }

        if pack_ref:
            payload["pack_ref"] = pack_ref
        if description:
            payload["description"] = description
        if param_schema:
            payload["param_schema"] = param_schema
        if out_schema:
            payload["out_schema"] = out_schema

        # Merge any additional kwargs
        payload.update(kwargs)

        response = self.post("/api/v1/triggers", json=payload)
        return response["data"]

    def delete_trigger(self, trigger_id: int) -> Dict[str, Any]:
        """Delete trigger"""
        return self.delete(f"/api/v1/triggers/{trigger_id}")

    def fire_webhook(self, trigger_id: int, payload: Dict[str, Any]) -> Dict[str, Any]:
        """
        Fire webhook trigger

        Args:
            trigger_id: Webhook trigger ID
            payload: Webhook payload data

        Returns:
            Created event data
        """
        response = self.post(f"/api/v1/webhooks/{trigger_id}", json=payload)
        return response["data"]

    # ========================================================================
    # Sensor Management
    # ========================================================================

    def list_sensors(self) -> List[Dict[str, Any]]:
        """List all sensors"""
        response = self.get("/api/v1/sensors")
        return response["data"]

    def get_sensor(self, sensor_id: int) -> Dict[str, Any]:
        """Get sensor by ID"""
        response = self.get(f"/api/v1/sensors/{sensor_id}")
        return response["data"]

    def create_sensor(
        self,
        ref: str = None,
        trigger_types: List[str] = None,
        label: str = None,
        description: str = "",
        entrypoint: str = "internal://timer",
        runtime_ref: str = "python3",
        pack_ref: str = None,
        enabled: bool = True,
        config: Optional[Dict[str, Any]] = None,
        **kwargs,
    ) -> Dict[str, Any]:
        """
        Create sensor (using direct SQL until API endpoint exists)

        Args:
            ref: Unique reference (e.g., "pack.sensor_name")
            trigger_types: List of trigger refs this sensor emits events for
            label: Human-readable label
            description: Sensor description
            entrypoint: Entry point (default: internal://timer for timers)
            runtime_ref: Runtime reference (default: python3)
            pack_ref: Pack reference
            enabled: Whether sensor is enabled
            config: Sensor configuration (e.g., {"interval": 5, "unit": "seconds"})

        Returns:
            Created sensor data
        """
        # Get database connection from environment
        db_url = os.environ.get(
            "DATABASE_URL", "postgresql://postgres:postgres@localhost:5432/attune"
        )

        conn = psycopg.connect(db_url)
        cur = conn.cursor()

        try:
            # Map short runtime names to full refs (core.action.*)
            runtime_ref_map = {
                "python3": "core.action.python3",
                "nodejs": "core.action.nodejs",
                "shell": "core.action.shell",
            }
            full_runtime_ref = runtime_ref_map.get(runtime_ref, runtime_ref)

            # Get runtime ID
            cur.execute(
                "SELECT id FROM attune.runtime WHERE ref = %s", (full_runtime_ref,)
            )
            runtime_row = cur.fetchone()
            if not runtime_row:
                raise ValueError(f"Runtime not found: {full_runtime_ref}")
            runtime_id = runtime_row[0]

            # Get pack ID if pack_ref is provided
            pack_id = None
            if pack_ref:
                cur.execute("SELECT id FROM attune.pack WHERE ref = %s", (pack_ref,))
                pack_row = cur.fetchone()
                if pack_row:
                    pack_id = pack_row[0]

            # Convert config to JSON
            import json

            config_json = json.dumps(config) if config else None

            # Insert sensor (without trigger — relationship is trigger→sensor now)
            cur.execute(
                """
                INSERT INTO attune.sensor
                (ref, pack, pack_ref, label, description, entrypoint, runtime, runtime_ref,
                 enabled, config)
                VALUES (%s, %s, %s, %s, %s, %s, %s, %s, %s, %s::jsonb)
                RETURNING id, ref, pack, pack_ref, label, description, entrypoint,
                          runtime, runtime_ref, enabled,
                          config, created, updated
            """,
                (
                    ref,
                    pack_id,
                    pack_ref,
                    label,
                    description,
                    entrypoint,
                    runtime_id,
                    full_runtime_ref,
                    enabled,
                    config_json,
                ),
            )

            row = cur.fetchone()

            # Convert to dict
            sensor = {
                "id": row[0],
                "ref": row[1],
                "pack": row[2],
                "pack_ref": row[3],
                "label": row[4],
                "description": row[5],
                "entrypoint": row[6],
                "runtime": row[7],
                "runtime_ref": row[8],
                "enabled": row[9],
                "config": row[10],
                "created": row[11].isoformat() if row[11] else None,
                "updated": row[12].isoformat() if row[12] else None,
            }

            # Link triggers to this sensor
            if trigger_types:
                for tref in trigger_types:
                    cur.execute(
                        "UPDATE attune.trigger SET sensor = %s, sensor_ref = %s WHERE ref = %s",
                        (sensor["id"], ref, tref),
                    )

            conn.commit()

            return sensor

        finally:
            cur.close()
            conn.close()

    def delete_sensor(self, sensor_id: int) -> Dict[str, Any]:
        """Delete sensor"""
        return self.delete(f"/api/v1/sensors/{sensor_id}")

    # ========================================================================
    # Rule Management
    # ========================================================================

    def list_rules(self) -> List[Dict[str, Any]]:
        """List all rules"""
        response = self.get("/api/v1/rules")
        return response["data"]

    def get_rule(self, rule_id: int) -> Dict[str, Any]:
        """Get rule by ID"""
        response = self.get(f"/api/v1/rules/{rule_id}")
        return response["data"]

    def create_rule(
        self,
        name: str = None,
        pack_ref: str = None,
        trigger_id: int = None,
        action_ref: str = None,
        enabled: bool = True,
        criteria: Optional[str] = None,
        action_parameters: Optional[Dict[str, Any]] = None,
        ref: str = None,
        label: str = None,
        description: str = None,
        trigger_ref: str = None,
        conditions: Optional[Dict[str, Any]] = None,
        action_params: Optional[Dict[str, Any]] = None,
        trigger_params: Optional[Dict[str, Any]] = None,
        **kwargs,
    ) -> Dict[str, Any]:
        """
        Create rule

        Args:
            name: Rule name (legacy, maps to label)
            pack_ref: Pack reference
            trigger_id: Trigger ID (legacy, converted to trigger_ref)
            action_ref: Action reference to execute
            enabled: Whether rule is enabled
            criteria: Optional Jinja2 criteria expression (legacy, maps to conditions)
            action_parameters: Parameters to pass to action (legacy, maps to action_params)
            ref: Unique reference identifier (e.g., "mypack.notify_on_error")
            label: Human-readable label
            description: Rule description
            trigger_ref: Trigger reference (preferred over trigger_id)
            conditions: Conditions for rule evaluation (JSON Logic)
            action_params: Parameters to pass to action
            trigger_params: Parameters for trigger configuration
            **kwargs: Additional rule fields

        Returns:
            Created rule data
        """
        # Handle legacy parameters
        if not ref and name:
            # Generate ref from pack_ref and name
            if pack_ref and "." not in name:
                ref = f"{pack_ref}.{name}"
            else:
                ref = name

        if not label:
            label = name or ref or "Unnamed Rule"

        if not description:
            description = label

        # Convert trigger_id to trigger_ref if needed
        if not trigger_ref and trigger_id:
            trigger = self.get_trigger(trigger_id)
            trigger_ref = trigger["ref"]

        if not conditions and criteria:
            conditions = {"expression": criteria}

        if not action_params:
            action_params = action_parameters or {}

        if not trigger_params:
            trigger_params = {}

        payload = {
            "ref": ref,
            "pack_ref": pack_ref,
            "label": label,
            "description": description,
            "action_ref": action_ref,
            "trigger_ref": trigger_ref,
            "conditions": conditions or {},
            "action_params": action_params,
            "trigger_params": trigger_params,
            "enabled": enabled,
        }

        # Merge any additional kwargs
        payload.update(kwargs)

        response = self.post("/api/v1/rules", json=payload)
        return response["data"]

    def update_rule(self, rule_id: int, **kwargs) -> Dict[str, Any]:
        """Update rule"""
        response = self.patch(f"/api/v1/rules/{rule_id}", json=kwargs)
        return response["data"]

    def enable_rule(self, rule_id: int) -> Dict[str, Any]:
        """Enable rule"""
        return self.update_rule(rule_id, enabled=True)

    def disable_rule(self, rule_id: int) -> Dict[str, Any]:
        """Disable rule"""
        return self.update_rule(rule_id, enabled=False)

    def delete_rule(self, rule_id: int) -> Dict[str, Any]:
        """Delete rule"""
        return self.delete(f"/api/v1/rules/{rule_id}")

    # ========================================================================
    # Event Management
    # ========================================================================

    def list_events(
        self, trigger_id: Optional[int] = None, limit: int = 100, offset: int = 0
    ) -> List[Dict[str, Any]]:
        """
        List events

        Args:
            trigger_id: Optional filter by trigger ID
            limit: Maximum number of events to return
            offset: Pagination offset

        Returns:
            List of events
        """
        params = {"limit": limit, "offset": offset}
        if trigger_id:
            params["trigger_id"] = trigger_id

        response = self.get("/api/v1/events", params=params)
        return response["data"]

    def get_event(self, event_id: int) -> Dict[str, Any]:
        """Get event by ID"""
        response = self.get(f"/api/v1/events/{event_id}")
        return response["data"]

    # ========================================================================
    # Enforcement Management
    # ========================================================================

    def list_enforcements(
        self, rule_id: Optional[int] = None, limit: int = 100, offset: int = 0
    ) -> List[Dict[str, Any]]:
        """List enforcements"""
        params = {"limit": limit, "offset": offset}
        if rule_id:
            params["rule_id"] = rule_id

        response = self.get("/api/v1/enforcements", params=params)
        return response["data"]

    def get_enforcement(self, enforcement_id: int) -> Dict[str, Any]:
        """Get enforcement by ID"""
        response = self.get(f"/api/v1/enforcements/{enforcement_id}")
        return response["data"]

    # ========================================================================
    # Execution Management
    # ========================================================================

    def list_executions(
        self,
        action_ref: Optional[str] = None,
        status: Optional[str] = None,
        enforcement_id: Optional[int] = None,
        limit: int = 100,
        offset: int = 0,
    ) -> List[Dict[str, Any]]:
        """
        List executions

        Args:
            action_ref: Optional filter by action reference
            status: Optional filter by status
            enforcement_id: Optional filter by enforcement ID
            limit: Maximum number of executions
            offset: Pagination offset

        Returns:
            List of executions
        """
        params = {"limit": limit, "offset": offset}
        if action_ref:
            params["action_ref"] = action_ref
        if status:
            params["status"] = status
        if enforcement_id:
            params["enforcement"] = enforcement_id

        response = self.get("/api/v1/executions", params=params)
        return response["data"]

    def get_execution(self, execution_id: int) -> Dict[str, Any]:
        """Get execution by ID"""
        response = self.get(f"/api/v1/executions/{execution_id}")
        return response["data"]

    def cancel_execution(self, execution_id: int) -> Dict[str, Any]:
        """Cancel running execution"""
        response = self.post(f"/api/v1/executions/{execution_id}/cancel")
        return response["data"]

    # ========================================================================
    # Inquiry Management
    # ========================================================================

    def list_inquiries(
        self, status: Optional[str] = None, limit: int = 100, offset: int = 0
    ) -> List[Dict[str, Any]]:
        """List inquiries"""
        params = {"limit": limit, "offset": offset}
        if status:
            params["status"] = status

        response = self.get("/api/v1/inquiries", params=params)
        return response["data"]

    def get_inquiry(self, inquiry_id: int) -> Dict[str, Any]:
        """Get inquiry by ID"""
        response = self.get(f"/api/v1/inquiries/{inquiry_id}")
        return response["data"]

    def respond_to_inquiry(
        self, inquiry_id: int, response_data: Dict[str, Any]
    ) -> Dict[str, Any]:
        """
        Respond to inquiry

        Args:
            inquiry_id: Inquiry ID
            response_data: Response data (structure depends on inquiry type)

        Returns:
            Updated inquiry data
        """
        response = self.post(
            f"/api/v1/inquiries/{inquiry_id}/respond", json=response_data
        )
        return response["data"]

    # ========================================================================
    # Datastore (Key-Value Store)
    # ========================================================================

    def datastore_get(self, key: str) -> Optional[Any]:
        """
        Get value from datastore

        Args:
            key: Datastore key

        Returns:
            Value or None if not found
        """
        try:
            response = self.get(f"/api/v1/datastore/{key}")
            return response["data"]["value"]
        except requests.exceptions.HTTPError as e:
            if e.response.status_code == 404:
                return None
            raise

    def datastore_set(
        self, key: str, value: Any, encrypted: bool = False, ttl: Optional[int] = None
    ) -> Dict[str, Any]:
        """
        Set value in datastore

        Args:
            key: Datastore key
            value: Value to store
            encrypted: Whether to encrypt value
            ttl: Time-to-live in seconds

        Returns:
            Created datastore item
        """
        payload = {
            "key": key,
            "value": value,
            "encrypted": encrypted,
        }
        if ttl:
            payload["ttl"] = ttl

        response = self.post("/api/v1/datastore", json=payload)
        return response["data"]

    def datastore_delete(self, key: str) -> Dict[str, Any]:
        """Delete key from datastore"""
        return self.delete(f"/api/v1/datastore/{key}")

    # ========================================================================
    # Secrets Management
    # ========================================================================

    def list_secrets(self) -> List[Dict[str, Any]]:
        """List all secrets (values are encrypted)"""
        response = self.get("/api/v1/secrets")
        return response["data"]

    def get_secret(self, key: str) -> Optional[str]:
        """
        Get secret value

        Args:
            key: Secret key

        Returns:
            Decrypted secret value or None if not found
        """
        try:
            response = self.get(f"/api/v1/secrets/{key}")
            return response["data"]["value"]
        except requests.exceptions.HTTPError as e:
            if e.response.status_code == 404:
                return None
            raise

    def create_secret(
        self,
        key: str = None,
        value: str = None,
        name: str = None,
        encrypted: bool = True,
        owner_type: str = "system",
        owner_identity_login: str = None,
        owner_pack_ref: str = None,
        owner_action_ref: str = None,
        owner_sensor_ref: str = None,
        **kwargs,
    ) -> Dict[str, Any]:
        """
        Create secret (stored as a key in the API)

        Args:
            key: Local secret reference (e.g., "github_token")
            value: Secret value (will be encrypted if encrypted=True)
            name: Human-readable name for the key
            encrypted: Whether to encrypt the value (default: True)
            owner_type: Type of owner (system, identity, pack, action, sensor)
            owner_identity_login: Optional owner identity login
            owner_pack_ref: Optional owner pack reference
            owner_action_ref: Optional owner action reference
            owner_sensor_ref: Optional owner sensor reference

        Returns:
            Created secret metadata
        """
        # Handle legacy kwargs for backwards compatibility
        if "description" in kwargs:
            # Ignore description as it's not in the actual API
            pass

        payload = {
            "local_ref": key.rsplit(".", 1)[-1],
            "name": name or key,
            "value": value,
            "encrypted": encrypted,
            "owner_type": owner_type,
        }

        # Add optional owner fields
        if owner_identity_login:
            payload["owner_identity_login"] = owner_identity_login
        if owner_pack_ref:
            payload["owner_pack_ref"] = owner_pack_ref
        if owner_action_ref:
            payload["owner_action_ref"] = owner_action_ref
        if owner_sensor_ref:
            payload["owner_sensor_ref"] = owner_sensor_ref

        response = self.post("/api/v1/keys", json=payload)
        return response["data"]

    def update_secret(self, key: str, value: str) -> Dict[str, Any]:
        """Update secret value"""
        response = self.put(f"/api/v1/secrets/{key}", json={"value": value})
        return response["data"]

    def delete_secret(self, key: str) -> Dict[str, Any]:
        """Delete secret"""
        return self.delete(f"/api/v1/secrets/{key}")
