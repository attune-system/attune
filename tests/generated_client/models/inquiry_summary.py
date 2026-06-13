from __future__ import annotations

import datetime
from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.inquiry_status import InquiryStatus
from ..types import UNSET, Unset

T = TypeVar("T", bound="InquirySummary")


@_attrs_define
class InquirySummary:
    """Summary inquiry response for list views

    Attributes:
        created (datetime.datetime): Creation timestamp Example: 2024-01-13T10:30:00Z.
        execution (int):
        has_response (bool): Whether a response has been provided
        id (int):
        prompt (str): Prompt text Example: Approve deployment to production?.
        status (InquiryStatus):
        assigned_to (int | None | Unset):
        timeout_at (datetime.datetime | None | Unset): Timeout timestamp Example: 2024-01-13T11:30:00Z.
    """

    created: datetime.datetime
    execution: int
    has_response: bool
    id: int
    prompt: str
    status: InquiryStatus
    assigned_to: int | None | Unset = UNSET
    timeout_at: datetime.datetime | None | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        created = self.created.isoformat()

        execution = self.execution

        has_response = self.has_response

        id = self.id

        prompt = self.prompt

        status = self.status.value

        assigned_to: int | None | Unset
        if isinstance(self.assigned_to, Unset):
            assigned_to = UNSET
        else:
            assigned_to = self.assigned_to

        timeout_at: None | str | Unset
        if isinstance(self.timeout_at, Unset):
            timeout_at = UNSET
        elif isinstance(self.timeout_at, datetime.datetime):
            timeout_at = self.timeout_at.isoformat()
        else:
            timeout_at = self.timeout_at

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "created": created,
                "execution": execution,
                "has_response": has_response,
                "id": id,
                "prompt": prompt,
                "status": status,
            }
        )
        if assigned_to is not UNSET:
            field_dict["assigned_to"] = assigned_to
        if timeout_at is not UNSET:
            field_dict["timeout_at"] = timeout_at

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        created = datetime.datetime.fromisoformat(d.pop("created"))

        execution = d.pop("execution")

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

        inquiry_summary = cls(
            created=created,
            execution=execution,
            has_response=has_response,
            id=id,
            prompt=prompt,
            status=status,
            assigned_to=assigned_to,
            timeout_at=timeout_at,
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
