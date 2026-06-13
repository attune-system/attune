from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.action_reference_visibility import ActionReferenceVisibility
from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.log_retention_limit_patch_type_0 import LogRetentionLimitPatchType0
    from ..models.log_retention_limit_patch_type_1 import LogRetentionLimitPatchType1
    from ..models.log_retention_policy_patch_type_0 import LogRetentionPolicyPatchType0
    from ..models.log_retention_policy_patch_type_1 import LogRetentionPolicyPatchType1
    from ..models.runtime_version_constraint_patch_type_0 import (
        RuntimeVersionConstraintPatchType0,
    )
    from ..models.runtime_version_constraint_patch_type_1 import (
        RuntimeVersionConstraintPatchType1,
    )
    from ..models.timeout_seconds_patch_type_0 import TimeoutSecondsPatchType0
    from ..models.timeout_seconds_patch_type_1 import TimeoutSecondsPatchType1
    from ..models.update_action_request_out_schema_type_0 import (
        UpdateActionRequestOutSchemaType0,
    )
    from ..models.update_action_request_param_schema_type_0 import (
        UpdateActionRequestParamSchemaType0,
    )
    from ..models.update_action_request_required_worker_runtimes_type_0 import (
        UpdateActionRequestRequiredWorkerRuntimesType0,
    )
    from ..models.update_action_request_worker_selector_type_0 import (
        UpdateActionRequestWorkerSelectorType0,
    )
    from ..models.worker_affinity import WorkerAffinity
    from ..models.worker_toleration import WorkerToleration


T = TypeVar("T", bound="UpdateActionRequest")


@_attrs_define
class UpdateActionRequest:
    """Request DTO for updating an action

    Attributes:
        accesses_mcp (bool | None | Unset): Hint that this action may invoke the Attune MCP server and spawn child
            executions.
        artifact_retention_limit (LogRetentionLimitPatchType0 | LogRetentionLimitPatchType1 | None | Unset):
        artifact_retention_policy (LogRetentionPolicyPatchType0 | LogRetentionPolicyPatchType1 | None | Unset):
        default_execution_permission_set_refs (list[str] | None | Unset): Default permission set refs for execution-
            scoped API tokens. Example: ['core.agent_reader'].
        description (None | str | Unset): Action description Example: Posts a message to a Slack channel with enhanced
            features.
        enabled (bool | None | Unset): Whether this action is enabled. Example: True.
        entrypoint (None | str | Unset): Entry point for action execution Example: /actions/slack/post_message_v2.py.
        label (None | str | Unset): Human-readable label Example: Post Message to Slack (Updated).
        log_retention_limit (LogRetentionLimitPatchType0 | LogRetentionLimitPatchType1 | None | Unset):
        log_retention_policy (LogRetentionPolicyPatchType0 | LogRetentionPolicyPatchType1 | None | Unset):
        out_schema (None | Unset | UpdateActionRequestOutSchemaType0): Output schema
        param_schema (None | Unset | UpdateActionRequestParamSchemaType0): Parameter schema (StackStorm-style with
            inline required/secret)
        reference_allowed_pack_refs (list[str] | None | Unset): Replace the restricted visibility allow-list. Example:
            ['incident_response', 'deployments'].
        reference_visibility (ActionReferenceVisibility | None | Unset):
        required_worker_runtimes (None | Unset | UpdateActionRequestRequiredWorkerRuntimesType0): Additional worker
            runtime requirements keyed by runtime name/alias. Use "*" for any available version.
        runtime (int | None | Unset): Runtime ID Example: 1.
        runtime_ref (None | str | Unset): Runtime reference Example: core.python.
        runtime_version_constraint (None | RuntimeVersionConstraintPatchType0 | RuntimeVersionConstraintPatchType1 |
            Unset):
        timeout_seconds (None | TimeoutSecondsPatchType0 | TimeoutSecondsPatchType1 | Unset):
        worker_affinity (None | Unset | WorkerAffinity):
        worker_selector (None | Unset | UpdateActionRequestWorkerSelectorType0): Exact worker label requirements. All
            labels must match the selected worker.
        worker_tolerations (list[WorkerToleration] | None | Unset): Tolerations that allow scheduling onto workers with
            matching taints.
    """

    accesses_mcp: bool | None | Unset = UNSET
    artifact_retention_limit: (
        LogRetentionLimitPatchType0 | LogRetentionLimitPatchType1 | None | Unset
    ) = UNSET
    artifact_retention_policy: (
        LogRetentionPolicyPatchType0 | LogRetentionPolicyPatchType1 | None | Unset
    ) = UNSET
    default_execution_permission_set_refs: list[str] | None | Unset = UNSET
    description: None | str | Unset = UNSET
    enabled: bool | None | Unset = UNSET
    entrypoint: None | str | Unset = UNSET
    label: None | str | Unset = UNSET
    log_retention_limit: (
        LogRetentionLimitPatchType0 | LogRetentionLimitPatchType1 | None | Unset
    ) = UNSET
    log_retention_policy: (
        LogRetentionPolicyPatchType0 | LogRetentionPolicyPatchType1 | None | Unset
    ) = UNSET
    out_schema: None | Unset | UpdateActionRequestOutSchemaType0 = UNSET
    param_schema: None | Unset | UpdateActionRequestParamSchemaType0 = UNSET
    reference_allowed_pack_refs: list[str] | None | Unset = UNSET
    reference_visibility: ActionReferenceVisibility | None | Unset = UNSET
    required_worker_runtimes: (
        None | Unset | UpdateActionRequestRequiredWorkerRuntimesType0
    ) = UNSET
    runtime: int | None | Unset = UNSET
    runtime_ref: None | str | Unset = UNSET
    runtime_version_constraint: (
        None
        | RuntimeVersionConstraintPatchType0
        | RuntimeVersionConstraintPatchType1
        | Unset
    ) = UNSET
    timeout_seconds: (
        None | TimeoutSecondsPatchType0 | TimeoutSecondsPatchType1 | Unset
    ) = UNSET
    worker_affinity: None | Unset | WorkerAffinity = UNSET
    worker_selector: None | Unset | UpdateActionRequestWorkerSelectorType0 = UNSET
    worker_tolerations: list[WorkerToleration] | None | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.log_retention_limit_patch_type_0 import (
            LogRetentionLimitPatchType0,
        )
        from ..models.log_retention_limit_patch_type_1 import (
            LogRetentionLimitPatchType1,
        )
        from ..models.log_retention_policy_patch_type_0 import (
            LogRetentionPolicyPatchType0,
        )
        from ..models.log_retention_policy_patch_type_1 import (
            LogRetentionPolicyPatchType1,
        )
        from ..models.runtime_version_constraint_patch_type_0 import (
            RuntimeVersionConstraintPatchType0,
        )
        from ..models.runtime_version_constraint_patch_type_1 import (
            RuntimeVersionConstraintPatchType1,
        )
        from ..models.timeout_seconds_patch_type_0 import TimeoutSecondsPatchType0
        from ..models.timeout_seconds_patch_type_1 import TimeoutSecondsPatchType1
        from ..models.update_action_request_out_schema_type_0 import (
            UpdateActionRequestOutSchemaType0,
        )
        from ..models.update_action_request_param_schema_type_0 import (
            UpdateActionRequestParamSchemaType0,
        )
        from ..models.update_action_request_required_worker_runtimes_type_0 import (
            UpdateActionRequestRequiredWorkerRuntimesType0,
        )
        from ..models.update_action_request_worker_selector_type_0 import (
            UpdateActionRequestWorkerSelectorType0,
        )
        from ..models.worker_affinity import WorkerAffinity

        accesses_mcp: bool | None | Unset
        if isinstance(self.accesses_mcp, Unset):
            accesses_mcp = UNSET
        else:
            accesses_mcp = self.accesses_mcp

        artifact_retention_limit: dict[str, Any] | None | Unset
        if isinstance(self.artifact_retention_limit, Unset):
            artifact_retention_limit = UNSET
        elif isinstance(self.artifact_retention_limit, LogRetentionLimitPatchType0):
            artifact_retention_limit = self.artifact_retention_limit.to_dict()
        elif isinstance(self.artifact_retention_limit, LogRetentionLimitPatchType1):
            artifact_retention_limit = self.artifact_retention_limit.to_dict()
        else:
            artifact_retention_limit = self.artifact_retention_limit

        artifact_retention_policy: dict[str, Any] | None | Unset
        if isinstance(self.artifact_retention_policy, Unset):
            artifact_retention_policy = UNSET
        elif isinstance(self.artifact_retention_policy, LogRetentionPolicyPatchType0):
            artifact_retention_policy = self.artifact_retention_policy.to_dict()
        elif isinstance(self.artifact_retention_policy, LogRetentionPolicyPatchType1):
            artifact_retention_policy = self.artifact_retention_policy.to_dict()
        else:
            artifact_retention_policy = self.artifact_retention_policy

        default_execution_permission_set_refs: list[str] | None | Unset
        if isinstance(self.default_execution_permission_set_refs, Unset):
            default_execution_permission_set_refs = UNSET
        elif isinstance(self.default_execution_permission_set_refs, list):
            default_execution_permission_set_refs = (
                self.default_execution_permission_set_refs
            )

        else:
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

        entrypoint: None | str | Unset
        if isinstance(self.entrypoint, Unset):
            entrypoint = UNSET
        else:
            entrypoint = self.entrypoint

        label: None | str | Unset
        if isinstance(self.label, Unset):
            label = UNSET
        else:
            label = self.label

        log_retention_limit: dict[str, Any] | None | Unset
        if isinstance(self.log_retention_limit, Unset):
            log_retention_limit = UNSET
        elif isinstance(self.log_retention_limit, LogRetentionLimitPatchType0):
            log_retention_limit = self.log_retention_limit.to_dict()
        elif isinstance(self.log_retention_limit, LogRetentionLimitPatchType1):
            log_retention_limit = self.log_retention_limit.to_dict()
        else:
            log_retention_limit = self.log_retention_limit

        log_retention_policy: dict[str, Any] | None | Unset
        if isinstance(self.log_retention_policy, Unset):
            log_retention_policy = UNSET
        elif isinstance(self.log_retention_policy, LogRetentionPolicyPatchType0):
            log_retention_policy = self.log_retention_policy.to_dict()
        elif isinstance(self.log_retention_policy, LogRetentionPolicyPatchType1):
            log_retention_policy = self.log_retention_policy.to_dict()
        else:
            log_retention_policy = self.log_retention_policy

        out_schema: dict[str, Any] | None | Unset
        if isinstance(self.out_schema, Unset):
            out_schema = UNSET
        elif isinstance(self.out_schema, UpdateActionRequestOutSchemaType0):
            out_schema = self.out_schema.to_dict()
        else:
            out_schema = self.out_schema

        param_schema: dict[str, Any] | None | Unset
        if isinstance(self.param_schema, Unset):
            param_schema = UNSET
        elif isinstance(self.param_schema, UpdateActionRequestParamSchemaType0):
            param_schema = self.param_schema.to_dict()
        else:
            param_schema = self.param_schema

        reference_allowed_pack_refs: list[str] | None | Unset
        if isinstance(self.reference_allowed_pack_refs, Unset):
            reference_allowed_pack_refs = UNSET
        elif isinstance(self.reference_allowed_pack_refs, list):
            reference_allowed_pack_refs = self.reference_allowed_pack_refs

        else:
            reference_allowed_pack_refs = self.reference_allowed_pack_refs

        reference_visibility: None | str | Unset
        if isinstance(self.reference_visibility, Unset):
            reference_visibility = UNSET
        elif isinstance(self.reference_visibility, ActionReferenceVisibility):
            reference_visibility = self.reference_visibility.value
        else:
            reference_visibility = self.reference_visibility

        required_worker_runtimes: dict[str, Any] | None | Unset
        if isinstance(self.required_worker_runtimes, Unset):
            required_worker_runtimes = UNSET
        elif isinstance(
            self.required_worker_runtimes,
            UpdateActionRequestRequiredWorkerRuntimesType0,
        ):
            required_worker_runtimes = self.required_worker_runtimes.to_dict()
        else:
            required_worker_runtimes = self.required_worker_runtimes

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

        runtime_version_constraint: dict[str, Any] | None | Unset
        if isinstance(self.runtime_version_constraint, Unset):
            runtime_version_constraint = UNSET
        elif isinstance(
            self.runtime_version_constraint, RuntimeVersionConstraintPatchType0
        ):
            runtime_version_constraint = self.runtime_version_constraint.to_dict()
        elif isinstance(
            self.runtime_version_constraint, RuntimeVersionConstraintPatchType1
        ):
            runtime_version_constraint = self.runtime_version_constraint.to_dict()
        else:
            runtime_version_constraint = self.runtime_version_constraint

        timeout_seconds: dict[str, Any] | None | Unset
        if isinstance(self.timeout_seconds, Unset):
            timeout_seconds = UNSET
        elif isinstance(self.timeout_seconds, TimeoutSecondsPatchType0):
            timeout_seconds = self.timeout_seconds.to_dict()
        elif isinstance(self.timeout_seconds, TimeoutSecondsPatchType1):
            timeout_seconds = self.timeout_seconds.to_dict()
        else:
            timeout_seconds = self.timeout_seconds

        worker_affinity: dict[str, Any] | None | Unset
        if isinstance(self.worker_affinity, Unset):
            worker_affinity = UNSET
        elif isinstance(self.worker_affinity, WorkerAffinity):
            worker_affinity = self.worker_affinity.to_dict()
        else:
            worker_affinity = self.worker_affinity

        worker_selector: dict[str, Any] | None | Unset
        if isinstance(self.worker_selector, Unset):
            worker_selector = UNSET
        elif isinstance(self.worker_selector, UpdateActionRequestWorkerSelectorType0):
            worker_selector = self.worker_selector.to_dict()
        else:
            worker_selector = self.worker_selector

        worker_tolerations: list[dict[str, Any]] | None | Unset
        if isinstance(self.worker_tolerations, Unset):
            worker_tolerations = UNSET
        elif isinstance(self.worker_tolerations, list):
            worker_tolerations = []
            for worker_tolerations_type_0_item_data in self.worker_tolerations:
                worker_tolerations_type_0_item = (
                    worker_tolerations_type_0_item_data.to_dict()
                )
                worker_tolerations.append(worker_tolerations_type_0_item)

        else:
            worker_tolerations = self.worker_tolerations

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update({})
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
        if entrypoint is not UNSET:
            field_dict["entrypoint"] = entrypoint
        if label is not UNSET:
            field_dict["label"] = label
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
        from ..models.log_retention_limit_patch_type_0 import (
            LogRetentionLimitPatchType0,
        )
        from ..models.log_retention_limit_patch_type_1 import (
            LogRetentionLimitPatchType1,
        )
        from ..models.log_retention_policy_patch_type_0 import (
            LogRetentionPolicyPatchType0,
        )
        from ..models.log_retention_policy_patch_type_1 import (
            LogRetentionPolicyPatchType1,
        )
        from ..models.runtime_version_constraint_patch_type_0 import (
            RuntimeVersionConstraintPatchType0,
        )
        from ..models.runtime_version_constraint_patch_type_1 import (
            RuntimeVersionConstraintPatchType1,
        )
        from ..models.timeout_seconds_patch_type_0 import TimeoutSecondsPatchType0
        from ..models.timeout_seconds_patch_type_1 import TimeoutSecondsPatchType1
        from ..models.update_action_request_out_schema_type_0 import (
            UpdateActionRequestOutSchemaType0,
        )
        from ..models.update_action_request_param_schema_type_0 import (
            UpdateActionRequestParamSchemaType0,
        )
        from ..models.update_action_request_required_worker_runtimes_type_0 import (
            UpdateActionRequestRequiredWorkerRuntimesType0,
        )
        from ..models.update_action_request_worker_selector_type_0 import (
            UpdateActionRequestWorkerSelectorType0,
        )
        from ..models.worker_affinity import WorkerAffinity
        from ..models.worker_toleration import WorkerToleration

        d = dict(src_dict)

        def _parse_accesses_mcp(data: object) -> bool | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(bool | None | Unset, data)

        accesses_mcp = _parse_accesses_mcp(d.pop("accesses_mcp", UNSET))

        def _parse_artifact_retention_limit(
            data: object,
        ) -> LogRetentionLimitPatchType0 | LogRetentionLimitPatchType1 | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_log_retention_limit_patch_type_0 = (
                    LogRetentionLimitPatchType0.from_dict(data)
                )

                return componentsschemas_log_retention_limit_patch_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_log_retention_limit_patch_type_1 = (
                    LogRetentionLimitPatchType1.from_dict(data)
                )

                return componentsschemas_log_retention_limit_patch_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(
                LogRetentionLimitPatchType0
                | LogRetentionLimitPatchType1
                | None
                | Unset,
                data,
            )

        artifact_retention_limit = _parse_artifact_retention_limit(
            d.pop("artifact_retention_limit", UNSET)
        )

        def _parse_artifact_retention_policy(
            data: object,
        ) -> LogRetentionPolicyPatchType0 | LogRetentionPolicyPatchType1 | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_log_retention_policy_patch_type_0 = (
                    LogRetentionPolicyPatchType0.from_dict(data)
                )

                return componentsschemas_log_retention_policy_patch_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_log_retention_policy_patch_type_1 = (
                    LogRetentionPolicyPatchType1.from_dict(data)
                )

                return componentsschemas_log_retention_policy_patch_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(
                LogRetentionPolicyPatchType0
                | LogRetentionPolicyPatchType1
                | None
                | Unset,
                data,
            )

        artifact_retention_policy = _parse_artifact_retention_policy(
            d.pop("artifact_retention_policy", UNSET)
        )

        def _parse_default_execution_permission_set_refs(
            data: object,
        ) -> list[str] | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, list):
                    raise TypeError()
                default_execution_permission_set_refs_type_0 = cast(list[str], data)

                return default_execution_permission_set_refs_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(list[str] | None | Unset, data)

        default_execution_permission_set_refs = (
            _parse_default_execution_permission_set_refs(
                d.pop("default_execution_permission_set_refs", UNSET)
            )
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

        def _parse_entrypoint(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        entrypoint = _parse_entrypoint(d.pop("entrypoint", UNSET))

        def _parse_label(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        label = _parse_label(d.pop("label", UNSET))

        def _parse_log_retention_limit(
            data: object,
        ) -> LogRetentionLimitPatchType0 | LogRetentionLimitPatchType1 | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_log_retention_limit_patch_type_0 = (
                    LogRetentionLimitPatchType0.from_dict(data)
                )

                return componentsschemas_log_retention_limit_patch_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_log_retention_limit_patch_type_1 = (
                    LogRetentionLimitPatchType1.from_dict(data)
                )

                return componentsschemas_log_retention_limit_patch_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(
                LogRetentionLimitPatchType0
                | LogRetentionLimitPatchType1
                | None
                | Unset,
                data,
            )

        log_retention_limit = _parse_log_retention_limit(
            d.pop("log_retention_limit", UNSET)
        )

        def _parse_log_retention_policy(
            data: object,
        ) -> LogRetentionPolicyPatchType0 | LogRetentionPolicyPatchType1 | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_log_retention_policy_patch_type_0 = (
                    LogRetentionPolicyPatchType0.from_dict(data)
                )

                return componentsschemas_log_retention_policy_patch_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_log_retention_policy_patch_type_1 = (
                    LogRetentionPolicyPatchType1.from_dict(data)
                )

                return componentsschemas_log_retention_policy_patch_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(
                LogRetentionPolicyPatchType0
                | LogRetentionPolicyPatchType1
                | None
                | Unset,
                data,
            )

        log_retention_policy = _parse_log_retention_policy(
            d.pop("log_retention_policy", UNSET)
        )

        def _parse_out_schema(
            data: object,
        ) -> None | Unset | UpdateActionRequestOutSchemaType0:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                out_schema_type_0 = UpdateActionRequestOutSchemaType0.from_dict(data)

                return out_schema_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(None | Unset | UpdateActionRequestOutSchemaType0, data)

        out_schema = _parse_out_schema(d.pop("out_schema", UNSET))

        def _parse_param_schema(
            data: object,
        ) -> None | Unset | UpdateActionRequestParamSchemaType0:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                param_schema_type_0 = UpdateActionRequestParamSchemaType0.from_dict(
                    data
                )

                return param_schema_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(None | Unset | UpdateActionRequestParamSchemaType0, data)

        param_schema = _parse_param_schema(d.pop("param_schema", UNSET))

        def _parse_reference_allowed_pack_refs(
            data: object,
        ) -> list[str] | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, list):
                    raise TypeError()
                reference_allowed_pack_refs_type_0 = cast(list[str], data)

                return reference_allowed_pack_refs_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(list[str] | None | Unset, data)

        reference_allowed_pack_refs = _parse_reference_allowed_pack_refs(
            d.pop("reference_allowed_pack_refs", UNSET)
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

        def _parse_required_worker_runtimes(
            data: object,
        ) -> None | Unset | UpdateActionRequestRequiredWorkerRuntimesType0:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                required_worker_runtimes_type_0 = (
                    UpdateActionRequestRequiredWorkerRuntimesType0.from_dict(data)
                )

                return required_worker_runtimes_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(
                None | Unset | UpdateActionRequestRequiredWorkerRuntimesType0, data
            )

        required_worker_runtimes = _parse_required_worker_runtimes(
            d.pop("required_worker_runtimes", UNSET)
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

        def _parse_runtime_version_constraint(
            data: object,
        ) -> (
            None
            | RuntimeVersionConstraintPatchType0
            | RuntimeVersionConstraintPatchType1
            | Unset
        ):
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_runtime_version_constraint_patch_type_0 = (
                    RuntimeVersionConstraintPatchType0.from_dict(data)
                )

                return componentsschemas_runtime_version_constraint_patch_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_runtime_version_constraint_patch_type_1 = (
                    RuntimeVersionConstraintPatchType1.from_dict(data)
                )

                return componentsschemas_runtime_version_constraint_patch_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(
                None
                | RuntimeVersionConstraintPatchType0
                | RuntimeVersionConstraintPatchType1
                | Unset,
                data,
            )

        runtime_version_constraint = _parse_runtime_version_constraint(
            d.pop("runtime_version_constraint", UNSET)
        )

        def _parse_timeout_seconds(
            data: object,
        ) -> None | TimeoutSecondsPatchType0 | TimeoutSecondsPatchType1 | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_timeout_seconds_patch_type_0 = (
                    TimeoutSecondsPatchType0.from_dict(data)
                )

                return componentsschemas_timeout_seconds_patch_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_timeout_seconds_patch_type_1 = (
                    TimeoutSecondsPatchType1.from_dict(data)
                )

                return componentsschemas_timeout_seconds_patch_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(
                None | TimeoutSecondsPatchType0 | TimeoutSecondsPatchType1 | Unset, data
            )

        timeout_seconds = _parse_timeout_seconds(d.pop("timeout_seconds", UNSET))

        def _parse_worker_affinity(data: object) -> None | Unset | WorkerAffinity:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                worker_affinity_type_1 = WorkerAffinity.from_dict(data)

                return worker_affinity_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(None | Unset | WorkerAffinity, data)

        worker_affinity = _parse_worker_affinity(d.pop("worker_affinity", UNSET))

        def _parse_worker_selector(
            data: object,
        ) -> None | Unset | UpdateActionRequestWorkerSelectorType0:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                worker_selector_type_0 = (
                    UpdateActionRequestWorkerSelectorType0.from_dict(data)
                )

                return worker_selector_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(None | Unset | UpdateActionRequestWorkerSelectorType0, data)

        worker_selector = _parse_worker_selector(d.pop("worker_selector", UNSET))

        def _parse_worker_tolerations(
            data: object,
        ) -> list[WorkerToleration] | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, list):
                    raise TypeError()
                worker_tolerations_type_0 = []
                _worker_tolerations_type_0 = data
                for worker_tolerations_type_0_item_data in _worker_tolerations_type_0:
                    worker_tolerations_type_0_item = WorkerToleration.from_dict(
                        worker_tolerations_type_0_item_data
                    )

                    worker_tolerations_type_0.append(worker_tolerations_type_0_item)

                return worker_tolerations_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(list[WorkerToleration] | None | Unset, data)

        worker_tolerations = _parse_worker_tolerations(
            d.pop("worker_tolerations", UNSET)
        )

        update_action_request = cls(
            accesses_mcp=accesses_mcp,
            artifact_retention_limit=artifact_retention_limit,
            artifact_retention_policy=artifact_retention_policy,
            default_execution_permission_set_refs=default_execution_permission_set_refs,
            description=description,
            enabled=enabled,
            entrypoint=entrypoint,
            label=label,
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

        update_action_request.additional_properties = d
        return update_action_request

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
