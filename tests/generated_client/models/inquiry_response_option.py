from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar

from attrs import define as _attrs_define
from typing_extensions import Self

from ..models.inquiry_response_option_style import InquiryResponseOptionStyle

if TYPE_CHECKING:
    from ..models.inquiry_response_option_response import InquiryResponseOptionResponse


T = TypeVar("T", bound="InquiryResponseOption")


@_attrs_define
class InquiryResponseOption:
    """
    Attributes:
        label (str):
        ref (str):
        response (InquiryResponseOptionResponse):
        style (InquiryResponseOptionStyle):
    """

    label: str
    ref: str
    response: InquiryResponseOptionResponse
    style: InquiryResponseOptionStyle

    def to_dict(self) -> dict[str, Any]:
        label = self.label

        ref = self.ref

        response = self.response.to_dict()

        style = self.style.value

        field_dict: dict[str, Any] = {}

        field_dict.update(
            {
                "label": label,
                "ref": ref,
                "response": response,
                "style": style,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        from ..models.inquiry_response_option_response import (
            InquiryResponseOptionResponse,
        )

        d = dict(src_dict)
        label = d.pop("label")

        ref = d.pop("ref")

        response = InquiryResponseOptionResponse.from_dict(d.pop("response"))

        style = InquiryResponseOptionStyle(d.pop("style"))

        inquiry_response_option = cls(
            label=label,
            ref=ref,
            response=response,
            style=style,
        )

        return inquiry_response_option
