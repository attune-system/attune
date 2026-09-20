from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

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
        prompt (str): Prompt text to display to the user Example: Approve deployment to production?.
        purpose (str): Stable purpose used to make creation idempotent within this workflow task attempt. Example:
            approval.
        response_schema (CreateInquiryRequestResponseSchema): Optional schema for the expected response format (flat
            format with inline required/secret)
        assigned_to (int | None | Unset):
        timeout_seconds (int | None | Unset): Optional relative timeout in seconds. Example: 3600.
    """

    prompt: str
    purpose: str
    response_schema: CreateInquiryRequestResponseSchema
    assigned_to: int | None | Unset = UNSET
    timeout_seconds: int | None | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        prompt = self.prompt

        purpose = self.purpose

        response_schema = self.response_schema.to_dict()

        assigned_to: int | None | Unset
        if isinstance(self.assigned_to, Unset):
            assigned_to = UNSET
        else:
            assigned_to = self.assigned_to

        timeout_seconds: int | None | Unset
        if isinstance(self.timeout_seconds, Unset):
            timeout_seconds = UNSET
        else:
            timeout_seconds = self.timeout_seconds

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "prompt": prompt,
                "purpose": purpose,
                "response_schema": response_schema,
            }
        )
        if assigned_to is not UNSET:
            field_dict["assigned_to"] = assigned_to
        if timeout_seconds is not UNSET:
            field_dict["timeout_seconds"] = timeout_seconds

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        from ..models.create_inquiry_request_response_schema import (
            CreateInquiryRequestResponseSchema,
        )

        d = dict(src_dict)
        prompt = d.pop("prompt")

        purpose = d.pop("purpose")

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

        def _parse_timeout_seconds(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        timeout_seconds = _parse_timeout_seconds(d.pop("timeout_seconds", UNSET))

        create_inquiry_request = cls(
            prompt=prompt,
            purpose=purpose,
            response_schema=response_schema,
            assigned_to=assigned_to,
            timeout_seconds=timeout_seconds,
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
