from __future__ import annotations

import datetime
from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

from ..models.inquiry_status import InquiryStatus
from ..types import UNSET, Unset

T = TypeVar("T", bound="InquirySummary")


@_attrs_define
class InquirySummary:
    """Summary inquiry response for list views

    Attributes:
        created (datetime.datetime): Creation timestamp Example: 2024-01-13T10:30:00Z.
        created_by_execution (int):
        has_response (bool): Whether a response has been provided Example: False.
        id (int):
        prompt (str): Prompt text Example: Approve deployment to production?.
        status (InquiryStatus):
        assigned_to (int | None | Unset):
        assigned_to_display_name (None | str | Unset):
        assigned_to_login (None | str | Unset):
        created_by_action_ref (None | str | Unset):
        created_by_pack_ref (None | str | Unset):
        timeout_at (datetime.datetime | None | Unset): Timeout timestamp Example: 2024-01-13T11:30:00Z.
        workflow_action_ref (None | str | Unset):
        workflow_execution (int | None | Unset):
        workflow_pack_ref (None | str | Unset):
        workflow_root_execution (int | None | Unset):
        workflow_task_name (None | str | Unset):
    """

    created: datetime.datetime
    created_by_execution: int
    has_response: bool
    id: int
    prompt: str
    status: InquiryStatus
    assigned_to: int | None | Unset = UNSET
    assigned_to_display_name: None | str | Unset = UNSET
    assigned_to_login: None | str | Unset = UNSET
    created_by_action_ref: None | str | Unset = UNSET
    created_by_pack_ref: None | str | Unset = UNSET
    timeout_at: datetime.datetime | None | Unset = UNSET
    workflow_action_ref: None | str | Unset = UNSET
    workflow_execution: int | None | Unset = UNSET
    workflow_pack_ref: None | str | Unset = UNSET
    workflow_root_execution: int | None | Unset = UNSET
    workflow_task_name: None | str | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        created = self.created.isoformat()

        created_by_execution = self.created_by_execution

        has_response = self.has_response

        id = self.id

        prompt = self.prompt

        status = self.status.value

        assigned_to: int | None | Unset
        if isinstance(self.assigned_to, Unset):
            assigned_to = UNSET
        else:
            assigned_to = self.assigned_to

        assigned_to_display_name: None | str | Unset
        if isinstance(self.assigned_to_display_name, Unset):
            assigned_to_display_name = UNSET
        else:
            assigned_to_display_name = self.assigned_to_display_name

        assigned_to_login: None | str | Unset
        if isinstance(self.assigned_to_login, Unset):
            assigned_to_login = UNSET
        else:
            assigned_to_login = self.assigned_to_login

        created_by_action_ref: None | str | Unset
        if isinstance(self.created_by_action_ref, Unset):
            created_by_action_ref = UNSET
        else:
            created_by_action_ref = self.created_by_action_ref

        created_by_pack_ref: None | str | Unset
        if isinstance(self.created_by_pack_ref, Unset):
            created_by_pack_ref = UNSET
        else:
            created_by_pack_ref = self.created_by_pack_ref

        timeout_at: None | str | Unset
        if isinstance(self.timeout_at, Unset):
            timeout_at = UNSET
        elif isinstance(self.timeout_at, datetime.datetime):
            timeout_at = self.timeout_at.isoformat()
        else:
            timeout_at = self.timeout_at

        workflow_action_ref: None | str | Unset
        if isinstance(self.workflow_action_ref, Unset):
            workflow_action_ref = UNSET
        else:
            workflow_action_ref = self.workflow_action_ref

        workflow_execution: int | None | Unset
        if isinstance(self.workflow_execution, Unset):
            workflow_execution = UNSET
        else:
            workflow_execution = self.workflow_execution

        workflow_pack_ref: None | str | Unset
        if isinstance(self.workflow_pack_ref, Unset):
            workflow_pack_ref = UNSET
        else:
            workflow_pack_ref = self.workflow_pack_ref

        workflow_root_execution: int | None | Unset
        if isinstance(self.workflow_root_execution, Unset):
            workflow_root_execution = UNSET
        else:
            workflow_root_execution = self.workflow_root_execution

        workflow_task_name: None | str | Unset
        if isinstance(self.workflow_task_name, Unset):
            workflow_task_name = UNSET
        else:
            workflow_task_name = self.workflow_task_name

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "created": created,
                "created_by_execution": created_by_execution,
                "has_response": has_response,
                "id": id,
                "prompt": prompt,
                "status": status,
            }
        )
        if assigned_to is not UNSET:
            field_dict["assigned_to"] = assigned_to
        if assigned_to_display_name is not UNSET:
            field_dict["assigned_to_display_name"] = assigned_to_display_name
        if assigned_to_login is not UNSET:
            field_dict["assigned_to_login"] = assigned_to_login
        if created_by_action_ref is not UNSET:
            field_dict["created_by_action_ref"] = created_by_action_ref
        if created_by_pack_ref is not UNSET:
            field_dict["created_by_pack_ref"] = created_by_pack_ref
        if timeout_at is not UNSET:
            field_dict["timeout_at"] = timeout_at
        if workflow_action_ref is not UNSET:
            field_dict["workflow_action_ref"] = workflow_action_ref
        if workflow_execution is not UNSET:
            field_dict["workflow_execution"] = workflow_execution
        if workflow_pack_ref is not UNSET:
            field_dict["workflow_pack_ref"] = workflow_pack_ref
        if workflow_root_execution is not UNSET:
            field_dict["workflow_root_execution"] = workflow_root_execution
        if workflow_task_name is not UNSET:
            field_dict["workflow_task_name"] = workflow_task_name

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        d = dict(src_dict)
        created = datetime.datetime.fromisoformat(d.pop("created"))

        created_by_execution = d.pop("created_by_execution")

        has_response = d.pop("has_response")

        id = d.pop("id")

        prompt = d.pop("prompt")

        status = InquiryStatus(d.pop("status"))

        def _parse_assigned_to(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        assigned_to = _parse_assigned_to(d.pop("assigned_to", UNSET))

        def _parse_assigned_to_display_name(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        assigned_to_display_name = _parse_assigned_to_display_name(
            d.pop("assigned_to_display_name", UNSET)
        )

        def _parse_assigned_to_login(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        assigned_to_login = _parse_assigned_to_login(d.pop("assigned_to_login", UNSET))

        def _parse_created_by_action_ref(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        created_by_action_ref = _parse_created_by_action_ref(
            d.pop("created_by_action_ref", UNSET)
        )

        def _parse_created_by_pack_ref(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        created_by_pack_ref = _parse_created_by_pack_ref(
            d.pop("created_by_pack_ref", UNSET)
        )

        def _parse_timeout_at(data: object) -> datetime.datetime | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                timeout_at_type_0 = datetime.datetime.fromisoformat(data)

                return timeout_at_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(datetime.datetime | None | Unset, data)

        timeout_at = _parse_timeout_at(d.pop("timeout_at", UNSET))

        def _parse_workflow_action_ref(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        workflow_action_ref = _parse_workflow_action_ref(
            d.pop("workflow_action_ref", UNSET)
        )

        def _parse_workflow_execution(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        workflow_execution = _parse_workflow_execution(
            d.pop("workflow_execution", UNSET)
        )

        def _parse_workflow_pack_ref(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        workflow_pack_ref = _parse_workflow_pack_ref(d.pop("workflow_pack_ref", UNSET))

        def _parse_workflow_root_execution(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        workflow_root_execution = _parse_workflow_root_execution(
            d.pop("workflow_root_execution", UNSET)
        )

        def _parse_workflow_task_name(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        workflow_task_name = _parse_workflow_task_name(
            d.pop("workflow_task_name", UNSET)
        )

        inquiry_summary = cls(
            created=created,
            created_by_execution=created_by_execution,
            has_response=has_response,
            id=id,
            prompt=prompt,
            status=status,
            assigned_to=assigned_to,
            assigned_to_display_name=assigned_to_display_name,
            assigned_to_login=assigned_to_login,
            created_by_action_ref=created_by_action_ref,
            created_by_pack_ref=created_by_pack_ref,
            timeout_at=timeout_at,
            workflow_action_ref=workflow_action_ref,
            workflow_execution=workflow_execution,
            workflow_pack_ref=workflow_pack_ref,
            workflow_root_execution=workflow_root_execution,
            workflow_task_name=workflow_task_name,
        )

        inquiry_summary.additional_properties = d
        return inquiry_summary

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
