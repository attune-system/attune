from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar

from attrs import define as _attrs_define
from typing_extensions import Self

if TYPE_CHECKING:
    from ..models.inquiry_literal_response_request_response import (
        InquiryLiteralResponseRequestResponse,
    )


T = TypeVar("T", bound="InquiryLiteralResponseRequest")


@_attrs_define
class InquiryLiteralResponseRequest:
    """
    Attributes:
        response (InquiryLiteralResponseRequestResponse): Response data conforming to the inquiry's response_schema
    """

    response: InquiryLiteralResponseRequestResponse

    def to_dict(self) -> dict[str, Any]:
        response = self.response.to_dict()

        field_dict: dict[str, Any] = {}

        field_dict.update(
            {
                "response": response,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        from ..models.inquiry_literal_response_request_response import (
            InquiryLiteralResponseRequestResponse,
        )

        d = dict(src_dict)
        response = InquiryLiteralResponseRequestResponse.from_dict(d.pop("response"))

        inquiry_literal_response_request = cls(
            response=response,
        )

        return inquiry_literal_response_request
