"""
Wrapper for Generated API Client

This module provides test-oriented helpers around the auto-generated OpenAPI
client while keeping requests aligned with the current API contract.
"""

import json
import os
import re
import subprocess
import tempfile
from typing import Any, Optional
from urllib.parse import quote

import requests

from generated_client import AuthenticatedClient, Client
from generated_client.api.actions import (
    create_action as gen_create_action,
)
from generated_client.api.actions import (
    delete_action as gen_delete_action,
)
from generated_client.api.actions import (
    get_action as gen_get_action,
)
from generated_client.api.actions import (
    list_actions as gen_list_actions,
)
from generated_client.api.auth import (
    login as gen_login,
)
from generated_client.api.auth import (
    register as gen_register,
)
from generated_client.api.caches import (
    abandon_generation as gen_cache_abandon_generation,
    create_generation as gen_cache_create_generation,
    create_namespace as gen_cache_create_namespace,
    delete_namespace as gen_cache_delete_namespace,
    list_generations as gen_cache_list_generations,
    list_namespaces as gen_cache_list_namespaces,
    lookup_entries as gen_cache_lookup_entries,
    lookup_entry as gen_cache_lookup_entry,
    promote_generation as gen_cache_promote_generation,
    scan_entries as gen_cache_scan_entries,
    seal_generation as gen_cache_seal_generation,
    show_generation as gen_cache_show_generation,
    show_namespace as gen_cache_show_namespace,
    update_namespace as gen_cache_update_namespace,
    upload_chunk as gen_cache_upload_chunk,
)
from generated_client.api.enforcements import list_enforcements as gen_list_enforcements
from generated_client.api.events import get_event as gen_get_event
from generated_client.api.events import list_events as gen_list_events
from generated_client.api.executions import (
    get_execution as gen_get_execution,
)
from generated_client.api.executions import (
    list_executions as gen_list_executions,
)
from generated_client.api.health import health as gen_health
from generated_client.api.inquiries import (
    list_inquiries as gen_list_inquiries,
)
from generated_client.api.inquiries import (
    respond_to_inquiry as gen_respond_inquiry,
)
from generated_client.api.packs import (
    create_pack as gen_create_pack,
)
from generated_client.api.packs import (
    delete_pack as gen_delete_pack,
)
from generated_client.api.packs import (
    get_pack as gen_get_pack,
)
from generated_client.api.packs import (
    list_packs as gen_list_packs,
)
from generated_client.api.packs import (
    register_pack as gen_register_pack,
)
from generated_client.api.rules import (
    create_rule as gen_create_rule,
)
from generated_client.api.rules import (
    delete_rule as gen_delete_rule,
)
from generated_client.api.rules import (
    disable_rule as gen_disable_rule,
)
from generated_client.api.rules import (
    enable_rule as gen_enable_rule,
)
from generated_client.api.rules import (
    get_rule as gen_get_rule,
)
from generated_client.api.rules import (
    list_rules as gen_list_rules,
)
from generated_client.api.secrets import (
    create_key as gen_create_key,
)
from generated_client.api.secrets import (
    delete_key as gen_delete_key,
)
from generated_client.api.secrets import (
    get_key as gen_get_key,
)
from generated_client.api.secrets import (
    list_keys as gen_list_keys,
)
from generated_client.api.secrets import (
    update_key as gen_update_key,
)
from generated_client.api.sensors import (
    create_sensor as gen_create_sensor,
)
from generated_client.api.sensors import (
    delete_sensor as gen_delete_sensor,
)
from generated_client.api.sensors import (
    get_sensor as gen_get_sensor,
)
from generated_client.api.sensors import (
    list_sensors as gen_list_sensors,
)
from generated_client.api.triggers import (
    create_trigger as gen_create_trigger,
)
from generated_client.api.triggers import (
    delete_trigger as gen_delete_trigger,
)
from generated_client.api.triggers import (
    get_trigger as gen_get_trigger,
)
from generated_client.api.triggers import (
    list_triggers as gen_list_triggers,
)
from generated_client.api.webhooks import receive_webhook as gen_receive_webhook
from generated_client.models.create_action_request import CreateActionRequest
from generated_client.models.cache_multi_lookup_request import CacheMultiLookupRequest
from generated_client.models.cache_owner_body import CacheOwnerBody
from generated_client.models.cache_point_lookup_request import CachePointLookupRequest
from generated_client.models.create_cache_generation_request import (
    CreateCacheGenerationRequest,
)
from generated_client.models.create_cache_namespace_request import (
    CreateCacheNamespaceRequest,
)
from generated_client.models.create_key_request import CreateKeyRequest
from generated_client.models.create_pack_request import CreatePackRequest
from generated_client.models.create_rule_request import CreateRuleRequest
from generated_client.models.create_sensor_request import CreateSensorRequest
from generated_client.models.create_trigger_request import CreateTriggerRequest
from generated_client.models.inquiry_respond_request import InquiryRespondRequest
from generated_client.models.login_request import LoginRequest
from generated_client.models.register_pack_request import RegisterPackRequest
from generated_client.models.register_request import RegisterRequest
from generated_client.models.update_key_request import UpdateKeyRequest
from generated_client.models.owner_type import OwnerType
from generated_client.models.promote_cache_generation_request import (
    PromoteCacheGenerationRequest,
)
from generated_client.models.seal_cache_generation_request import (
    SealCacheGenerationRequest,
)
from generated_client.models.upload_cache_chunk_request import UploadCacheChunkRequest
from generated_client.models.update_cache_namespace_request import UpdateCacheNamespaceRequest
from generated_client.types import UNSET


class CacheApiError(RuntimeError):
    """Public cache API failure, retaining the HTTP response for E2E assertions."""

    def __init__(self, response: Any):
        self.response = response
        try:
            if hasattr(response, "json"):
                body = response.json()
            else:
                body = json.loads(response.content)
        except Exception:
            body = getattr(response, "text", None)
            if body is None:
                content = getattr(response, "content", b"")
                body = content.decode(errors="replace") if isinstance(content, bytes) else content
        self.body = body
        super().__init__(f"Cache API request failed: {int(response.status_code)} {body}")


def to_dict(obj: Any) -> Any:
    """Convert Pydantic model to dict recursively"""
    if obj is None:
        return None
    if hasattr(obj, "to_dict"):
        return obj.to_dict()
    if isinstance(obj, dict):
        return obj
    if isinstance(obj, list):
        return [to_dict(item) for item in obj]
    return obj


def unwrap_list(response: Any) -> list[dict]:
    """Unwrap a paginated list response into a list of dicts.

    The generated client returns PaginatedResponse* objects with 'items' field.
    """
    if response is None:
        return []
    result = to_dict(response)
    if isinstance(result, dict):
        if "items" in result:
            return result["items"]
        if "data" in result:
            return result["data"]
    if isinstance(result, list):
        return result
    return []


def unwrap_item(response: Any) -> Optional[dict]:
    """Unwrap a single-item API response into a dict.

    The generated client returns ApiResponse* objects with 'data' field.
    """
    if response is None:
        return None
    result = to_dict(response)
    if isinstance(result, dict):
        if "data" in result:
            return result["data"]
        return result
    return result


def qualify_ref(pack_ref: Optional[str], ref: Optional[str]) -> Optional[str]:
    if ref and pack_ref and "." not in ref:
        return f"{pack_ref}.{ref}"
    return ref


def safe_ref_part(value: Optional[str], fallback: str = "item") -> str:
    if not value:
        return fallback
    slug = re.sub(r"[^a-zA-Z0-9_]+", "_", value.strip()).strip("_").lower()
    return slug or fallback


def _json_or_empty(response: Any) -> Any:
    if not getattr(response, "text", ""):
        return {}
    return response.json()


class TestResponse:
    """Response facade that exposes parsed API JSON consistently in tests."""

    def __init__(self, response: Any):
        self._response = response

    def __getattr__(self, name: str) -> Any:
        return getattr(self._response, name)

    def json(self) -> Any:
        if not getattr(self._response, "text", ""):
            return {}
        body = self._response.json()
        if isinstance(body, dict):
            body = dict(body)
            if "data" not in body and "items" in body:
                body["data"] = body["items"]
            data = body.get("data")
            if isinstance(data, dict) and "key" not in data and "ref" in data:
                data = dict(data)
                data["key"] = data["ref"]
                if data.get("webhook_enabled") is False and "webhook_key" not in data:
                    data["webhook_key"] = None
                body["data"] = data
            elif isinstance(data, dict) and data.get("webhook_enabled") is False and "webhook_key" not in data:
                data = dict(data)
                data["webhook_key"] = None
                body["data"] = data
            if isinstance(data, dict):
                data = self._normalize_item(data)
                body["data"] = data
                result = data.get("result")
                if isinstance(result, dict) and isinstance(result.get("data"), dict):
                    normalized_result = dict(result)
                    for key, value in result["data"].items():
                        normalized_result.setdefault(key, value)
                    data = dict(data)
                    data["result"] = normalized_result
                    body["data"] = data
            elif isinstance(data, list):
                body["data"] = [
                    self._normalize_item(item)
                    for item in data
                ]
        return body

    def __getitem__(self, key: str) -> Any:
        return self.json()[key]

    def _normalize_item(self, item: Any) -> Any:
        if not isinstance(item, dict):
            return item
        normalized = dict(item)
        if "key" not in normalized and "ref" in normalized:
            normalized["key"] = normalized["ref"]
        if "rule_id" not in normalized and "rule" in normalized:
            normalized["rule_id"] = normalized["rule"]
        if "event_id" not in normalized and "event" in normalized:
            normalized["event_id"] = normalized["event"]
        if "updated" not in normalized and ("resolved_at" in normalized or "event" in normalized or "rule" in normalized):
            normalized["updated"] = normalized.get("resolved_at") or normalized.get("created")
        return normalized


class TestSession:
    """requests-like session backed by the same auth headers as AttuneClient."""

    def __init__(self, owner: "AttuneClient"):
        self._owner = owner
        self._session = requests.Session()

    @property
    def headers(self):
        return self._session.headers

    def request(self, method: str, url: str, **kwargs) -> TestResponse:
        if url.startswith("/"):
            url = f"{self._owner.base_url}{url}"
        response = self._session.request(method, url, **kwargs)
        return TestResponse(response)

    def get(self, url: str, **kwargs) -> TestResponse:
        return self.request("GET", url, **kwargs)

    def post(self, url: str, **kwargs) -> TestResponse:
        return self.request("POST", url, **kwargs)

    def put(self, url: str, **kwargs) -> TestResponse:
        return self.request("PUT", url, **kwargs)

    def patch(self, url: str, **kwargs) -> TestResponse:
        return self.request("PATCH", url, **kwargs)

    def delete(self, url: str, **kwargs) -> TestResponse:
        return self.request("DELETE", url, **kwargs)


class AttuneClient:
    """
    Test wrapper for the generated Attune API client.
    """

    _auth_cache: dict[tuple[str, str], dict] = {}

    def __init__(
        self,
        base_url: Optional[str] = None,
        timeout: int = 30,
        auto_login: bool = True,
    ):
        """
        Initialize Attune client wrapper

        Args:
            base_url: API base URL (defaults to ATTUNE_API_URL env var)
            timeout: Request timeout in seconds
            auto_login: Whether to auto-login with default credentials
        """
        self.base_url = (base_url or os.getenv("ATTUNE_API_URL", "http://localhost:8080")).rstrip("/")
        self.timeout = timeout
        self.auto_login_flag = auto_login
        self.session = TestSession(self)

        # Initialize unauthenticated clients
        # Note: Generated API functions include full paths like "/api/v1/packs"
        # so base_url should just be the host
        self.client = Client(
            base_url=self.base_url,
            timeout=float(timeout),
            verify_ssl=False,
        )

        # Auth client (same as regular client since paths are in generated code)
        self.auth_base_client = Client(
            base_url=self.base_url,
            timeout=float(timeout),
            verify_ssl=False,
        )

        # Will be set after login
        self.auth_client: Optional[AuthenticatedClient] = None
        self.access_token: Optional[str] = None
        self.refresh_token: Optional[str] = None
        self.user_info: Optional[dict] = None
        self._created_rule_refs: list[str] = []
        self._created_trigger_refs: list[str] = []
        self._created_action_refs: list[str] = []
        self._created_pack_refs: list[str] = []

        # Default credentials
        self.default_login = os.getenv("TEST_USER_LOGIN", "test@attune.local")
        self.default_password = os.getenv("TEST_USER_PASSWORD", "TestPass123!")

        # Auto-login if requested
        if self.auto_login_flag:
            self.login()

    def _get_client(self) -> AuthenticatedClient:
        """Get authenticated client (raises if not logged in)"""
        if not self.auth_client:
            raise Exception("Not authenticated. Please login first.")
        return self.auth_client

    def register(
        self,
        login: str,
        password: str,
        display_name: Optional[str] = None,
    ) -> dict:
        """
        Register a new user

        Args:
            login: User login/email
            password: User password
            display_name: User display name

        Returns:
            dict: User information
        """
        request = RegisterRequest(
            login=login,
            password=password,
            display_name=display_name or login,
        )

        response = gen_register.sync(client=self.auth_base_client, body=request)

        return unwrap_item(response)

        raise Exception("Registration failed")

    def login(
        self,
        login: Optional[str] = None,
        password: Optional[str] = None,
    ) -> dict:
        """
        Login and obtain access token

        Args:
            login: User login (defaults to TEST_USER_LOGIN)
            password: User password (defaults to TEST_USER_PASSWORD)

        Returns:
            dict: Login response with tokens
        """
        login_email = login or self.default_login
        login_password = password or self.default_password

        cache_enabled = os.getenv("ATTUNE_E2E_REUSE_AUTH", "true").lower() not in {
            "0",
            "false",
            "no",
        }
        cache_key = (self.base_url, login_email)
        if cache_enabled and cache_key in self._auth_cache:
            data = dict(self._auth_cache[cache_key])
            self._apply_auth_data(data)
            return data

        request = LoginRequest(
            login=login_email,
            password=login_password,
        )

        response = gen_login.sync(client=self.auth_base_client, body=request)

        if response:
            result = to_dict(response)
            if isinstance(result, dict) and "data" in result:
                data = result["data"]
                if cache_enabled:
                    self._auth_cache[cache_key] = dict(data)
                self._apply_auth_data(data)
                return data

        raise Exception("Login failed")

    def _apply_auth_data(self, data: dict) -> None:
        self.access_token = data.get("access_token")
        self.refresh_token = data.get("refresh_token")
        self.user_info = data.get("user")

        # Note: base_url should just be host since generated API includes full paths.
        self.auth_client = AuthenticatedClient(
            base_url=self.base_url,
            token=self.access_token,
            timeout=float(self.timeout),
            verify_ssl=False,
        )
        self.session.headers.update({"Authorization": f"Bearer {self.access_token}"})

    def logout(self):
        """Logout and clear tokens"""
        self.cleanup_created_resources()
        self.auth_client = None
        self.access_token = None
        self.refresh_token = None
        self.user_info = None
        self.session.headers.pop("Authorization", None)

    def cleanup_created_resources(self) -> None:
        """Best-effort teardown of resources created by this test client."""
        if not self.auth_client:
            self._clear_created_resources()
            return

        for rule_ref in reversed(self._created_rule_refs):
            try:
                gen_delete_rule.sync(ref=rule_ref, client=self._get_client())
            except Exception:
                pass

        for trigger_ref in reversed(self._created_trigger_refs):
            try:
                gen_delete_trigger.sync(ref=trigger_ref, client=self._get_client())
            except Exception:
                pass

        for action_ref in reversed(self._created_action_refs):
            try:
                gen_delete_action.sync(ref=action_ref, client=self._get_client())
            except Exception:
                pass

        for pack_ref in reversed(self._created_pack_refs):
            if pack_ref == "core" or pack_ref.startswith("test_pack"):
                continue
            try:
                gen_delete_pack.sync(ref=pack_ref, client=self._get_client())
            except Exception:
                pass

        self._clear_created_resources()

    def _clear_created_resources(self) -> None:
        self._created_rule_refs.clear()
        self._created_trigger_refs.clear()
        self._created_action_refs.clear()
        self._created_pack_refs.clear()

    def health(self) -> dict:
        """Check API health"""
        response = gen_health.sync(client=self.auth_base_client)
        return to_dict(response) if response else {}

    # ========================================================================
    # Packs
    # ========================================================================

    def list_packs(self, **params) -> list[dict]:
        """List all packs"""
        requested_page = params.get("page")
        per_page = min(int(params.get("limit") or params.get("per_page") or 100), 100)
        items = []
        page = int(requested_page or 1)

        while True:
            response = self._request(
                "GET",
                "/api/v1/packs",
                params={"page": page, "per_page": per_page, "page_size": per_page},
            )
            if response.status_code != 200:
                return items

            data = response.json()
            page_items = data.get("items") or data.get("data") or []
            if not isinstance(page_items, list):
                return items

            items.extend(page_items)
            if requested_page:
                break

            pagination = (
                data.get("pagination") if isinstance(data.get("pagination"), dict) else {}
            )
            total_pages = data.get("total_pages") or pagination.get("total_pages")
            if total_pages is not None:
                if page >= int(total_pages):
                    break
            elif len(page_items) < per_page:
                break

            page += 1

        return items

    def get_pack(self, pack_id: int) -> dict:
        """Get pack by ID - Note: API uses ref, so need to lookup by ref"""
        # The generated API uses ref, so ID lookups scan the current list first.
        packs = self.list_packs()
        for pack in packs:
            if pack.get("id") == pack_id:
                return self.get_pack_by_ref(pack["ref"])
        raise Exception(f"Pack {pack_id} not found")

    def get_pack_by_ref(self, ref: str) -> Optional[dict]:
        """Get pack by reference"""
        response = gen_get_pack.sync(ref=ref, client=self._get_client())
        return unwrap_item(response)

    def create_pack(
        self,
        ref: str | dict,
        label: Optional[str] = None,
        description: Optional[str] = None,
        version: str = "1.0.0",
        author: Optional[str] = None,
        **kwargs,
    ) -> dict:
        """Create a new pack"""
        if isinstance(ref, dict):
            data = ref
            ref = data.get("ref")
            label = label or data.get("label") or data.get("name")
            description = description or data.get("description")
            version = data.get("version", version)
            author = author or data.get("author")
            kwargs = {**data, **kwargs}

        if not ref or not label:
            raise ValueError("Missing required arguments: ref and label")

        payload = {
            "ref": ref,
            "label": label,
            "version": version,
        }
        if description is not None:
            payload["description"] = description

        for key in (
            "conf_schema",
            "config",
            "dependencies",
            "runtime_deps",
            "tags",
            "is_standard",
        ):
            if key in kwargs and kwargs[key] is not None:
                payload[key] = kwargs[key]

        meta = dict(kwargs.get("meta") or {})
        if author:
            meta.setdefault("author", author)
        if meta:
            payload["meta"] = meta

        response = self._request("POST", "/api/v1/packs", json=payload)
        if response.status_code in (200, 201):
            data = response.json()
            if "data" in data:
                pack = data["data"]
            else:
                pack = data
            if isinstance(pack, dict) and pack.get("ref"):
                self._created_pack_refs.append(pack["ref"])
            return pack
        raise Exception(f"Failed to create pack: {response.status_code} {response.text}")

    def register_pack(
        self, path: str, skip_tests: bool = True, force: bool = False
    ) -> dict:
        """Register a pack from filesystem path (API-server-side path)

        Args:
            path: Path to pack directory (must be accessible by the API server)
            skip_tests: Skip running pack tests during registration (default: True)
            force: Force registration even if tests fail (default: False)
        """
        # Use direct HTTP request to avoid generated client schema mismatch
        payload = {"path": path, "skip_tests": skip_tests, "force": force}
        response = self.post("/api/v1/packs/register", json=payload)

        if response.status_code in (200, 201):
            data = response.json()
            if isinstance(data, dict) and "data" in data:
                return data["data"]
            return data
        raise Exception(
            f"Failed to register pack: {response.status_code} {response.text}"
        )

    def upload_pack(
        self, pack_dir: str, force: bool = False, skip_tests: bool = True
    ) -> dict:
        """Upload a pack directory as a tarball to the API

        This works across container boundaries (test container → API container)
        by creating a .tar.gz archive and POSTing it to /api/v1/packs/upload.

        Args:
            pack_dir: Local path to pack directory
            force: Overwrite existing pack (default: False)
            skip_tests: Skip pack tests (default: True)
        """
        import io
        import tarfile

        # Create in-memory tarball
        buf = io.BytesIO()
        with tarfile.open(fileobj=buf, mode="w:gz") as tar:
            tar.add(pack_dir, arcname=".")
        buf.seek(0)

        # Upload via multipart form
        client = self._get_client()
        files = {"pack": ("pack.tar.gz", buf, "application/gzip")}
        data = {}
        if force:
            data["force"] = "true"
        if skip_tests:
            data["skip_tests"] = "true"

        response = client.get_httpx_client().post(
            "/api/v1/packs/upload", files=files, data=data
        )

        if response.status_code in (200, 201):
            resp_data = response.json()
            # Upload response wraps pack data in {"data": {"pack": {...}, ...}}
            if isinstance(resp_data, dict) and "data" in resp_data:
                resp_data = resp_data["data"]
            # Extract just the pack object if wrapped in {"pack": {...}}
            if isinstance(resp_data, dict) and "pack" in resp_data:
                pack = resp_data["pack"]
            else:
                pack = resp_data
            if isinstance(pack, dict) and pack.get("ref"):
                self._created_pack_refs.append(pack["ref"])
            return pack
        raise Exception(
            f"Failed to upload pack: {response.status_code} {response.text}"
        )

    def reload_pack(self, pack_id: int) -> dict:
        """Reload a pack"""
        # Not implemented in wrapper yet
        raise NotImplementedError("reload_pack not yet implemented")

    def delete_pack(self, pack_id: int | str):
        """Delete a pack"""
        if isinstance(pack_id, str):
            gen_delete_pack.sync(ref=pack_id, client=self._get_client())
            return

        pack = self.get_pack(pack_id)
        if pack:
            gen_delete_pack.sync(ref=pack["ref"], client=self._get_client())

    # ========================================================================
    # Actions
    # ========================================================================

    def list_actions(self, **params) -> list[dict]:
        """List all actions"""
        requested_page = params.get("page")
        per_page = min(int(params.get("limit") or params.get("per_page") or 100), 100)
        items = []
        page = int(requested_page or 1)
        while True:
            response = self._request(
                "GET",
                "/api/v1/actions",
                params={"page": page, "per_page": per_page, "page_size": per_page},
            )
            if response.status_code != 200:
                return []
            data = response.json()
            page_items = data.get("items") or data.get("data") or []
            items.extend(page_items)
            if requested_page:
                break
            pagination = data.get("pagination") if isinstance(data.get("pagination"), dict) else {}
            total_pages = data.get("total_pages") or pagination.get("total_pages")
            if total_pages is not None:
                if page >= int(total_pages):
                    break
            elif len(page_items) < per_page:
                break
            page += 1
        # Client-side pack filter (API no longer supports server-side filtering)
        pack = params.get("pack") or params.get("pack_ref")
        if pack:
            items = [a for a in items if a.get("pack_ref") == pack or a.get("ref", "").startswith(f"{pack}.")]
        return items

    def get_action(self, action_id: int) -> dict:
        """Get action by ID - needs ref lookup"""
        actions = self.list_actions()
        for action in actions:
            if action.get("id") == action_id:
                return self.get_action_by_ref(action["ref"])
        raise Exception(f"Action {action_id} not found")

    def get_action_by_ref(self, ref: str) -> Optional[dict]:
        """Get action by reference"""
        response = gen_get_action.sync(ref=ref, client=self._get_client())
        return unwrap_item(response)

    def create_action(
        self,
        ref: Optional[str | dict] = None,
        label: Optional[str] = None,
        pack_ref: Optional[str] = None,
        entrypoint: Optional[str] = None,
        description: Optional[str] = None,
        param_schema: Optional[dict] = None,
        out_schema: Optional[dict] = None,
        runtime_ref: Optional[str] = None,
        name: Optional[str] = None,
        data: Optional[dict] = None,
        **kwargs,
    ) -> dict:
        """Create a new action

        Supports direct keyword arguments and data={...} dictionaries that use
        current API field names.
        """
        if isinstance(ref, dict) and data is None:
            data = ref
            ref = None

        # Handle dict-style argument (tier 2/3 tests pass data={...})
        if data is not None:
            ref = ref or data.get("ref")
            label = label or data.get("label")
            pack_ref = pack_ref or data.get("pack_ref")
            if ref and "." in ref and not pack_ref:
                pack_ref = ref.split(".", 1)[0]
            ref = qualify_ref(pack_ref, ref)
            name = name or data.get("name")
            if not name and ref and "." in ref:
                name = ref.split(".", 1)[1]
            label = label or (name.replace("_", " ").title() if name else None)
            description = description or data.get("description")
            entrypoint = entrypoint or data.get("entrypoint")
            param_schema = param_schema or data.get("param_schema")
            out_schema = out_schema or data.get("out_schema")
            runtime_ref = runtime_ref or data.get("runtime_ref")

        if pack_ref and name and not ref:
            ref = f"{pack_ref}.{safe_ref_part(name)}"
            label = label or name.replace("_", " ").title()

        # Default entrypoint if not provided
        if not entrypoint:
            entrypoint = "action.sh"
        ref = qualify_ref(pack_ref, ref)
        label = label or (name.replace("_", " ").title() if name else None) or (
            ref.split(".", 1)[1].replace("_", " ").title() if ref and "." in ref else None
        )

        # Validate required fields
        if not ref or not label or not pack_ref:
            raise ValueError(
                "Missing required arguments: ref, label, pack_ref "
                "(or pack_ref, name)"
            )

        # Use plain POST request instead of generated client to handle API schema changes
        payload = {
            "ref": ref,
            "label": label,
            "pack_ref": pack_ref,
            "entrypoint": entrypoint,
            "description": description or f"Action: {label}",
        }
        if param_schema:
            payload["param_schema"] = param_schema
        if out_schema:
            payload["out_schema"] = out_schema
        if runtime_ref:
            payload["runtime_ref"] = runtime_ref
        if data is not None and "default_execution_permission_set_refs" in data:
            payload["default_execution_permission_set_refs"] = data[
                "default_execution_permission_set_refs"
            ]

        response = self._request("POST", "/api/v1/actions", json=payload)
        if response.status_code in (200, 201):
            data = response.json()
            if "data" in data:
                action = data["data"]
            else:
                action = data
            if isinstance(action, dict) and action.get("ref"):
                self._created_action_refs.append(action["ref"])
            return action
        raise Exception(
            f"Failed to create action: {response.status_code} {response.text}"
        )

    def create_workflow(
        self,
        pack_ref: str,
        name: str,
        tasks: list,
        *,
        label: str = None,
        description: str = None,
        version: str = "1.0.0",
        param_schema: dict = None,
        out_schema: dict = None,
        tags: list = None,
        vars: list = None,
        output_map: dict = None,
    ) -> dict:
        """Create a workflow via POST /api/v1/workflows.

        This creates both a workflow_definition record and a companion
        action record so the workflow can be executed like any action.

        Args:
            pack_ref: Pack the workflow belongs to
            name: Workflow name (used to build ref as {pack_ref}.{name})
            tasks: List of task dicts with name, action, input, next, etc.
            label: Human-readable label (defaults to name titlecased)
            description: Workflow description
            version: Semantic version string
            param_schema: Flat parameter schema dict
            out_schema: Flat output schema dict
            tags: List of tag strings
            vars: Workflow variables list
            output_map: Output mapping dict
        """
        ref = f"{pack_ref}.{name}"
        label = label or name.replace("_", " ").title()

        definition = {"version": version, "tasks": tasks}
        if vars:
            definition["vars"] = vars
        if output_map:
            definition["output_map"] = output_map

        payload = {
            "ref": ref,
            "pack_ref": pack_ref,
            "label": label,
            "description": description or f"Workflow: {label}",
            "version": version,
            "definition": definition,
        }
        if param_schema:
            payload["param_schema"] = param_schema
        if out_schema:
            payload["out_schema"] = out_schema
        if tags:
            payload["tags"] = tags

        response = self._request("POST", "/api/v1/workflows", json=payload)
        if response.status_code in (200, 201):
            data = response.json()
            if "data" in data:
                return data["data"]
            return data
        raise Exception(
            f"Failed to create workflow: {response.status_code} {response.text}"
        )

    def delete_action(self, action_id: int | str):
        """Delete an action"""
        if isinstance(action_id, str):
            gen_delete_action.sync(ref=action_id, client=self._get_client())
            return

        action = self.get_action(action_id)
        if action:
            gen_delete_action.sync(ref=action["ref"], client=self._get_client())

    # ========================================================================
    # Triggers
    # ========================================================================

    def list_triggers(self, **params) -> list[dict]:
        """List all triggers"""
        response = gen_list_triggers.sync(client=self._get_client(), page_size=1000)
        return unwrap_list(response)

    def get_trigger(self, trigger_id: int) -> dict:
        """Get trigger by ID"""
        # Use paginated listing with max page_size to handle many triggers
        response = self.get("/api/v1/triggers", params={"page_size": 100})
        if response.status_code == 200:
            data = response.json()
            triggers = data.get("items", data.get("data", []))
            if isinstance(triggers, list):
                for trigger in triggers:
                    if trigger.get("id") == trigger_id:
                        return self.get_trigger_by_ref(trigger["ref"])
            # Check additional pages
            pagination = data.get("pagination", {})
            page = 2
            while pagination.get("has_next"):
                response = self.get("/api/v1/triggers", params={"page_size": 100, "page": page})
                if response.status_code != 200:
                    break
                data = response.json()
                triggers = data.get("items", data.get("data", []))
                if isinstance(triggers, list):
                    for trigger in triggers:
                        if trigger.get("id") == trigger_id:
                            return self.get_trigger_by_ref(trigger["ref"])
                pagination = data.get("pagination", {})
                page += 1
        raise Exception(f"Trigger {trigger_id} not found")

    def get_trigger_by_ref(self, ref: str) -> Optional[dict]:
        """Get trigger by reference"""
        response = gen_get_trigger.sync(ref=ref, client=self._get_client())
        return unwrap_item(response)

    def create_trigger(
        self,
        ref: Optional[str] = None,
        label: Optional[str] = None,
        pack_ref: Optional[str] = None,
        description: Optional[str] = None,
        param_schema: Optional[dict] = None,
        out_schema: Optional[dict] = None,
        name: Optional[str] = None,
        trigger_type: Optional[str] = None,
        parameters: Optional[dict] = None,
        **kwargs,
    ) -> dict:
        """Create a new trigger

        Supports direct keyword arguments that match the current trigger API.
        """
        if pack_ref and name:
            ref = f"{pack_ref}.{name}"
            label = name.replace("_", " ").title()

        ref = qualify_ref(pack_ref, ref)

        # Validate required fields
        if not ref or not label or not pack_ref:
            raise ValueError(
                "Missing required arguments: ref, label, and pack_ref (or pack_ref and name)"
            )

        # Use plain POST request instead of generated client to handle API schema changes
        payload = {
            "ref": ref,
            "label": label,
            "pack_ref": pack_ref,
        }

        if description:
            payload["description"] = description
        if param_schema:
            payload["param_schema"] = param_schema
        if out_schema:
            payload["out_schema"] = out_schema

        response = self._request("POST", "/api/v1/triggers", json=payload)
        if response.status_code in (200, 201):
            data = response.json()
            if "data" in data:
                trigger = data["data"]
            else:
                trigger = data
            if isinstance(trigger, dict) and trigger.get("ref"):
                self._created_trigger_refs.append(trigger["ref"])
            return trigger
        raise Exception(
            f"Failed to create trigger: {response.status_code} {response.text}"
        )

    def delete_trigger(self, trigger_id: int):
        """Delete a trigger"""
        trigger = self.get_trigger(trigger_id)
        if trigger:
            gen_delete_trigger.sync(ref=trigger["ref"], client=self._get_client())

    def enable_webhook(
        self,
        trigger_ref: Optional[str] = None,
        trigger_id: Optional[int] = None,
    ) -> dict:
        """Enable webhooks for a trigger

        Requires a trigger reference.
        """
        if not trigger_ref:
            raise ValueError("trigger_ref is required")

        response = self._request(
            "POST", f"/api/v1/triggers/{trigger_ref}/webhooks/enable"
        )
        if response.status_code in (200, 201):
            data = response.json()
            if "data" in data:
                trigger = data["data"]
            else:
                trigger = data
            return trigger
        raise Exception(
            f"Failed to enable webhook: {response.status_code} {response.text}"
        )

    def disable_webhook(
        self,
        trigger_ref: Optional[str] = None,
        trigger_id: Optional[int] = None,
    ) -> dict:
        """Disable webhooks for a trigger

        Requires a trigger reference.
        """
        if not trigger_ref:
            raise ValueError("trigger_ref is required")

        response = self._request(
            "POST", f"/api/v1/triggers/{trigger_ref}/webhooks/disable"
        )
        if response.status_code in (200, 201):
            data = response.json()
            if "data" in data:
                return data["data"]
            return data
        raise Exception(
            f"Failed to disable webhook: {response.status_code} {response.text}"
        )

    def fire_webhook(
        self,
        trigger_ref: Optional[str] = None,
        payload: Optional[dict] = None,
        auto_enable: bool = True,
    ) -> dict:
        """Fire a webhook trigger

        Requires a trigger reference.

        Args:
            trigger_ref: Trigger reference
            payload: Webhook payload
            auto_enable: Automatically enable webhooks if not enabled (default: True)
        """
        if not trigger_ref:
            raise ValueError("trigger_ref is required")

        # Get the trigger to check if webhooks are enabled
        trigger = self.get_trigger_by_ref(trigger_ref)
        if not trigger:
            raise Exception(f"Trigger {trigger_ref} not found")

        # Enable webhooks if not enabled and auto_enable is True
        if not trigger.get("webhook_enabled") and auto_enable:
            trigger = self.enable_webhook(trigger_ref=trigger_ref)

        # Check if we have a webhook_key now
        if not trigger.get("webhook_key"):
            raise Exception(f"Trigger {trigger_ref} does not have a webhook_key")

        # Use plain POST request instead of generated client to handle API response structure
        webhook_key = trigger["webhook_key"]
        response = self._request(
            "POST",
            f"/api/v1/webhooks/{webhook_key}",
            json={"payload": payload},
        )
        if response.status_code in (200, 201):
            data = response.json()
            if "data" in data:
                return data["data"]
            return data
        raise Exception(
            f"Failed to fire webhook: {response.status_code} {response.text}"
        )

    # ========================================================================
    # Sensors
    # ========================================================================

    def list_sensors(self, **params) -> list[dict]:
        """List all sensors"""
        response = gen_list_sensors.sync(client=self._get_client())
        return unwrap_list(response)

    def get_sensor(self, sensor_id: int) -> dict:
        """Get sensor by ID"""
        sensors = self.list_sensors()
        for sensor in sensors:
            if sensor.get("id") == sensor_id:
                response = gen_get_sensor.sync(
                    ref=sensor.get("ref", str(sensor_id)), client=self._get_client()
                )
                if response:
                    result = to_dict(response)
                    if isinstance(result, dict) and "data" in result:
                        return result["data"]
        raise Exception(f"Sensor {sensor_id} not found")

    def create_sensor(
        self,
        trigger_types: Optional[list] = None,
        enabled: bool = True,
        parameters: Optional[dict] = None,
        **kwargs,
    ) -> dict:
        """Create a new sensor"""
        # Extract required fields from kwargs or use defaults
        ref = kwargs.get("ref", "test_sensor")
        pack_ref = kwargs.get("pack_ref", "core")
        runtime_ref = kwargs.get("runtime_ref", "python3")
        label = kwargs.get("label", f"Sensor {ref}")
        entrypoint = kwargs.get("entrypoint", "internal://sensor")
        description = kwargs.get("description", f"Sensor {ref}")
        param_schema = kwargs.get("param_schema")
        config = kwargs.get("config")

        request = CreateSensorRequest(
            ref=ref,
            pack_ref=pack_ref,
            runtime_ref=runtime_ref,
            label=label,
            entrypoint=entrypoint,
            description=description,
            param_schema=param_schema,
            config=config,
            enabled=enabled,
        )

        response = gen_create_sensor.sync(client=self._get_client(), body=request)

        return unwrap_item(response)

    def delete_sensor(self, sensor_id: int):
        """Delete a sensor"""
        sensor = self.get_sensor(sensor_id)
        if sensor:
            gen_delete_sensor.sync(
                ref=sensor.get("ref", str(sensor_id)), client=self._get_client()
            )

    # ========================================================================
    # Rules
    # ========================================================================

    def list_rules(self, **params) -> list[dict]:
        """List all rules"""
        per_page = params.get("per_page", params.get("limit", 100))
        page = params.get("page", 1)
        items: list[dict] = []

        while True:
            response = self._request(
                "GET",
                "/api/v1/rules",
                params={"page": page, "per_page": per_page},
            )
            if response.status_code != 200:
                return items
            payload = response.json()
            page_items = payload.get("data", payload.get("items", []))
            if not isinstance(page_items, list):
                return items
            items.extend(page_items)

            pagination = payload.get("pagination", {})
            total_pages = payload.get("total_pages") or pagination.get("total_pages")
            if total_pages is not None:
                if page >= int(total_pages):
                    break
            elif len(page_items) < per_page:
                break
            page += 1

        return items

    def get_rule(self, rule_id: int) -> dict:
        """Get rule by ID"""
        rules = self.list_rules()
        for rule in rules:
            if rule.get("id") == rule_id:
                response = gen_get_rule.sync(
                    ref=rule.get("ref", str(rule_id)), client=self._get_client()
                )
                if response:
                    result = to_dict(response)
                    if isinstance(result, dict) and "data" in result:
                        return result["data"]
        raise Exception(f"Rule {rule_id} not found")

    def create_rule(
        self,
        ref: Optional[str | dict] = None,
        label: Optional[str] = None,
        pack_ref: Optional[str] = None,
        trigger_ref: Optional[str] = None,
        action_ref: Optional[str] = None,
        enabled: bool = True,
        description: Optional[str] = None,
        action_params: Optional[dict] = None,
        trigger_params: Optional[dict] = None,
        name: Optional[str] = None,
        data: Optional[dict] = None,
        **kwargs,
    ) -> dict:
        """Create a new rule

        Supports current rule API keyword args and data={...} dictionaries.
        """
        if isinstance(ref, dict) and data is None:
            data = ref
            ref = None

        # Handle dict-style argument
        if data is not None:
            ref = ref or data.get("ref")
            name = name or data.get("name")
            label = label or data.get("label")
            pack_ref = pack_ref or data.get("pack_ref")
            if ref and "." in ref and not pack_ref:
                pack_ref = ref.split(".", 1)[0]
            ref = qualify_ref(pack_ref, ref)
            if not name and ref and "." in ref:
                name = ref.split(".", 1)[1]
            description = description or data.get("description")
            trigger_ref = trigger_ref or data.get("trigger_ref")
            action_ref = action_ref or data.get("action_ref")
            if isinstance(trigger_ref, dict):
                trigger_ref = trigger_ref.get("ref")
            if isinstance(action_ref, dict):
                action_ref = action_ref.get("ref")
            if not pack_ref and isinstance(action_ref, str) and "." in action_ref:
                pack_ref = action_ref.split(".", 1)[0]
            if not pack_ref and isinstance(trigger_ref, str) and "." in trigger_ref:
                pack_ref = trigger_ref.split(".", 1)[0]
            trigger_ref = qualify_ref(pack_ref, trigger_ref)
            action_ref = qualify_ref(pack_ref, action_ref)
            label = label or (name.replace("_", " ").title() if name else None)
            enabled = data.get("enabled", enabled)
            action_params = action_params or data.get("action_params")
            trigger_params = trigger_params or data.get("trigger_params")
            if data.get("conditions") is not None:
                kwargs["conditions"] = data["conditions"]

        if pack_ref and name and not ref:
            ref = f"{pack_ref}.{safe_ref_part(name)}"
            label = label or name.replace("_", " ").title()


        if isinstance(trigger_ref, dict):
            trigger_ref = trigger_ref.get("ref")
        if isinstance(action_ref, dict):
            action_ref = action_ref.get("ref")
        if not pack_ref and isinstance(action_ref, str) and "." in action_ref:
            pack_ref = action_ref.split(".", 1)[0]
        if not pack_ref and isinstance(trigger_ref, str) and "." in trigger_ref:
            pack_ref = trigger_ref.split(".", 1)[0]
        trigger_ref = qualify_ref(pack_ref, trigger_ref)
        action_ref = qualify_ref(pack_ref, action_ref)
        label = label or (name.replace("_", " ").title() if name else None) or (
            ref.split(".", 1)[1].replace("_", " ").title() if ref and "." in ref else None
        )
        if pack_ref and name and not ref:
            ref = f"{pack_ref}.{safe_ref_part(name)}"

        # Validate required fields
        if not ref or not label or not pack_ref or not trigger_ref or not action_ref:
            raise ValueError(
                "Missing required arguments: ref, label, pack_ref, trigger_ref, and action_ref "
                "(or pack_ref, name, trigger_ref, and action_ref)"
            )

        # Use plain POST request instead of generated client to handle API schema changes
        payload = {
            "ref": ref,
            "label": label,
            "pack_ref": pack_ref,
            "trigger_ref": trigger_ref,
            "action_ref": action_ref,
            "enabled": enabled,
            "description": description or f"Rule: {label}",
        }

        if kwargs.get("conditions") is not None:
            payload["conditions"] = kwargs["conditions"]
        explicit_action_params = kwargs.get("action_params", action_params)
        if explicit_action_params is not None:
            payload["action_params"] = explicit_action_params

        explicit_trigger_params = kwargs.get("trigger_params", trigger_params)
        if explicit_trigger_params is not None:
            payload["trigger_params"] = explicit_trigger_params

        response = self._request("POST", "/api/v1/rules", json=payload)
        if response.status_code in (200, 201):
            data = response.json()
            if "data" in data:
                rule = data["data"]
            else:
                rule = data
            if isinstance(rule, dict) and rule.get("ref"):
                self._created_rule_refs.append(rule["ref"])
            return rule
        raise Exception(
            f"Failed to create rule: {response.status_code} {response.text}"
        )

    def update_rule(self, rule_id: int, **kwargs) -> dict:
        """Update a rule"""
        raise NotImplementedError("update_rule not yet implemented")

    def enable_rule(self, rule_id: int) -> dict:
        """Enable a rule"""
        rule = self.get_rule(rule_id)
        response = gen_enable_rule.sync(
            ref=rule.get("ref", str(rule_id)), client=self._get_client()
        )
        return unwrap_item(response) or {}

    def disable_rule(self, rule_id: int) -> dict:
        """Disable a rule"""
        rule = self.get_rule(rule_id)
        response = gen_disable_rule.sync(
            ref=rule.get("ref", str(rule_id)), client=self._get_client()
        )
        return unwrap_item(response) or {}

    def delete_rule(self, rule_id: int):
        """Delete a rule"""
        rule = self.get_rule(rule_id)
        if rule:
            gen_delete_rule.sync(
                ref=rule.get("ref", str(rule_id)), client=self._get_client()
            )

    # ========================================================================
    # Events
    # ========================================================================

    def list_events(self, enrich: bool = True, **params) -> list[dict]:
        """List all events (optionally enriched with full payload from individual fetches)

        Args:
            enrich: If True, fetch full payload for each event (slow for large lists).
                    If False, return list response as-is (has rule, created, trigger fields).
        """
        trigger = params.get("trigger_id") or params.get("trigger")
        requested_page = params.get("page")
        per_page = min(int(params.get("limit") or params.get("per_page") or 100), 1000)
        page = int(requested_page or 1)
        items = []

        while True:
            page_params = {
                "trigger": trigger,
                "trigger_ref": params.get("trigger_ref"),
                "source": params.get("source"),
                "page": page,
                "per_page": per_page,
            }
            page_params = {
                key: value
                for key, value in page_params.items()
                if value not in (None, "")
            }
            response = self._request(
                "GET",
                "/api/v1/events",
                params=page_params,
            )
            if response.status_code != 200:
                break

            data = response.json()
            page_items = data.get("items") or data.get("data") or []
            if not isinstance(page_items, list):
                break
            items.extend(page_items)

            if requested_page:
                break

            pagination = (
                data.get("pagination") if isinstance(data.get("pagination"), dict) else {}
            )
            total_pages = data.get("total_pages") or pagination.get("total_pages")
            if total_pages is not None:
                if page >= int(total_pages):
                    break
            elif len(page_items) < per_page:
                break

            page += 1

        # Client-side rule_id filter (API doesn't support rule_id param)
        rule_id = params.get("rule_id")
        if rule_id is not None:
            items = [e for e in items if e.get("rule") == rule_id]

        if not enrich:
            return items
        # Enrich events with full payload (list endpoint only returns summaries)
        enriched = []
        for event in items:
            if event.get("has_payload") and "payload" not in event:
                try:
                    full = self.get_event(event["id"])
                    enriched.append(full)
                except Exception:
                    enriched.append(event)
            else:
                enriched.append(event)
        return enriched

    def get_event(self, event_id: int) -> dict:
        """Get event by ID (full response with payload)"""
        response = gen_get_event.sync(id=event_id, client=self._get_client())
        result = unwrap_item(response)
        if result:
            return result
        raise Exception(f"Event {event_id} not found")

    # ========================================================================
    # Enforcements
    # ========================================================================

    def list_enforcements(self, **params) -> list[dict]:
        """List all enforcements"""
        requested_page = params.get("page")
        per_page = min(int(params.get("limit") or params.get("per_page") or 100), 1000)
        page = int(requested_page or 1)
        items = []

        while True:
            page_params = {
                "rule": params.get("rule_id") or params.get("rule"),
                "rule_ref": params.get("rule_ref"),
                "status": params.get("status"),
                "page": page,
                "per_page": per_page,
            }
            page_params = {
                key: value
                for key, value in page_params.items()
                if value not in (None, "")
            }
            response = self._request(
                "GET",
                "/api/v1/enforcements",
                params=page_params,
            )
            if response.status_code != 200:
                return items

            payload = response.json()
            page_items = payload.get("items") or payload.get("data") or []
            if not isinstance(page_items, list):
                return items
            items.extend(page_items)

            if requested_page:
                break

            pagination = (
                payload.get("pagination")
                if isinstance(payload.get("pagination"), dict)
                else {}
            )
            total_pages = payload.get("total_pages") or pagination.get("total_pages")
            if total_pages is not None:
                if page >= int(total_pages):
                    break
            elif len(page_items) < per_page:
                break

            page += 1

        return items

    def get_enforcement(self, enforcement_id: int) -> dict:
        """Get enforcement by ID"""
        enforcements = self.list_enforcements(limit=1000)
        for enforcement in enforcements:
            if enforcement.get("id") == enforcement_id:
                return enforcement
        raise Exception(f"Enforcement {enforcement_id} not found")

    # ========================================================================
    # Executions
    # ========================================================================

    def list_executions(self, **params) -> list[dict]:
        """List all executions"""
        query_params = {}
        if params.get("action_ref"):
            query_params["action_ref"] = params["action_ref"]
        if params.get("status"):
            query_params["status"] = params["status"]
        if params.get("enforcement_id"):
            query_params["enforcement"] = params["enforcement_id"]
        if params.get("enforcement"):
            query_params["enforcement"] = params["enforcement"]
        if params.get("parent"):
            query_params["parent"] = params["parent"]
        if params.get("parent_id"):
            query_params["parent"] = params["parent_id"]
        per_page = params.get("limit") or params.get("per_page")
        requested_page = params.get("page")
        per_page = min(int(per_page or 100), 1000)
        page = int(requested_page or 1)
        items = []

        while True:
            page_params = {**query_params, "page": page, "per_page": per_page}
            response = self._request("GET", "/api/v1/executions", params=page_params)
            if response.status_code != 200:
                return items

            data = response.json()
            page_items = data.get("items") or data.get("data") or []
            if isinstance(data, list):
                page_items = data
            if not isinstance(page_items, list):
                return items
            items.extend(page_items)

            if requested_page or isinstance(data, list):
                break

            pagination = (
                data.get("pagination") if isinstance(data.get("pagination"), dict) else {}
            )
            total_pages = data.get("total_pages") or pagination.get("total_pages")
            if total_pages is not None:
                if page >= int(total_pages):
                    break
            elif len(page_items) < per_page:
                break

            page += 1

        return items

    def get_execution(self, execution_id: int) -> dict:
        """Get execution by ID"""
        response = self._request("GET", f"/api/v1/executions/{execution_id}")
        if response.status_code == 200:
            data = response.json()
            if "data" in data:
                return data["data"]
            return data
        raise Exception(f"Execution {execution_id} not found")

    def create_execution(
        self,
        action_ref: str,
        parameters: Optional[dict] = None,
        env_vars: Optional[dict] = None,
        permission_set_refs: Optional[list[str]] = None,
    ) -> dict:
        """Create and queue an execution"""
        payload: dict = {"action_ref": action_ref}
        if parameters:
            payload["parameters"] = parameters
        if env_vars:
            payload["env_vars"] = env_vars
        if permission_set_refs is not None:
            payload["permission_set_refs"] = permission_set_refs
        response = self._request("POST", "/api/v1/executions/execute", json=payload)
        if response.status_code in (200, 201):
            data = response.json()
            if "data" in data:
                return data["data"]
            return data
        raise Exception(
            f"Failed to create execution: {response.status_code} {response.text}"
        )

    def execute_action(self, data: dict | str, parameters: Optional[dict] = None, **kwargs) -> dict:
        """Create and queue an execution."""
        if isinstance(data, dict):
            action_ref = data.get("action_ref") or data.get("action")
            parameters = data.get("parameters", parameters)
            env_vars = data.get("env_vars")
            permission_set_refs = data.get("permission_set_refs")
        else:
            action_ref = data
            env_vars = kwargs.get("env_vars")
            permission_set_refs = kwargs.get("permission_set_refs")
        if isinstance(action_ref, dict):
            action_ref = action_ref.get("ref")
        return self.create_execution(
            action_ref=action_ref,
            parameters=parameters,
            env_vars=env_vars,
            permission_set_refs=permission_set_refs,
        )

    def cancel_execution(self, execution_id: int) -> dict:
        """Cancel an execution"""
        response = self._request(
            "POST", f"/api/v1/executions/{execution_id}/cancel", json={}
        )
        if response.status_code in (200, 201):
            data = response.json()
            if "data" in data:
                return data["data"]
            return data
        raise Exception(
            f"Failed to cancel execution: {response.status_code} {response.text}"
        )

    # ========================================================================
    # Webhooks
    # ========================================================================

    def post_webhook(self, webhook_url: str, payload: Optional[dict] = None) -> dict:
        """Post data to a webhook URL.

        Args:
            webhook_url: Full webhook URL or just the key path
            payload: JSON payload to send
        """
        # webhook_url may be full URL or just path like /api/v1/webhooks/{key}
        if webhook_url.startswith("http"):
            # Extract path from full URL
            from urllib.parse import urlparse

            parsed = urlparse(webhook_url)
            path = parsed.path
        else:
            path = webhook_url

        body = payload if isinstance(payload, dict) and "payload" in payload else {"payload": payload or {}}
        response = self._request("POST", path, json=body)
        if response.status_code in (200, 201, 202):
            return response.json() if response.text else {}
        raise Exception(
            f"Webhook POST failed: {response.status_code} {response.text}"
        )

    # ========================================================================
    # Inquiries
    # ========================================================================

    def create_inquiry(
        self,
        execution_id: Optional[int] = None,
        data: Optional[dict] = None,
        **kwargs,
    ) -> dict:
        """Create an inquiry.

        Supports both keyword args and data={...} dict style.
        """
        if data is not None:
            execution_id = execution_id or data.get("execution_id")
            prompt = data.get("prompt", data.get("message", "Please respond"))
            response_schema = data.get("schema") or data.get("response_schema")
            assigned_to = data.get("assigned_to")
            timeout_at = data.get("timeout_at") or data.get("ttl")
        else:
            prompt = kwargs.get("prompt", "Please respond")
            response_schema = kwargs.get("schema") or kwargs.get("response_schema")
            assigned_to = kwargs.get("assigned_to")
            timeout_at = kwargs.get("timeout_at")

        payload: dict = {
            "execution": execution_id or 0,
            "prompt": prompt,
        }
        if response_schema:
            payload["response_schema"] = response_schema
        if assigned_to:
            payload["assigned_to"] = assigned_to
        if timeout_at:
            # Convert TTL (integer seconds) to ISO timestamp if needed
            if isinstance(timeout_at, (int, float)):
                from datetime import datetime, timezone, timedelta

                timeout_at = (
                    datetime.now(timezone.utc) + timedelta(seconds=timeout_at)
                ).isoformat()
            payload["timeout_at"] = timeout_at

        response = self._request("POST", "/api/v1/inquiries", json=payload)
        if response.status_code in (200, 201):
            resp_data = response.json()
            if "data" in resp_data:
                return resp_data["data"]
            return resp_data
        raise Exception(
            f"Failed to create inquiry: {response.status_code} {response.text}"
        )

    def list_inquiries(self, **params) -> list[dict]:
        """List all inquiries"""
        response = gen_list_inquiries.sync(
            client=self._get_client(),
            status=params.get("status"),
            limit=params.get("limit"),
            offset=params.get("offset"),
        )
        return unwrap_list(response)

    def get_inquiry(self, inquiry_id: int) -> dict:
        """Get inquiry by ID"""
        resp = self._request("GET", f"/api/v1/inquiries/{inquiry_id}")
        if resp.status_code == 200:
            data = resp.json()
            if "data" in data:
                return data["data"]
            return data
        # Fallback to list scan
        inquiries = self.list_inquiries(limit=1000)
        for inquiry in inquiries:
            if inquiry.get("id") == inquiry_id:
                return inquiry
        raise Exception(f"Inquiry {inquiry_id} not found")

    def respond_to_inquiry(
        self, inquiry_id: int, response_data: dict = None, response: dict = None
    ) -> dict:
        """Respond to an inquiry"""
        data = response_data or response or {}
        resp = self._request(
            "POST", f"/api/v1/inquiries/{inquiry_id}/respond", json={"response": data}
        )
        if resp.status_code in (200, 201):
            resp_data = resp.json()
            if "data" in resp_data:
                return resp_data["data"]
            return resp_data
        raise Exception(
            f"Failed to respond to inquiry: {resp.status_code} {resp.text}"
        )

    # ========================================================================
    # Secrets (Datastore/Keys)
    # ========================================================================

    def list_secrets(self, **params) -> list[dict]:
        """List all secrets/keys"""
        response = gen_list_keys.sync(client=self._get_client())
        return unwrap_list(response)

    def get_secret(self, key_name: str, **params) -> Optional[dict]:
        """Get secret by key name"""
        if not key_name.startswith(("system.", "identity.", "pack.", "action.", "sensor.")):
            key_name = f"system.{key_name}"
        response = gen_get_key.sync(ref=key_name, client=self._get_client())
        return unwrap_item(response)

    def datastore_get(self, key: str, **params) -> Optional[str]:
        """Get value from datastore"""
        secret = self.get_secret(key)
        return secret.get("value") if secret else None

    def datastore_set(self, key: str, value: str, **params) -> dict:
        """Set value in datastore"""
        # Check if key exists
        existing = self.get_secret(key)
        if existing:
            # Update existing
            request = UpdateKeyRequest(value=value)
            response = gen_update_key.sync(
                ref=existing["ref"], client=self._get_client(), body=request
            )
        else:
            # Create new
            encrypted = params.get("encrypted", False)
            request = CreateKeyRequest(
                local_ref=key.removeprefix("system."),
                name=key,
                value=value,
                owner_type=OwnerType.SYSTEM,
                encrypted=encrypted,
            )
            response = gen_create_key.sync(client=self._get_client(), body=request)

        return unwrap_item(response) or {}

    def datastore_delete(self, key: str, **params):
        """Delete value from datastore"""
        if not key.startswith(("system.", "identity.", "pack.", "action.", "sensor.")):
            key = f"system.{key}"
        gen_delete_key.sync(ref=key, client=self._get_client())

    def create_secret(self, key: str, value: str, **params) -> dict:
        """Create a new secret"""
        encrypted = params.get("encrypted", True)
        request = CreateKeyRequest(
            local_ref=key.removeprefix("system."),
            name=key,
            value=value,
            owner_type=OwnerType.SYSTEM,
            encrypted=encrypted,
        )
        response = gen_create_key.sync(client=self._get_client(), body=request)
        return unwrap_item(response) or {}

    def update_secret(self, key_id: int, **params) -> dict:
        """Update a secret"""
        # Need to get key ref first
        secrets = self.list_secrets()
        for secret in secrets:
            if secret.get("id") == key_id:
                request = UpdateKeyRequest(value=params.get("value"))
                response = gen_update_key.sync(
                    ref=secret.get("key") or secret["ref"], client=self._get_client(), body=request
                )
                if response:
                    result = to_dict(response)
                    if isinstance(result, dict) and "data" in result:
                        return result["data"]
        return {}

    def delete_secret(self, key_id: int):
        """Delete a secret"""
        secrets = self.list_secrets()
        for secret in secrets:
            if secret.get("id") == key_id:
                gen_delete_key.sync(ref=secret.get("key") or secret["ref"], client=self._get_client())
                return

    # Datastore-style convenience helpers backed by key storage.
    def get_datastore_item(self, key: str, **params) -> Optional[dict]:
        """Get datastore item (alias for get_secret)"""
        return self.get_secret(key)

    def set_datastore_item(self, key: str, value: str, **params) -> dict:
        """Set datastore item (alias for datastore_set)"""
        return self.datastore_set(key, value, **params)

    # ========================================================================
    # Cache namespaces and immutable generations
    # ========================================================================
    # These helpers use the generated cache request/response modules while
    # preserving the wrapper's uniform CacheApiError behavior for negative
    # E2E assertions.

    @staticmethod
    def _cache_owner_type(owner_type: str) -> OwnerType:
        return OwnerType(owner_type)

    @staticmethod
    def _cache_owner_ref(owner_ref: Optional[str]):
        return owner_ref if owner_ref else UNSET

    @staticmethod
    def _cache_generated_data(response: Any) -> dict | list:
        if not 200 <= int(response.status_code) < 300:
            raise CacheApiError(response)
        if response.parsed is not None:
            body = to_dict(response.parsed)
        else:
            body = json.loads(response.content or b"{}")
        if isinstance(body, dict) and "data" in body:
            return body["data"]
        return body

    def cache_create_namespace(
        self,
        *,
        owner_type: str,
        owner_ref: Optional[str],
        namespace: str,
        policy: Optional[dict] = None,
    ) -> dict:
        """Create an owner-scoped cache namespace through the public API."""
        payload = {
            "owner_type": owner_type,
            "owner_ref": owner_ref,
            "namespace": namespace,
            **(policy or {}),
        }
        return self._cache_generated_data(
            gen_cache_create_namespace.sync_detailed(
                client=self._get_client(),
                body=CreateCacheNamespaceRequest.from_dict(payload),
            )
        )

    def cache_list_namespaces(self, *, owner_type: str, owner_ref: Optional[str]) -> list[dict]:
        data = self._cache_generated_data(
            gen_cache_list_namespaces.sync_detailed(
                client=self._get_client(),
                owner_type=self._cache_owner_type(owner_type),
                owner_ref=self._cache_owner_ref(owner_ref),
            )
        )
        if isinstance(data, dict):
            data = data.get("items", data.get("namespaces", []))
        return data if isinstance(data, list) else []

    def cache_get_namespace(
        self, *, owner_type: str, owner_ref: Optional[str], namespace: str
    ) -> dict:
        return self._cache_generated_data(
            gen_cache_show_namespace.sync_detailed(
                namespace=namespace,
                client=self._get_client(),
                owner_type=self._cache_owner_type(owner_type),
                owner_ref=self._cache_owner_ref(owner_ref),
            )
        )

    def cache_update_namespace(
        self,
        *,
        owner_type: str,
        owner_ref: Optional[str],
        namespace: str,
        policy: dict,
    ) -> dict:
        payload = {"owner_type": owner_type, "owner_ref": owner_ref, **policy}
        return self._cache_generated_data(
            gen_cache_update_namespace.sync_detailed(
                namespace=namespace,
                client=self._get_client(),
                body=UpdateCacheNamespaceRequest.from_dict(payload),
            )
        )

    def cache_delete_namespace(
        self, *, owner_type: str, owner_ref: Optional[str], namespace: str
    ) -> dict:
        return self._cache_generated_data(
            gen_cache_delete_namespace.sync_detailed(
                namespace=namespace,
                client=self._get_client(),
                owner_type=self._cache_owner_type(owner_type),
                owner_ref=self._cache_owner_ref(owner_ref),
            )
        )

    def cache_list_generations(
        self, *, owner_type: str, owner_ref: Optional[str], namespace: str
    ) -> list[dict]:
        data = self._cache_generated_data(
            gen_cache_list_generations.sync_detailed(
                namespace=namespace,
                client=self._get_client(),
                owner_type=self._cache_owner_type(owner_type),
                owner_ref=self._cache_owner_ref(owner_ref),
            )
        )
        if isinstance(data, dict):
            data = data.get("items", data.get("generations", []))
        return data if isinstance(data, list) else []

    def cache_get_generation(
        self,
        *,
        owner_type: str,
        owner_ref: Optional[str],
        namespace: str,
        generation_id: int,
    ) -> dict:
        return self._cache_generated_data(
            gen_cache_show_generation.sync_detailed(
                namespace=namespace,
                generation_id=generation_id,
                client=self._get_client(),
                owner_type=self._cache_owner_type(owner_type),
                owner_ref=self._cache_owner_ref(owner_ref),
            )
        )

    def cache_create_generation(
        self,
        *,
        owner_type: str,
        owner_ref: Optional[str],
        namespace: str,
        client_refresh_id: str,
        expected_active_generation_id: Optional[int],
        expected_chunk_count: int,
        expected_record_count: Optional[int] = None,
        expected_size_bytes: Optional[int] = None,
        source_revision: Optional[str] = None,
    ) -> dict:
        payload: dict[str, Any] = {
            "owner_type": owner_type,
            "owner_ref": owner_ref,
            "client_refresh_id": client_refresh_id,
            "expected_active_generation_id": expected_active_generation_id,
            "expected_chunk_count": expected_chunk_count,
        }
        if expected_record_count is not None:
            payload["expected_record_count"] = expected_record_count
        if expected_size_bytes is not None:
            payload["expected_size_bytes"] = expected_size_bytes
        if source_revision is not None:
            payload["source_revision"] = source_revision
        return self._cache_generated_data(
            gen_cache_create_generation.sync_detailed(
                namespace=namespace,
                client=self._get_client(),
                body=CreateCacheGenerationRequest.from_dict(payload),
            )
        )

    def cache_upload_chunk(
        self,
        *,
        owner_type: str,
        owner_ref: Optional[str],
        namespace: str,
        generation_id: int,
        chunk_index: int,
        entries: list[dict],
    ) -> dict:
        payload = {"owner_type": owner_type, "owner_ref": owner_ref, "entries": entries}
        return self._cache_generated_data(
            gen_cache_upload_chunk.sync_detailed(
                namespace=namespace,
                generation_id=generation_id,
                chunk_index=chunk_index,
                client=self._get_client(),
                body=UploadCacheChunkRequest.from_dict(payload),
            )
        )

    def cache_seal_generation(
        self,
        *,
        owner_type: str,
        owner_ref: Optional[str],
        namespace: str,
        generation_id: int,
        expected_chunk_count: int,
        expected_record_count: Optional[int] = None,
        expected_size_bytes: Optional[int] = None,
    ) -> dict:
        payload: dict[str, Any] = {
            "owner_type": owner_type,
            "owner_ref": owner_ref,
            "expected_chunk_count": expected_chunk_count,
        }
        if expected_record_count is not None:
            payload["expected_record_count"] = expected_record_count
        if expected_size_bytes is not None:
            payload["expected_size_bytes"] = expected_size_bytes
        return self._cache_generated_data(
            gen_cache_seal_generation.sync_detailed(
                namespace=namespace,
                generation_id=generation_id,
                client=self._get_client(),
                body=SealCacheGenerationRequest.from_dict(payload),
            )
        )

    def cache_promote_generation(
        self,
        *,
        owner_type: str,
        owner_ref: Optional[str],
        namespace: str,
        generation_id: int,
        expected_active_generation_id: Optional[int],
    ) -> dict:
        payload = {
            "owner_type": owner_type,
            "owner_ref": owner_ref,
            "expected_active_generation_id": expected_active_generation_id,
        }
        return self._cache_generated_data(
            gen_cache_promote_generation.sync_detailed(
                namespace=namespace,
                generation_id=generation_id,
                client=self._get_client(),
                body=PromoteCacheGenerationRequest.from_dict(payload),
            )
        )

    def cache_abandon_generation(
        self,
        *,
        owner_type: str,
        owner_ref: Optional[str],
        namespace: str,
        generation_id: int,
    ) -> dict:
        return self._cache_generated_data(
            gen_cache_abandon_generation.sync_detailed(
                namespace=namespace,
                generation_id=generation_id,
                client=self._get_client(),
                body=CacheOwnerBody.from_dict(
                    {"owner_type": owner_type, "owner_ref": owner_ref}
                ),
            )
        )

    def cache_lookup(
        self,
        *,
        owner_type: str,
        owner_ref: Optional[str],
        namespace: str,
        external_id: str,
        generation_id: Optional[int] = None,
        require_fresh: bool = False,
    ) -> dict:
        payload = {
            "owner_type": owner_type,
            "owner_ref": owner_ref,
            "external_id": external_id,
            "generation_id": generation_id,
            "require_fresh": require_fresh,
        }
        return self._cache_generated_data(
            gen_cache_lookup_entry.sync_detailed(
                namespace=namespace,
                client=self._get_client(),
                body=CachePointLookupRequest.from_dict(payload),
            )
        )

    def cache_lookup_many(
        self,
        *,
        owner_type: str,
        owner_ref: Optional[str],
        namespace: str,
        external_ids: list[str],
        generation_id: Optional[int] = None,
        require_fresh: bool = False,
    ) -> dict:
        payload = {
            "owner_type": owner_type,
            "owner_ref": owner_ref,
            "external_ids": external_ids,
            "generation_id": generation_id,
            "require_fresh": require_fresh,
        }
        return self._cache_generated_data(
            gen_cache_lookup_entries.sync_detailed(
                namespace=namespace,
                client=self._get_client(),
                body=CacheMultiLookupRequest.from_dict(payload),
            )
        )

    def cache_scan(
        self,
        *,
        owner_type: str,
        owner_ref: Optional[str],
        namespace: str,
        page_size: int,
        generation_id: Optional[int] = None,
        cursor: Optional[str] = None,
        require_fresh: bool = False,
    ) -> dict:
        return self._cache_generated_data(
            gen_cache_scan_entries.sync_detailed(
                namespace=namespace,
                client=self._get_client(),
                owner_type=self._cache_owner_type(owner_type),
                owner_ref=self._cache_owner_ref(owner_ref),
                limit=page_size,
                require_fresh=require_fresh,
                generation=generation_id if generation_id is not None else UNSET,
                cursor=cursor if cursor is not None else UNSET,
            )
        )

    def get_sensor_log(self, sensor_ref: str, stream: str = "stdout") -> str:
        """Read public sensor log output without inspecting the container filesystem."""
        response = self._request(
            "GET", f"/api/v1/sensors/{quote(sensor_ref, safe='')}/logs/{stream}"
        )
        if response.status_code != 200:
            raise RuntimeError(
                f"Failed to read sensor log {sensor_ref}/{stream}: "
                f"{response.status_code} {response.text}"
            )
        return response.text

    # ========================================================================
    # Raw request helpers
    # ========================================================================

    def _request(self, method: str, path: str, **kwargs):
        """Raw request method for tests that need endpoint-level access."""
        client = self._get_client()
        response = client.get_httpx_client().request(method, path, **kwargs)
        return TestResponse(response)

    def get(self, path: str, **kwargs):
        """GET request"""
        return self._request("GET", path, **kwargs)

    def post(self, path: str, **kwargs):
        """POST request"""
        return self._request("POST", path, **kwargs)

    def upload_action_files(self, action_ref: str | dict, files: dict[str, str]) -> dict:
        """Copy ad-hoc test action files into the running API container."""
        ref = action_ref.get("ref") if isinstance(action_ref, dict) else action_ref
        if not ref:
            return {"uploaded": []}

        action = self.get_action_by_ref(ref)
        pack_ref = (action or {}).get("pack_ref") or (ref.split(".", 1)[0] if "." in ref else "test_pack")
        uploaded = []
        for filename, content in files.items():
            uploaded.append(filename)
            self._copy_action_file_to_running_api(pack_ref, filename, content)

        return {"uploaded": uploaded}

    def _copy_action_file_to_running_api(self, pack_ref: str, filename: str, content: str) -> None:
        container = os.getenv("ATTUNE_API_CONTAINER", "attune-api")
        with tempfile.NamedTemporaryFile("w", delete=False) as tmp:
            tmp.write(content)
            tmp_path = tmp.name
        try:
            subprocess.run(
                [
                    "docker",
                    "exec",
                    container,
                    "mkdir",
                    "-p",
                    f"/opt/attune/packs/{pack_ref}/actions",
                ],
                check=False,
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
                timeout=5,
            )
            subprocess.run(
                [
                    "docker",
                    "cp",
                    tmp_path,
                    f"{container}:/opt/attune/packs/{pack_ref}/actions/{filename}",
                ],
                check=False,
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
                timeout=5,
            )
        except Exception:
            pass
        finally:
            try:
                os.unlink(tmp_path)
            except OSError:
                pass

    def put(self, path: str, **kwargs):
        """PUT request"""
        return self._request("PUT", path, **kwargs)

    def patch(self, path: str, **kwargs):
        """PATCH request"""
        return self._request("PATCH", path, **kwargs)

    def delete(self, path: str, **kwargs):
        """DELETE request"""
        return self._request("DELETE", path, **kwargs)
