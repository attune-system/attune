from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.create_inquiry_request_response_schema_type_0 import (
        CreateInquiryRequestResponseSchemaType0,
    )
    from ..models.inquiry_response_option import InquiryResponseOption


T = TypeVar("T", bound="CreateInquiryRequest")


@_attrs_define
class CreateInquiryRequest:
    """Request to create a new inquiry

    Attributes:
        prompt (str): Prompt text to display to the user Example: Approve deployment to production?.
        purpose (str): Stable purpose used to make creation idempotent within this workflow task attempt. Example:
            approval.
        response_options (list[InquiryResponseOption]): Fixed response choices rendered by provider actions.
        assigned_to (int | None | Unset):
        response_schema (CreateInquiryRequestResponseSchemaType0 | None | Unset): Optional schema for the expected
            response format (flat format with inline required/secret)
        timeout_seconds (int | None | Unset): Optional relative timeout in seconds. Example: 3600.
    """

    prompt: str
    purpose: str
    response_options: list[InquiryResponseOption]
    assigned_to: int | None | Unset = UNSET
    response_schema: CreateInquiryRequestResponseSchemaType0 | None | Unset = UNSET
    timeout_seconds: int | None | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.create_inquiry_request_response_schema_type_0 import (
            CreateInquiryRequestResponseSchemaType0,
        )

        prompt = self.prompt

        purpose = self.purpose

        response_options = []
        for response_options_item_data in self.response_options:
            response_options_item = response_options_item_data.to_dict()
            response_options.append(response_options_item)

        assigned_to: int | None | Unset
        if isinstance(self.assigned_to, Unset):
            assigned_to = UNSET
        else:
            assigned_to = self.assigned_to

        response_schema: dict[str, Any] | None | Unset
        if isinstance(self.response_schema, Unset):
            response_schema = UNSET
        elif isinstance(self.response_schema, CreateInquiryRequestResponseSchemaType0):
            response_schema = self.response_schema.to_dict()
        else:
            response_schema = self.response_schema

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
                "response_options": response_options,
            }
        )
        if assigned_to is not UNSET:
            field_dict["assigned_to"] = assigned_to
        if response_schema is not UNSET:
            field_dict["response_schema"] = response_schema
        if timeout_seconds is not UNSET:
            field_dict["timeout_seconds"] = timeout_seconds

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        from ..models.create_inquiry_request_response_schema_type_0 import (
            CreateInquiryRequestResponseSchemaType0,
        )
        from ..models.inquiry_response_option import (
            InquiryResponseOption,
        )

        d = dict(src_dict)
        prompt = d.pop("prompt")

        purpose = d.pop("purpose")

        response_options = []
        _response_options = d.pop("response_options")
        for response_options_item_data in _response_options:
            response_options_item = InquiryResponseOption.from_dict(
                response_options_item_data
            )

            response_options.append(response_options_item)

        def _parse_assigned_to(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        assigned_to = _parse_assigned_to(d.pop("assigned_to", UNSET))

        def _parse_response_schema(
            data: object,
        ) -> CreateInquiryRequestResponseSchemaType0 | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                response_schema_type_0 = (
                    CreateInquiryRequestResponseSchemaType0.from_dict(data)
                )

                return response_schema_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(CreateInquiryRequestResponseSchemaType0 | None | Unset, data)

        response_schema = _parse_response_schema(d.pop("response_schema", UNSET))

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
            response_options=response_options,
            assigned_to=assigned_to,
            response_schema=response_schema,
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
