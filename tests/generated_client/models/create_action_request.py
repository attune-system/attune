from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.action_reference_visibility import ActionReferenceVisibility
from ..models.retention_policy_type import RetentionPolicyType
from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.create_action_request_out_schema_type_0 import (
        CreateActionRequestOutSchemaType0,
    )
    from ..models.create_action_request_param_schema_type_0 import (
        CreateActionRequestParamSchemaType0,
    )
    from ..models.create_action_request_required_worker_runtimes import (
        CreateActionRequestRequiredWorkerRuntimes,
    )
    from ..models.create_action_request_worker_selector import (
        CreateActionRequestWorkerSelector,
    )
    from ..models.worker_affinity import WorkerAffinity
    from ..models.worker_toleration import WorkerToleration


T = TypeVar("T", bound="CreateActionRequest")


@_attrs_define
class CreateActionRequest:
    """Request DTO for creating a new action

    Attributes:
        entrypoint (str): Entry point for action execution (e.g., path to script, function name) Example:
            /actions/slack/post_message.py.
        label (str): Human-readable label Example: Post Message to Slack.
        pack_ref (str): Pack reference this action belongs to Example: slack.
        ref (str): Unique reference identifier (e.g., "core.http", "aws.ec2.start_instance") Example:
            slack.post_message.
        accesses_mcp (bool | None | Unset): Hint that this action may invoke the Attune MCP server and spawn child
            executions.
            When true, consumers (UI, CLI, timeline charts) render subtask views eagerly. Default: False.
        artifact_retention_limit (int | None | Unset): Optional per-action retention limit override for non-log
            artifacts created by executions. Example: 10.
        artifact_retention_policy (None | RetentionPolicyType | Unset):
        default_execution_permission_set_refs (list[str] | Unset): Default permission set refs for execution-scoped API
            tokens.
            Empty or omitted means executions of this action receive no API token by default. Example:
            ['core.agent_reader'].
        description (None | str | Unset): Action description Example: Posts a message to a Slack channel.
        enabled (bool | None | Unset): Whether this action is enabled. Omitted defaults to true. Default: True. Example:
            True.
        log_retention_limit (int | None | Unset): Optional per-action retention limit override for stdout/stderr
            execution log artifacts. Example: 4.
        log_retention_policy (None | RetentionPolicyType | Unset):
        out_schema (CreateActionRequestOutSchemaType0 | None | Unset): Output schema (flat format) defining expected
            outputs with inline required/secret
        param_schema (CreateActionRequestParamSchemaType0 | None | Unset): Parameter schema (StackStorm-style) defining
            expected inputs with inline required/secret
        reference_allowed_pack_refs (list[str] | Unset): Pack refs allowed to reference this action when visibility is
            restricted. Example: ['incident_response', 'deployments'].
        reference_visibility (ActionReferenceVisibility | None | Unset):  Default: ActionReferenceVisibility.PUBLIC.
        required_worker_runtimes (CreateActionRequestRequiredWorkerRuntimes | Unset): Additional worker runtime
            requirements keyed by runtime name/alias. Use "*" for any available version.
        runtime (int | None | Unset): Optional runtime ID for this action Example: 1.
        runtime_ref (None | str | Unset): Optional runtime reference for this action Example: core.python.
        runtime_version_constraint (None | str | Unset): Optional semver version constraint for the runtime (e.g.,
            ">=3.12", ">=3.12,<4.0", "~18.0") Example: >=3.12.
        timeout_seconds (int | None | Unset): Optional default execution timeout in seconds for executions of this
            action.
            When omitted, executions fall back to the app-level
            `default_execution_timeout_seconds`. Example: 300.
        worker_affinity (WorkerAffinity | Unset):
        worker_selector (CreateActionRequestWorkerSelector | Unset): Exact worker label requirements. All labels must
            match the selected worker.
        worker_tolerations (list[WorkerToleration] | Unset): Tolerations that allow scheduling onto workers with
            matching taints.
    """

    entrypoint: str
    label: str
    pack_ref: str
    ref: str
    accesses_mcp: bool | None | Unset = False
    artifact_retention_limit: int | None | Unset = UNSET
    artifact_retention_policy: None | RetentionPolicyType | Unset = UNSET
    default_execution_permission_set_refs: list[str] | Unset = UNSET
    description: None | str | Unset = UNSET
    enabled: bool | None | Unset = True
    log_retention_limit: int | None | Unset = UNSET
    log_retention_policy: None | RetentionPolicyType | Unset = UNSET
    out_schema: CreateActionRequestOutSchemaType0 | None | Unset = UNSET
    param_schema: CreateActionRequestParamSchemaType0 | None | Unset = UNSET
    reference_allowed_pack_refs: list[str] | Unset = UNSET
    reference_visibility: ActionReferenceVisibility | None | Unset = (
        ActionReferenceVisibility.PUBLIC
    )
    required_worker_runtimes: CreateActionRequestRequiredWorkerRuntimes | Unset = UNSET
    runtime: int | None | Unset = UNSET
    runtime_ref: None | str | Unset = UNSET
    runtime_version_constraint: None | str | Unset = UNSET
    timeout_seconds: int | None | Unset = UNSET
    worker_affinity: WorkerAffinity | Unset = UNSET
    worker_selector: CreateActionRequestWorkerSelector | Unset = UNSET
    worker_tolerations: list[WorkerToleration] | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.create_action_request_out_schema_type_0 import (
            CreateActionRequestOutSchemaType0,
        )
        from ..models.create_action_request_param_schema_type_0 import (
            CreateActionRequestParamSchemaType0,
        )

        entrypoint = self.entrypoint

        label = self.label

        pack_ref = self.pack_ref

        ref = self.ref

        accesses_mcp: bool | None | Unset
        if isinstance(self.accesses_mcp, Unset):
            accesses_mcp = UNSET
        else:
            accesses_mcp = self.accesses_mcp

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

        enabled: bool | None | Unset
        if isinstance(self.enabled, Unset):
            enabled = UNSET
        else:
            enabled = self.enabled

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

        out_schema: dict[str, Any] | None | Unset
        if isinstance(self.out_schema, Unset):
            out_schema = UNSET
        elif isinstance(self.out_schema, CreateActionRequestOutSchemaType0):
            out_schema = self.out_schema.to_dict()
        else:
            out_schema = self.out_schema

        param_schema: dict[str, Any] | None | Unset
        if isinstance(self.param_schema, Unset):
            param_schema = UNSET
        elif isinstance(self.param_schema, CreateActionRequestParamSchemaType0):
            param_schema = self.param_schema.to_dict()
        else:
            param_schema = self.param_schema

        reference_allowed_pack_refs: list[str] | Unset = UNSET
        if not isinstance(self.reference_allowed_pack_refs, Unset):
            reference_allowed_pack_refs = self.reference_allowed_pack_refs

        reference_visibility: None | str | Unset
        if isinstance(self.reference_visibility, Unset):
            reference_visibility = UNSET
        elif isinstance(self.reference_visibility, ActionReferenceVisibility):
            reference_visibility = self.reference_visibility.value
        else:
            reference_visibility = self.reference_visibility

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

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "entrypoint": entrypoint,
                "label": label,
                "pack_ref": pack_ref,
                "ref": ref,
            }
        )
        if accesses_mcp is not UNSET:
            field_dict["accesses_mcp"] = accesses_mcp
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
        if enabled is not UNSET:
            field_dict["enabled"] = enabled
        if log_retention_limit is not UNSET:
            field_dict["log_retention_limit"] = log_retention_limit
        if log_retention_policy is not UNSET:
            field_dict["log_retention_policy"] = log_retention_policy
        if out_schema is not UNSET:
            field_dict["out_schema"] = out_schema
        if param_schema is not UNSET:
            field_dict["param_schema"] = param_schema
        if reference_allowed_pack_refs is not UNSET:
            field_dict["reference_allowed_pack_refs"] = reference_allowed_pack_refs
        if reference_visibility is not UNSET:
            field_dict["reference_visibility"] = reference_visibility
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

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.create_action_request_out_schema_type_0 import (
            CreateActionRequestOutSchemaType0,
        )
        from ..models.create_action_request_param_schema_type_0 import (
            CreateActionRequestParamSchemaType0,
        )
        from ..models.create_action_request_required_worker_runtimes import (
            CreateActionRequestRequiredWorkerRuntimes,
        )
        from ..models.create_action_request_worker_selector import (
            CreateActionRequestWorkerSelector,
        )
        from ..models.worker_affinity import WorkerAffinity
        from ..models.worker_toleration import WorkerToleration

        d = dict(src_dict)
        entrypoint = d.pop("entrypoint")

        label = d.pop("label")

        pack_ref = d.pop("pack_ref")

        ref = d.pop("ref")

        def _parse_accesses_mcp(data: object) -> bool | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(bool | None | Unset, data)

        accesses_mcp = _parse_accesses_mcp(d.pop("accesses_mcp", UNSET))

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

        def _parse_enabled(data: object) -> bool | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(bool | None | Unset, data)

        enabled = _parse_enabled(d.pop("enabled", UNSET))

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

        def _parse_out_schema(
            data: object,
        ) -> CreateActionRequestOutSchemaType0 | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                out_schema_type_0 = CreateActionRequestOutSchemaType0.from_dict(data)

                return out_schema_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(CreateActionRequestOutSchemaType0 | None | Unset, data)

        out_schema = _parse_out_schema(d.pop("out_schema", UNSET))

        def _parse_param_schema(
            data: object,
        ) -> CreateActionRequestParamSchemaType0 | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                param_schema_type_0 = CreateActionRequestParamSchemaType0.from_dict(
                    data
                )

                return param_schema_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(CreateActionRequestParamSchemaType0 | None | Unset, data)

        param_schema = _parse_param_schema(d.pop("param_schema", UNSET))

        reference_allowed_pack_refs = cast(
            list[str], d.pop("reference_allowed_pack_refs", UNSET)
        )

        def _parse_reference_visibility(
            data: object,
        ) -> ActionReferenceVisibility | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                reference_visibility_type_1 = ActionReferenceVisibility(data)

                return reference_visibility_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(ActionReferenceVisibility | None | Unset, data)

        reference_visibility = _parse_reference_visibility(
            d.pop("reference_visibility", UNSET)
        )

        _required_worker_runtimes = d.pop("required_worker_runtimes", UNSET)
        required_worker_runtimes: CreateActionRequestRequiredWorkerRuntimes | Unset
        if isinstance(_required_worker_runtimes, Unset):
            required_worker_runtimes = UNSET
        else:
            required_worker_runtimes = (
                CreateActionRequestRequiredWorkerRuntimes.from_dict(
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
        worker_selector: CreateActionRequestWorkerSelector | Unset
        if isinstance(_worker_selector, Unset):
            worker_selector = UNSET
        else:
            worker_selector = CreateActionRequestWorkerSelector.from_dict(
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

        create_action_request = cls(
            entrypoint=entrypoint,
            label=label,
            pack_ref=pack_ref,
            ref=ref,
            accesses_mcp=accesses_mcp,
            artifact_retention_limit=artifact_retention_limit,
            artifact_retention_policy=artifact_retention_policy,
            default_execution_permission_set_refs=default_execution_permission_set_refs,
            description=description,
            enabled=enabled,
            log_retention_limit=log_retention_limit,
            log_retention_policy=log_retention_policy,
            out_schema=out_schema,
            param_schema=param_schema,
            reference_allowed_pack_refs=reference_allowed_pack_refs,
            reference_visibility=reference_visibility,
            required_worker_runtimes=required_worker_runtimes,
            runtime=runtime,
            runtime_ref=runtime_ref,
            runtime_version_constraint=runtime_version_constraint,
            timeout_seconds=timeout_seconds,
            worker_affinity=worker_affinity,
            worker_selector=worker_selector,
            worker_tolerations=worker_tolerations,
        )

        create_action_request.additional_properties = d
        return create_action_request

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
