from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.retention_policy_type import RetentionPolicyType
from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.create_execution_request_env_vars import CreateExecutionRequestEnvVars
    from ..models.create_execution_request_parameters import (
        CreateExecutionRequestParameters,
    )
    from ..models.create_execution_request_worker_affinity_type_0 import (
        CreateExecutionRequestWorkerAffinityType0,
    )
    from ..models.create_execution_request_worker_selector_type_0 import (
        CreateExecutionRequestWorkerSelectorType0,
    )
    from ..models.create_execution_request_worker_tolerations_type_0_item import (
        CreateExecutionRequestWorkerTolerationsType0Item,
    )


T = TypeVar("T", bound="CreateExecutionRequest")


@_attrs_define
class CreateExecutionRequest:
    """Request DTO for creating a manual execution

    Attributes:
        action_ref (str): Action reference to execute Example: slack.post_message.
        env_vars (CreateExecutionRequestEnvVars): Environment variables for this execution
        parameters (CreateExecutionRequestParameters): Execution parameters/configuration
        artifact_retention_limit (int | None | Unset): Retention limit override for non-log artifacts created by this
            execution.
            Omit to inherit the action default. Example: 10.
        artifact_retention_policy (None | RetentionPolicyType | Unset):
        permission_set_refs (list[str] | None | Unset): Permission set refs to apply to this execution's API token. Omit
            to use
            the action default. Provide an empty array to force no API token. Example: ['core.agent_reader'].
        timeout_seconds (int | None | Unset): Execution timeout override in seconds. Omit to inherit the action default
            (or the app-level `default_execution_timeout_seconds` when the action has
            no default). Must be a positive integer. Example: 300.
        worker_affinity (CreateExecutionRequestWorkerAffinityType0 | None | Unset): Worker affinity override. Omit to
            inherit the action default; provide
            `{}` to explicitly clear affinity requirements/preferences.
        worker_selector (CreateExecutionRequestWorkerSelectorType0 | None | Unset): Worker label selector override. Omit
            to inherit the action default;
            provide `{}` to explicitly clear selector requirements.
        worker_tolerations (list[CreateExecutionRequestWorkerTolerationsType0Item] | None | Unset): Worker taint
            tolerations override. Omit to inherit the action default;
            provide `[]` to explicitly clear tolerations. Example: [{'effect': 'no_schedule', 'key': 'dedicated',
            'operator': 'equal', 'value': 'gpu'}].
    """

    action_ref: str
    env_vars: CreateExecutionRequestEnvVars
    parameters: CreateExecutionRequestParameters
    artifact_retention_limit: int | None | Unset = UNSET
    artifact_retention_policy: None | RetentionPolicyType | Unset = UNSET
    permission_set_refs: list[str] | None | Unset = UNSET
    timeout_seconds: int | None | Unset = UNSET
    worker_affinity: CreateExecutionRequestWorkerAffinityType0 | None | Unset = UNSET
    worker_selector: CreateExecutionRequestWorkerSelectorType0 | None | Unset = UNSET
    worker_tolerations: (
        list[CreateExecutionRequestWorkerTolerationsType0Item] | None | Unset
    ) = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.create_execution_request_worker_affinity_type_0 import (
            CreateExecutionRequestWorkerAffinityType0,
        )
        from ..models.create_execution_request_worker_selector_type_0 import (
            CreateExecutionRequestWorkerSelectorType0,
        )

        action_ref = self.action_ref

        env_vars = self.env_vars.to_dict()

        parameters = self.parameters.to_dict()

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

        permission_set_refs: list[str] | None | Unset
        if isinstance(self.permission_set_refs, Unset):
            permission_set_refs = UNSET
        elif isinstance(self.permission_set_refs, list):
            permission_set_refs = self.permission_set_refs

        else:
            permission_set_refs = self.permission_set_refs

        timeout_seconds: int | None | Unset
        if isinstance(self.timeout_seconds, Unset):
            timeout_seconds = UNSET
        else:
            timeout_seconds = self.timeout_seconds

        worker_affinity: dict[str, Any] | None | Unset
        if isinstance(self.worker_affinity, Unset):
            worker_affinity = UNSET
        elif isinstance(
            self.worker_affinity, CreateExecutionRequestWorkerAffinityType0
        ):
            worker_affinity = self.worker_affinity.to_dict()
        else:
            worker_affinity = self.worker_affinity

        worker_selector: dict[str, Any] | None | Unset
        if isinstance(self.worker_selector, Unset):
            worker_selector = UNSET
        elif isinstance(
            self.worker_selector, CreateExecutionRequestWorkerSelectorType0
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

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "action_ref": action_ref,
                "env_vars": env_vars,
                "parameters": parameters,
            }
        )
        if artifact_retention_limit is not UNSET:
            field_dict["artifact_retention_limit"] = artifact_retention_limit
        if artifact_retention_policy is not UNSET:
            field_dict["artifact_retention_policy"] = artifact_retention_policy
        if permission_set_refs is not UNSET:
            field_dict["permission_set_refs"] = permission_set_refs
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
        from ..models.create_execution_request_env_vars import (
            CreateExecutionRequestEnvVars,
        )
        from ..models.create_execution_request_parameters import (
            CreateExecutionRequestParameters,
        )
        from ..models.create_execution_request_worker_affinity_type_0 import (
            CreateExecutionRequestWorkerAffinityType0,
        )
        from ..models.create_execution_request_worker_selector_type_0 import (
            CreateExecutionRequestWorkerSelectorType0,
        )
        from ..models.create_execution_request_worker_tolerations_type_0_item import (
            CreateExecutionRequestWorkerTolerationsType0Item,
        )

        d = dict(src_dict)
        action_ref = d.pop("action_ref")

        env_vars = CreateExecutionRequestEnvVars.from_dict(d.pop("env_vars"))

        parameters = CreateExecutionRequestParameters.from_dict(d.pop("parameters"))

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

        def _parse_permission_set_refs(data: object) -> list[str] | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, list):
                    raise TypeError()
                permission_set_refs_type_0 = cast(list[str], data)

                return permission_set_refs_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(list[str] | None | Unset, data)

        permission_set_refs = _parse_permission_set_refs(
            d.pop("permission_set_refs", UNSET)
        )

        def _parse_timeout_seconds(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        timeout_seconds = _parse_timeout_seconds(d.pop("timeout_seconds", UNSET))

        def _parse_worker_affinity(
            data: object,
        ) -> CreateExecutionRequestWorkerAffinityType0 | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                worker_affinity_type_0 = (
                    CreateExecutionRequestWorkerAffinityType0.from_dict(data)
                )

                return worker_affinity_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(CreateExecutionRequestWorkerAffinityType0 | None | Unset, data)

        worker_affinity = _parse_worker_affinity(d.pop("worker_affinity", UNSET))

        def _parse_worker_selector(
            data: object,
        ) -> CreateExecutionRequestWorkerSelectorType0 | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                worker_selector_type_0 = (
                    CreateExecutionRequestWorkerSelectorType0.from_dict(data)
                )

                return worker_selector_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(CreateExecutionRequestWorkerSelectorType0 | None | Unset, data)

        worker_selector = _parse_worker_selector(d.pop("worker_selector", UNSET))

        def _parse_worker_tolerations(
            data: object,
        ) -> list[CreateExecutionRequestWorkerTolerationsType0Item] | None | Unset:
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
                    worker_tolerations_type_0_item = (
                        CreateExecutionRequestWorkerTolerationsType0Item.from_dict(
                            worker_tolerations_type_0_item_data
                        )
                    )

                    worker_tolerations_type_0.append(worker_tolerations_type_0_item)

                return worker_tolerations_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(
                list[CreateExecutionRequestWorkerTolerationsType0Item] | None | Unset,
                data,
            )

        worker_tolerations = _parse_worker_tolerations(
            d.pop("worker_tolerations", UNSET)
        )

        create_execution_request = cls(
            action_ref=action_ref,
            env_vars=env_vars,
            parameters=parameters,
            artifact_retention_limit=artifact_retention_limit,
            artifact_retention_policy=artifact_retention_policy,
            permission_set_refs=permission_set_refs,
            timeout_seconds=timeout_seconds,
            worker_affinity=worker_affinity,
            worker_selector=worker_selector,
            worker_tolerations=worker_tolerations,
        )

        create_execution_request.additional_properties = d
        return create_execution_request

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
