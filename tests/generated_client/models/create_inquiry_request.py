from __future__ import annotations

import datetime
from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.create_inquiry_request_response_schema import (
        CreateInquiryRequestResponseSchema,
    )


T = TypeVar("T", bound="CreateInquiryRequest")


@_attrs_define
class CreateInquiryRequest:
    """Request to create a new inquiry

    Attributes:
        execution (int):
        prompt (str): Prompt text to display to the user Example: Approve deployment to production?.
        response_schema (CreateInquiryRequestResponseSchema): Optional schema for the expected response format (flat
            format with inline required/secret)
        assigned_to (int | None | Unset):
        timeout_at (datetime.datetime | None | Unset): Optional timeout timestamp (when inquiry expires) Example:
            2024-01-13T11:30:00Z.
    """

    execution: int
    prompt: str
    response_schema: CreateInquiryRequestResponseSchema
    assigned_to: int | None | Unset = UNSET
    timeout_at: datetime.datetime | None | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        execution = self.execution

        prompt = self.prompt

        response_schema = self.response_schema.to_dict()

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
                "execution": execution,
                "prompt": prompt,
                "response_schema": response_schema,
            }
        )
        if assigned_to is not UNSET:
            field_dict["assigned_to"] = assigned_to
        if timeout_at is not UNSET:
            field_dict["timeout_at"] = timeout_at

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.create_inquiry_request_response_schema import (
            CreateInquiryRequestResponseSchema,
        )

        d = dict(src_dict)
        execution = d.pop("execution")

        prompt = d.pop("prompt")

        response_schema = CreateInquiryRequestResponseSchema.from_dict(
            d.pop("response_schema")
        )

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

        create_inquiry_request = cls(
            execution=execution,
            prompt=prompt,
            response_schema=response_schema,
            assigned_to=assigned_to,
            timeout_at=timeout_at,
        )

        create_inquiry_request.additional_properties = d
        return create_inquiry_request

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
