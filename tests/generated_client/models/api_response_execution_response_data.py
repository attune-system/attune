from __future__ import annotations

import datetime
from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.execution_status import ExecutionStatus
from ..models.retention_policy_type import RetentionPolicyType
from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.api_response_execution_response_data_config import (
        ApiResponseExecutionResponseDataConfig,
    )
    from ..models.api_response_execution_response_data_result import (
        ApiResponseExecutionResponseDataResult,
    )
    from ..models.api_response_execution_response_data_worker_affinity_type_0 import (
        ApiResponseExecutionResponseDataWorkerAffinityType0,
    )
    from ..models.api_response_execution_response_data_worker_selector_type_0 import (
        ApiResponseExecutionResponseDataWorkerSelectorType0,
    )
    from ..models.api_response_execution_response_data_worker_tolerations_type_0_item import (
        ApiResponseExecutionResponseDataWorkerTolerationsType0Item,
    )
    from ..models.api_response_execution_response_data_workflow_task_type_0 import (
        ApiResponseExecutionResponseDataWorkflowTaskType0,
    )


T = TypeVar("T", bound="ApiResponseExecutionResponseData")


@_attrs_define
class ApiResponseExecutionResponseData:
    """Response DTO for execution information

    Attributes:
        action_ref (str): Action reference Example: slack.post_message.
        config (ApiResponseExecutionResponseDataConfig): Execution configuration/parameters
        created (datetime.datetime): Creation timestamp Example: 2024-01-13T10:30:00Z.
        id (int): Execution ID Example: 1.
        result (ApiResponseExecutionResponseDataResult): Execution result/output
        status (ExecutionStatus):
        updated (datetime.datetime): Last update timestamp Example: 2024-01-13T10:35:00Z.
        action (int | None | Unset): Action ID (optional, may be null for ad-hoc executions) Example: 1.
        artifact_retention_limit (int | None | Unset): Retention limit override for non-log artifacts created by this
            execution. Example: 10.
        artifact_retention_policy (None | RetentionPolicyType | Unset):
        enforcement (int | None | Unset): Enforcement ID (rule enforcement that triggered this) Example: 1.
        executor (int | None | Unset): Identity ID that initiated this execution Example: 1.
        original_execution (int | None | Unset): ID of the original execution if this execution is a retry. Example: 1.
        parent (int | None | Unset): Parent execution ID (for nested/child executions) Example: 1.
        permission_set_refs (list[str] | Unset): Permission set refs embedded in the execution-scoped API token.
            Example: ['core.agent_reader'].
        started_at (datetime.datetime | None | Unset): When the execution actually started running (worker picked it
            up).
            Null if the execution hasn't started running yet. Example: 2024-01-13T10:31:00Z.
        timeout_seconds (int | None | Unset): Resolved execution timeout in seconds, snapshotted at creation time.
            Example: 600.
        worker (int | None | Unset): Worker ID currently assigned to this execution Example: 1.
        worker_affinity (ApiResponseExecutionResponseDataWorkerAffinityType0 | None | Unset): Worker affinity override
            stored on the execution, if any.
        worker_selector (ApiResponseExecutionResponseDataWorkerSelectorType0 | None | Unset): Worker selector override
            stored on the execution, if any.
        worker_tolerations (list[ApiResponseExecutionResponseDataWorkerTolerationsType0Item] | None | Unset): Worker
            tolerations override stored on the execution, if any.
        workflow_task (ApiResponseExecutionResponseDataWorkflowTaskType0 | None | Unset): Workflow task metadata (only
            populated for workflow task executions)
    """

    action_ref: str
    config: ApiResponseExecutionResponseDataConfig
    created: datetime.datetime
    id: int
    result: ApiResponseExecutionResponseDataResult
    status: ExecutionStatus
    updated: datetime.datetime
    action: int | None | Unset = UNSET
    artifact_retention_limit: int | None | Unset = UNSET
    artifact_retention_policy: None | RetentionPolicyType | Unset = UNSET
    enforcement: int | None | Unset = UNSET
    executor: int | None | Unset = UNSET
    original_execution: int | None | Unset = UNSET
    parent: int | None | Unset = UNSET
    permission_set_refs: list[str] | Unset = UNSET
    started_at: datetime.datetime | None | Unset = UNSET
    timeout_seconds: int | None | Unset = UNSET
    worker: int | None | Unset = UNSET
    worker_affinity: (
        ApiResponseExecutionResponseDataWorkerAffinityType0 | None | Unset
    ) = UNSET
    worker_selector: (
        ApiResponseExecutionResponseDataWorkerSelectorType0 | None | Unset
    ) = UNSET
    worker_tolerations: (
        list[ApiResponseExecutionResponseDataWorkerTolerationsType0Item] | None | Unset
    ) = UNSET
    workflow_task: ApiResponseExecutionResponseDataWorkflowTaskType0 | None | Unset = (
        UNSET
    )
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.api_response_execution_response_data_worker_affinity_type_0 import (
            ApiResponseExecutionResponseDataWorkerAffinityType0,
        )
        from ..models.api_response_execution_response_data_worker_selector_type_0 import (
            ApiResponseExecutionResponseDataWorkerSelectorType0,
        )
        from ..models.api_response_execution_response_data_workflow_task_type_0 import (
            ApiResponseExecutionResponseDataWorkflowTaskType0,
        )

        action_ref = self.action_ref

        config = self.config.to_dict()

        created = self.created.isoformat()

        id = self.id

        result = self.result.to_dict()

        status = self.status.value

        updated = self.updated.isoformat()

        action: int | None | Unset
        if isinstance(self.action, Unset):
            action = UNSET
        else:
            action = self.action

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

        enforcement: int | None | Unset
        if isinstance(self.enforcement, Unset):
            enforcement = UNSET
        else:
            enforcement = self.enforcement

        executor: int | None | Unset
        if isinstance(self.executor, Unset):
            executor = UNSET
        else:
            executor = self.executor

        original_execution: int | None | Unset
        if isinstance(self.original_execution, Unset):
            original_execution = UNSET
        else:
            original_execution = self.original_execution

        parent: int | None | Unset
        if isinstance(self.parent, Unset):
            parent = UNSET
        else:
            parent = self.parent

        permission_set_refs: list[str] | Unset = UNSET
        if not isinstance(self.permission_set_refs, Unset):
            permission_set_refs = self.permission_set_refs

        started_at: None | str | Unset
        if isinstance(self.started_at, Unset):
            started_at = UNSET
        elif isinstance(self.started_at, datetime.datetime):
            started_at = self.started_at.isoformat()
        else:
            started_at = self.started_at

        timeout_seconds: int | None | Unset
        if isinstance(self.timeout_seconds, Unset):
            timeout_seconds = UNSET
        else:
            timeout_seconds = self.timeout_seconds

        worker: int | None | Unset
        if isinstance(self.worker, Unset):
            worker = UNSET
        else:
            worker = self.worker

        worker_affinity: dict[str, Any] | None | Unset
        if isinstance(self.worker_affinity, Unset):
            worker_affinity = UNSET
        elif isinstance(
            self.worker_affinity, ApiResponseExecutionResponseDataWorkerAffinityType0
        ):
            worker_affinity = self.worker_affinity.to_dict()
        else:
            worker_affinity = self.worker_affinity

        worker_selector: dict[str, Any] | None | Unset
        if isinstance(self.worker_selector, Unset):
            worker_selector = UNSET
        elif isinstance(
            self.worker_selector, ApiResponseExecutionResponseDataWorkerSelectorType0
        ):
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

        workflow_task: dict[str, Any] | None | Unset
        if isinstance(self.workflow_task, Unset):
            workflow_task = UNSET
        elif isinstance(
            self.workflow_task, ApiResponseExecutionResponseDataWorkflowTaskType0
        ):
            workflow_task = self.workflow_task.to_dict()
        else:
            workflow_task = self.workflow_task

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "action_ref": action_ref,
                "config": config,
                "created": created,
                "id": id,
                "result": result,
                "status": status,
                "updated": updated,
            }
        )
        if action is not UNSET:
            field_dict["action"] = action
        if artifact_retention_limit is not UNSET:
            field_dict["artifact_retention_limit"] = artifact_retention_limit
        if artifact_retention_policy is not UNSET:
            field_dict["artifact_retention_policy"] = artifact_retention_policy
        if enforcement is not UNSET:
            field_dict["enforcement"] = enforcement
        if executor is not UNSET:
            field_dict["executor"] = executor
        if original_execution is not UNSET:
            field_dict["original_execution"] = original_execution
        if parent is not UNSET:
            field_dict["parent"] = parent
        if permission_set_refs is not UNSET:
            field_dict["permission_set_refs"] = permission_set_refs
        if started_at is not UNSET:
            field_dict["started_at"] = started_at
        if timeout_seconds is not UNSET:
            field_dict["timeout_seconds"] = timeout_seconds
        if worker is not UNSET:
            field_dict["worker"] = worker
        if worker_affinity is not UNSET:
            field_dict["worker_affinity"] = worker_affinity
        if worker_selector is not UNSET:
            field_dict["worker_selector"] = worker_selector
        if worker_tolerations is not UNSET:
            field_dict["worker_tolerations"] = worker_tolerations
        if workflow_task is not UNSET:
            field_dict["workflow_task"] = workflow_task

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.api_response_execution_response_data_config import (
            ApiResponseExecutionResponseDataConfig,
        )
        from ..models.api_response_execution_response_data_result import (
            ApiResponseExecutionResponseDataResult,
        )
        from ..models.api_response_execution_response_data_worker_affinity_type_0 import (
            ApiResponseExecutionResponseDataWorkerAffinityType0,
        )
        from ..models.api_response_execution_response_data_worker_selector_type_0 import (
            ApiResponseExecutionResponseDataWorkerSelectorType0,
        )
        from ..models.api_response_execution_response_data_worker_tolerations_type_0_item import (
            ApiResponseExecutionResponseDataWorkerTolerationsType0Item,
        )
        from ..models.api_response_execution_response_data_workflow_task_type_0 import (
            ApiResponseExecutionResponseDataWorkflowTaskType0,
        )

        d = dict(src_dict)
        action_ref = d.pop("action_ref")

        config = ApiResponseExecutionResponseDataConfig.from_dict(d.pop("config"))

        created = datetime.datetime.fromisoformat(d.pop("created"))

        id = d.pop("id")

        result = ApiResponseExecutionResponseDataResult.from_dict(d.pop("result"))

        status = ExecutionStatus(d.pop("status"))

        updated = datetime.datetime.fromisoformat(d.pop("updated"))

        def _parse_action(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        action = _parse_action(d.pop("action", UNSET))

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

        def _parse_enforcement(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        enforcement = _parse_enforcement(d.pop("enforcement", UNSET))

        def _parse_executor(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        executor = _parse_executor(d.pop("executor", UNSET))

        def _parse_original_execution(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        original_execution = _parse_original_execution(
            d.pop("original_execution", UNSET)
        )

        def _parse_parent(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        parent = _parse_parent(d.pop("parent", UNSET))

        permission_set_refs = cast(list[str], d.pop("permission_set_refs", UNSET))

        def _parse_started_at(data: object) -> datetime.datetime | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                started_at_type_0 = datetime.datetime.fromisoformat(data)

                return started_at_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(datetime.datetime | None | Unset, data)

        started_at = _parse_started_at(d.pop("started_at", UNSET))

        def _parse_timeout_seconds(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        timeout_seconds = _parse_timeout_seconds(d.pop("timeout_seconds", UNSET))

        def _parse_worker(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        worker = _parse_worker(d.pop("worker", UNSET))

        def _parse_worker_affinity(
            data: object,
        ) -> ApiResponseExecutionResponseDataWorkerAffinityType0 | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                worker_affinity_type_0 = (
                    ApiResponseExecutionResponseDataWorkerAffinityType0.from_dict(data)
                )

                return worker_affinity_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(
                ApiResponseExecutionResponseDataWorkerAffinityType0 | None | Unset, data
            )

        worker_affinity = _parse_worker_affinity(d.pop("worker_affinity", UNSET))

        def _parse_worker_selector(
            data: object,
        ) -> ApiResponseExecutionResponseDataWorkerSelectorType0 | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                worker_selector_type_0 = (
                    ApiResponseExecutionResponseDataWorkerSelectorType0.from_dict(data)
                )

                return worker_selector_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(
                ApiResponseExecutionResponseDataWorkerSelectorType0 | None | Unset, data
            )

        worker_selector = _parse_worker_selector(d.pop("worker_selector", UNSET))

        def _parse_worker_tolerations(
            data: object,
        ) -> (
            list[ApiResponseExecutionResponseDataWorkerTolerationsType0Item]
            | None
            | Unset
        ):
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
                    worker_tolerations_type_0_item = ApiResponseExecutionResponseDataWorkerTolerationsType0Item.from_dict(
                        worker_tolerations_type_0_item_data
                    )

                    worker_tolerations_type_0.append(worker_tolerations_type_0_item)

                return worker_tolerations_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(
                list[ApiResponseExecutionResponseDataWorkerTolerationsType0Item]
                | None
                | Unset,
                data,
            )

        worker_tolerations = _parse_worker_tolerations(
            d.pop("worker_tolerations", UNSET)
        )

        def _parse_workflow_task(
            data: object,
        ) -> ApiResponseExecutionResponseDataWorkflowTaskType0 | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                workflow_task_type_0 = (
                    ApiResponseExecutionResponseDataWorkflowTaskType0.from_dict(data)
                )

                return workflow_task_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(
                ApiResponseExecutionResponseDataWorkflowTaskType0 | None | Unset, data
            )

        workflow_task = _parse_workflow_task(d.pop("workflow_task", UNSET))

        api_response_execution_response_data = cls(
            action_ref=action_ref,
            config=config,
            created=created,
            id=id,
            result=result,
            status=status,
            updated=updated,
            action=action,
            artifact_retention_limit=artifact_retention_limit,
            artifact_retention_policy=artifact_retention_policy,
            enforcement=enforcement,
            executor=executor,
            original_execution=original_execution,
            parent=parent,
            permission_set_refs=permission_set_refs,
            started_at=started_at,
            timeout_seconds=timeout_seconds,
            worker=worker,
            worker_affinity=worker_affinity,
            worker_selector=worker_selector,
            worker_tolerations=worker_tolerations,
            workflow_task=workflow_task,
        )

        api_response_execution_response_data.additional_properties = d
        return api_response_execution_response_data

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
