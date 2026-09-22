from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

if TYPE_CHECKING:
    from ..models.inquiry_response import InquiryResponse
    from ..models.inquiry_response_option_handle import InquiryResponseOptionHandle


T = TypeVar("T", bound="CreateInquiryResponse")


@_attrs_define
class CreateInquiryResponse:
    """Creation result containing the inquiry and one opaque handle per response option.

    Attributes:
        inquiry (InquiryResponse): Full inquiry response with all details
        response_options (list[InquiryResponseOptionHandle]):
    """

    inquiry: InquiryResponse
    response_options: list[InquiryResponseOptionHandle]
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        inquiry = self.inquiry.to_dict()

        response_options = []
        for response_options_item_data in self.response_options:
            response_options_item = response_options_item_data.to_dict()
            response_options.append(response_options_item)

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "inquiry": inquiry,
                "response_options": response_options,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        from ..models.inquiry_response import InquiryResponse
        from ..models.inquiry_response_option_handle import (
            InquiryResponseOptionHandle,
        )

        d = dict(src_dict)
        inquiry = InquiryResponse.from_dict(d.pop("inquiry"))

        response_options = []
        _response_options = d.pop("response_options")
        for response_options_item_data in _response_options:
            response_options_item = InquiryResponseOptionHandle.from_dict(
                response_options_item_data
            )

            response_options.append(response_options_item)

        create_inquiry_response = cls(
            inquiry=inquiry,
            response_options=response_options,
        )

        create_inquiry_response.additional_properties = d
        return create_inquiry_response

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
