from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

if TYPE_CHECKING:
    from ..models.inquiry_response import InquiryResponse


T = TypeVar("T", bound="ApiResponseCreateInquiryResponseData")


@_attrs_define
class ApiResponseCreateInquiryResponseData:
    """Creation result containing the inquiry and its provider-neutral response handle.

    Attributes:
        inquiry (InquiryResponse): Full inquiry response with all details
        response_handle (str): Opaque correlation handle for one-shot external responses. Example: attune_irh_REDACTED.
    """

    inquiry: InquiryResponse
    response_handle: str
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        inquiry = self.inquiry.to_dict()

        response_handle = self.response_handle

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "inquiry": inquiry,
                "response_handle": response_handle,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        from ..models.inquiry_response import InquiryResponse

        d = dict(src_dict)
        inquiry = InquiryResponse.from_dict(d.pop("inquiry"))

        response_handle = d.pop("response_handle")

        api_response_create_inquiry_response_data = cls(
            inquiry=inquiry,
            response_handle=response_handle,
        )

        api_response_create_inquiry_response_data.additional_properties = d
        return api_response_create_inquiry_response_data

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
