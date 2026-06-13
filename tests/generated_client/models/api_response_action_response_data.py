from __future__ import annotations

import datetime
from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.action_reference_visibility import ActionReferenceVisibility
from ..models.retention_policy_type import RetentionPolicyType
from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.api_response_action_response_data_out_schema_type_0 import (
        ApiResponseActionResponseDataOutSchemaType0,
    )
    from ..models.api_response_action_response_data_param_schema_type_0 import (
        ApiResponseActionResponseDataParamSchemaType0,
    )
    from ..models.api_response_action_response_data_required_worker_runtimes import (
        ApiResponseActionResponseDataRequiredWorkerRuntimes,
    )
    from ..models.api_response_action_response_data_worker_selector import (
        ApiResponseActionResponseDataWorkerSelector,
    )
    from ..models.worker_affinity import WorkerAffinity
    from ..models.worker_toleration import WorkerToleration


T = TypeVar("T", bound="ApiResponseActionResponseData")


@_attrs_define
class ApiResponseActionResponseData:
    """Response DTO for action information

    Attributes:
        accesses_mcp (bool): Hint that this action may invoke the Attune MCP server and spawn child executions. Default:
            False.
        created (datetime.datetime): Creation timestamp Example: 2024-01-13T10:30:00Z.
        enabled (bool): Whether this action is enabled Example: True.
        entrypoint (str): Entry point Example: /actions/slack/post_message.py.
        id (int): Action ID Example: 1.
        is_adhoc (bool): Whether this is an ad-hoc action (not from pack installation)
        label (str): Human-readable label Example: Post Message to Slack.
        out_schema (ApiResponseActionResponseDataOutSchemaType0 | None): Output schema
        pack (int): Pack ID Example: 1.
        pack_ref (str): Pack reference Example: slack.
        param_schema (ApiResponseActionResponseDataParamSchemaType0 | None): Parameter schema (StackStorm-style with
            inline required/secret)
        ref (str): Unique reference identifier Example: slack.post_message.
        reference_visibility (ActionReferenceVisibility):
        updated (datetime.datetime): Last update timestamp Example: 2024-01-13T10:30:00Z.
        artifact_retention_limit (int | None | Unset): Per-action retention limit override for non-log artifacts created
            by executions. Example: 10.
        artifact_retention_policy (None | RetentionPolicyType | Unset):
        default_execution_permission_set_refs (list[str] | Unset): Default permission set refs used when executions do
            not explicitly override token permissions. Example: ['core.agent_reader'].
        description (None | str | Unset): Action description Example: Posts a message to a Slack channel.
        log_retention_limit (int | None | Unset): Per-action retention limit override for stdout/stderr execution log
            artifacts. Example: 4.
        log_retention_policy (None | RetentionPolicyType | Unset):
        reference_allowed_pack_refs (list[str] | Unset): Pack refs allowed to reference this action when visibility is
            restricted. Example: ['incident_response', 'deployments'].
        required_worker_runtimes (ApiResponseActionResponseDataRequiredWorkerRuntimes | Unset): Additional worker
            runtime requirements keyed by runtime name/alias. Use "*" for any available version.
        runtime (int | None | Unset): Runtime ID Example: 1.
        runtime_ref (None | str | Unset): Runtime reference (stable identifier, e.g., "core.python") Example:
            core.python.
        runtime_version_constraint (None | str | Unset): Semver version constraint for the runtime (e.g., ">=3.12",
            ">=3.12,<4.0", "~18.0") Example: >=3.12.
        timeout_seconds (int | None | Unset): Default execution timeout (seconds) snapshotted onto executions of this
            action. Example: 300.
        worker_affinity (WorkerAffinity | Unset):
        worker_selector (ApiResponseActionResponseDataWorkerSelector | Unset): Exact worker label requirements.
        worker_tolerations (list[WorkerToleration] | Unset): Tolerations for worker taints.
        workflow_def (int | None | Unset): Workflow definition ID (non-null if this action is a workflow) Example: 42.
    """

    created: datetime.datetime
    enabled: bool
    entrypoint: str
    id: int
    is_adhoc: bool
    label: str
    out_schema: ApiResponseActionResponseDataOutSchemaType0 | None
    pack: int
    pack_ref: str
    param_schema: ApiResponseActionResponseDataParamSchemaType0 | None
    ref: str
    reference_visibility: ActionReferenceVisibility
    updated: datetime.datetime
    accesses_mcp: bool = False
    artifact_retention_limit: int | None | Unset = UNSET
    artifact_retention_policy: None | RetentionPolicyType | Unset = UNSET
    default_execution_permission_set_refs: list[str] | Unset = UNSET
    description: None | str | Unset = UNSET
    log_retention_limit: int | None | Unset = UNSET
    log_retention_policy: None | RetentionPolicyType | Unset = UNSET
    reference_allowed_pack_refs: list[str] | Unset = UNSET
    required_worker_runtimes: (
        ApiResponseActionResponseDataRequiredWorkerRuntimes | Unset
    ) = UNSET
    runtime: int | None | Unset = UNSET
    runtime_ref: None | str | Unset = UNSET
    runtime_version_constraint: None | str | Unset = UNSET
    timeout_seconds: int | None | Unset = UNSET
    worker_affinity: WorkerAffinity | Unset = UNSET
    worker_selector: ApiResponseActionResponseDataWorkerSelector | Unset = UNSET
    worker_tolerations: list[WorkerToleration] | Unset = UNSET
    workflow_def: int | None | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.api_response_action_response_data_out_schema_type_0 import (
            ApiResponseActionResponseDataOutSchemaType0,
        )
        from ..models.api_response_action_response_data_param_schema_type_0 import (
            ApiResponseActionResponseDataParamSchemaType0,
        )

        accesses_mcp = self.accesses_mcp

        created = self.created.isoformat()

        enabled = self.enabled

        entrypoint = self.entrypoint

        id = self.id

        is_adhoc = self.is_adhoc

        label = self.label

        out_schema: dict[str, Any] | None
        if isinstance(self.out_schema, ApiResponseActionResponseDataOutSchemaType0):
            out_schema = self.out_schema.to_dict()
        else:
            out_schema = self.out_schema

        pack = self.pack

        pack_ref = self.pack_ref

        param_schema: dict[str, Any] | None
        if isinstance(self.param_schema, ApiResponseActionResponseDataParamSchemaType0):
            param_schema = self.param_schema.to_dict()
        else:
            param_schema = self.param_schema

        ref = self.ref

        reference_visibility = self.reference_visibility.value

        updated = self.updated.isoformat()

        artifact_retention_limit: int | None | Unset
        if isinstance(self.artifact_retention_limit, Unset):
            artifact_retention_limit = UNSET
        else:
            artifact_retention_limit = self.artifact_retention_limit

        artifact_retention_policy: None | str | Unset
        if isinstance(self.artifact_retention_policy, Unset):
            artifact_retention_policy = UNSET
        elif isinstance(self.artifact_retention_policy, RetentionPolicyType):
            artifact_retention_policy = self.artifact_retention_policy.value
        else:
            artifact_retention_policy = self.artifact_retention_policy

        default_execution_permission_set_refs: list[str] | Unset = UNSET
        if not isinstance(self.default_execution_permission_set_refs, Unset):
            default_execution_permission_set_refs = (
                self.default_execution_permission_set_refs
            )

        description: None | str | Unset
        if isinstance(self.description, Unset):
            description = UNSET
        else:
            description = self.description

        log_retention_limit: int | None | Unset
        if isinstance(self.log_retention_limit, Unset):
            log_retention_limit = UNSET
        else:
            log_retention_limit = self.log_retention_limit

        log_retention_policy: None | str | Unset
        if isinstance(self.log_retention_policy, Unset):
            log_retention_policy = UNSET
        elif isinstance(self.log_retention_policy, RetentionPolicyType):
            log_retention_policy = self.log_retention_policy.value
        else:
            log_retention_policy = self.log_retention_policy

        reference_allowed_pack_refs: list[str] | Unset = UNSET
        if not isinstance(self.reference_allowed_pack_refs, Unset):
            reference_allowed_pack_refs = self.reference_allowed_pack_refs

        required_worker_runtimes: dict[str, Any] | Unset = UNSET
        if not isinstance(self.required_worker_runtimes, Unset):
            required_worker_runtimes = self.required_worker_runtimes.to_dict()

        runtime: int | None | Unset
        if isinstance(self.runtime, Unset):
            runtime = UNSET
        else:
            runtime = self.runtime

        runtime_ref: None | str | Unset
        if isinstance(self.runtime_ref, Unset):
            runtime_ref = UNSET
        else:
            runtime_ref = self.runtime_ref

        runtime_version_constraint: None | str | Unset
        if isinstance(self.runtime_version_constraint, Unset):
            runtime_version_constraint = UNSET
        else:
            runtime_version_constraint = self.runtime_version_constraint

        timeout_seconds: int | None | Unset
        if isinstance(self.timeout_seconds, Unset):
            timeout_seconds = UNSET
        else:
            timeout_seconds = self.timeout_seconds

        worker_affinity: dict[str, Any] | Unset = UNSET
        if not isinstance(self.worker_affinity, Unset):
            worker_affinity = self.worker_affinity.to_dict()

        worker_selector: dict[str, Any] | Unset = UNSET
        if not isinstance(self.worker_selector, Unset):
            worker_selector = self.worker_selector.to_dict()

        worker_tolerations: list[dict[str, Any]] | Unset = UNSET
        if not isinstance(self.worker_tolerations, Unset):
            worker_tolerations = []
            for worker_tolerations_item_data in self.worker_tolerations:
                worker_tolerations_item = worker_tolerations_item_data.to_dict()
                worker_tolerations.append(worker_tolerations_item)

        workflow_def: int | None | Unset
        if isinstance(self.workflow_def, Unset):
            workflow_def = UNSET
        else:
            workflow_def = self.workflow_def

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "accesses_mcp": accesses_mcp,
                "created": created,
                "enabled": enabled,
                "entrypoint": entrypoint,
                "id": id,
                "is_adhoc": is_adhoc,
                "label": label,
                "out_schema": out_schema,
                "pack": pack,
                "pack_ref": pack_ref,
                "param_schema": param_schema,
                "ref": ref,
                "reference_visibility": reference_visibility,
                "updated": updated,
            }
        )
        if artifact_retention_limit is not UNSET:
            field_dict["artifact_retention_limit"] = artifact_retention_limit
        if artifact_retention_policy is not UNSET:
            field_dict["artifact_retention_policy"] = artifact_retention_policy
        if default_execution_permission_set_refs is not UNSET:
            field_dict["default_execution_permission_set_refs"] = (
                default_execution_permission_set_refs
            )
        if description is not UNSET:
            field_dict["description"] = description
        if log_retention_limit is not UNSET:
            field_dict["log_retention_limit"] = log_retention_limit
        if log_retention_policy is not UNSET:
            field_dict["log_retention_policy"] = log_retention_policy
        if reference_allowed_pack_refs is not UNSET:
            field_dict["reference_allowed_pack_refs"] = reference_allowed_pack_refs
        if required_worker_runtimes is not UNSET:
            field_dict["required_worker_runtimes"] = required_worker_runtimes
        if runtime is not UNSET:
            field_dict["runtime"] = runtime
        if runtime_ref is not UNSET:
            field_dict["runtime_ref"] = runtime_ref
        if runtime_version_constraint is not UNSET:
            field_dict["runtime_version_constraint"] = runtime_version_constraint
        if timeout_seconds is not UNSET:
            field_dict["timeout_seconds"] = timeout_seconds
        if worker_affinity is not UNSET:
            field_dict["worker_affinity"] = worker_affinity
        if worker_selector is not UNSET:
            field_dict["worker_selector"] = worker_selector
        if worker_tolerations is not UNSET:
            field_dict["worker_tolerations"] = worker_tolerations
        if workflow_def is not UNSET:
            field_dict["workflow_def"] = workflow_def

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.api_response_action_response_data_out_schema_type_0 import (
            ApiResponseActionResponseDataOutSchemaType0,
        )
        from ..models.api_response_action_response_data_param_schema_type_0 import (
            ApiResponseActionResponseDataParamSchemaType0,
        )
        from ..models.api_response_action_response_data_required_worker_runtimes import (
            ApiResponseActionResponseDataRequiredWorkerRuntimes,
        )
        from ..models.api_response_action_response_data_worker_selector import (
            ApiResponseActionResponseDataWorkerSelector,
        )
        from ..models.worker_affinity import WorkerAffinity
        from ..models.worker_toleration import WorkerToleration

        d = dict(src_dict)
        accesses_mcp = d.pop("accesses_mcp")

        created = datetime.datetime.fromisoformat(d.pop("created"))

        enabled = d.pop("enabled")

        entrypoint = d.pop("entrypoint")

        id = d.pop("id")

        is_adhoc = d.pop("is_adhoc")

        label = d.pop("label")

        def _parse_out_schema(
            data: object,
        ) -> ApiResponseActionResponseDataOutSchemaType0 | None:
            if data is None:
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                out_schema_type_0 = (
                    ApiResponseActionResponseDataOutSchemaType0.from_dict(data)
                )

                return out_schema_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(ApiResponseActionResponseDataOutSchemaType0 | None, data)

        out_schema = _parse_out_schema(d.pop("out_schema"))

        pack = d.pop("pack")

        pack_ref = d.pop("pack_ref")

        def _parse_param_schema(
            data: object,
        ) -> ApiResponseActionResponseDataParamSchemaType0 | None:
            if data is None:
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                param_schema_type_0 = (
                    ApiResponseActionResponseDataParamSchemaType0.from_dict(data)
                )

                return param_schema_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(ApiResponseActionResponseDataParamSchemaType0 | None, data)

        param_schema = _parse_param_schema(d.pop("param_schema"))

        ref = d.pop("ref")

        reference_visibility = ActionReferenceVisibility(d.pop("reference_visibility"))

        updated = datetime.datetime.fromisoformat(d.pop("updated"))

        def _parse_artifact_retention_limit(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        artifact_retention_limit = _parse_artifact_retention_limit(
            d.pop("artifact_retention_limit", UNSET)
        )

        def _parse_artifact_retention_policy(
            data: object,
        ) -> None | RetentionPolicyType | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                artifact_retention_policy_type_1 = RetentionPolicyType(data)

                return artifact_retention_policy_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(None | RetentionPolicyType | Unset, data)

        artifact_retention_policy = _parse_artifact_retention_policy(
            d.pop("artifact_retention_policy", UNSET)
        )

        default_execution_permission_set_refs = cast(
            list[str], d.pop("default_execution_permission_set_refs", UNSET)
        )

        def _parse_description(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        description = _parse_description(d.pop("description", UNSET))

        def _parse_log_retention_limit(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        log_retention_limit = _parse_log_retention_limit(
            d.pop("log_retention_limit", UNSET)
        )

        def _parse_log_retention_policy(
            data: object,
        ) -> None | RetentionPolicyType | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                log_retention_policy_type_1 = RetentionPolicyType(data)

                return log_retention_policy_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(None | RetentionPolicyType | Unset, data)

        log_retention_policy = _parse_log_retention_policy(
            d.pop("log_retention_policy", UNSET)
        )

        reference_allowed_pack_refs = cast(
            list[str], d.pop("reference_allowed_pack_refs", UNSET)
        )

        _required_worker_runtimes = d.pop("required_worker_runtimes", UNSET)
        required_worker_runtimes: (
            ApiResponseActionResponseDataRequiredWorkerRuntimes | Unset
        )
        if isinstance(_required_worker_runtimes, Unset):
            required_worker_runtimes = UNSET
        else:
            required_worker_runtimes = (
                ApiResponseActionResponseDataRequiredWorkerRuntimes.from_dict(
                    _required_worker_runtimes
                )
            )

        def _parse_runtime(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        runtime = _parse_runtime(d.pop("runtime", UNSET))

        def _parse_runtime_ref(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        runtime_ref = _parse_runtime_ref(d.pop("runtime_ref", UNSET))

        def _parse_runtime_version_constraint(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        runtime_version_constraint = _parse_runtime_version_constraint(
            d.pop("runtime_version_constraint", UNSET)
        )

        def _parse_timeout_seconds(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        timeout_seconds = _parse_timeout_seconds(d.pop("timeout_seconds", UNSET))

        _worker_affinity = d.pop("worker_affinity", UNSET)
        worker_affinity: WorkerAffinity | Unset
        if isinstance(_worker_affinity, Unset):
            worker_affinity = UNSET
        else:
            worker_affinity = WorkerAffinity.from_dict(_worker_affinity)

        _worker_selector = d.pop("worker_selector", UNSET)
        worker_selector: ApiResponseActionResponseDataWorkerSelector | Unset
        if isinstance(_worker_selector, Unset):
            worker_selector = UNSET
        else:
            worker_selector = ApiResponseActionResponseDataWorkerSelector.from_dict(
                _worker_selector
            )

        _worker_tolerations = d.pop("worker_tolerations", UNSET)
        worker_tolerations: list[WorkerToleration] | Unset = UNSET
        if _worker_tolerations is not UNSET:
            worker_tolerations = []
            for worker_tolerations_item_data in _worker_tolerations:
                worker_tolerations_item = WorkerToleration.from_dict(
                    worker_tolerations_item_data
                )

                worker_tolerations.append(worker_tolerations_item)

        def _parse_workflow_def(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        workflow_def = _parse_workflow_def(d.pop("workflow_def", UNSET))

        api_response_action_response_data = cls(
            accesses_mcp=accesses_mcp,
            created=created,
            enabled=enabled,
            entrypoint=entrypoint,
            id=id,
            is_adhoc=is_adhoc,
            label=label,
            out_schema=out_schema,
            pack=pack,
            pack_ref=pack_ref,
            param_schema=param_schema,
            ref=ref,
            reference_visibility=reference_visibility,
            updated=updated,
            artifact_retention_limit=artifact_retention_limit,
            artifact_retention_policy=artifact_retention_policy,
            default_execution_permission_set_refs=default_execution_permission_set_refs,
            description=description,
            log_retention_limit=log_retention_limit,
            log_retention_policy=log_retention_policy,
            reference_allowed_pack_refs=reference_allowed_pack_refs,
            required_worker_runtimes=required_worker_runtimes,
            runtime=runtime,
            runtime_ref=runtime_ref,
            runtime_version_constraint=runtime_version_constraint,
            timeout_seconds=timeout_seconds,
            worker_affinity=worker_affinity,
            worker_selector=worker_selector,
            worker_tolerations=worker_tolerations,
            workflow_def=workflow_def,
        )

        api_response_action_response_data.additional_properties = d
        return api_response_action_response_data

    @property
    def additional_keys(self) -> list[str]:
        return list(self.additional_properties.keys())

    def __getitem__(self, key: str) -> Any:
        return self.additional_properties[key]

    def __setitem__(self, key: str, value: Any) -> None:
        self.additional_properties[key] = value

    def __delitem__(self, key: str) -> None:
        del self.additional_properties[key]

    def __contains__(self, key: str) -> bool:
        return key in self.additional_properties
