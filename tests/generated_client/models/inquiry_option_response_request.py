from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define
from typing_extensions import Self

T = TypeVar("T", bound="InquiryOptionResponseRequest")


@_attrs_define
class InquiryOptionResponseRequest:
    """
    Attributes:
        option_ref (str): Select the stored option without exposing its protected response value.
    """

    option_ref: str

    def to_dict(self) -> dict[str, Any]:
        option_ref = self.option_ref

        field_dict: dict[str, Any] = {}

        field_dict.update(
            {
                "option_ref": option_ref,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        d = dict(src_dict)
        option_ref = d.pop("option_ref")

        inquiry_option_response_request = cls(
            option_ref=option_ref,
        )

        return inquiry_option_response_request
