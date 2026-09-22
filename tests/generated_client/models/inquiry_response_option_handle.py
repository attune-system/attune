from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

from ..models.inquiry_response_option_style import InquiryResponseOptionStyle

T = TypeVar("T", bound="InquiryResponseOptionHandle")


@_attrs_define
class InquiryResponseOptionHandle:
    """Provider rendering metadata for one fixed response option.

    Attributes:
        label (str):
        ref (str):
        response_handle (str):  Example: attune_irh_REDACTED.
        style (InquiryResponseOptionStyle):
    """

    label: str
    ref: str
    response_handle: str
    style: InquiryResponseOptionStyle
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        label = self.label

        ref = self.ref

        response_handle = self.response_handle

        style = self.style.value

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "label": label,
                "ref": ref,
                "response_handle": response_handle,
                "style": style,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        d = dict(src_dict)
        label = d.pop("label")

        ref = d.pop("ref")

        response_handle = d.pop("response_handle")

        style = InquiryResponseOptionStyle(d.pop("style"))

        inquiry_response_option_handle = cls(
            label=label,
            ref=ref,
            response_handle=response_handle,
            style=style,
        )

        inquiry_response_option_handle.additional_properties = d
        return inquiry_response_option_handle

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
