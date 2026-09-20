from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar

from attrs import define as _attrs_define
from typing_extensions import Self

if TYPE_CHECKING:
    from ..models.external_actor_assertion import ExternalActorAssertion
    from ..models.external_inquiry_respond_request_response import (
        ExternalInquiryRespondRequestResponse,
    )


T = TypeVar("T", bound="ExternalInquiryRespondRequest")


@_attrs_define
class ExternalInquiryRespondRequest:
    """Provider-neutral one-shot response submitted by an integration adapter.

    Attributes:
        external_actor (ExternalActorAssertion): External actor asserted by an authenticated integration adapter.
        response (ExternalInquiryRespondRequestResponse):
        response_handle (str):
    """

    external_actor: ExternalActorAssertion
    response: ExternalInquiryRespondRequestResponse
    response_handle: str

    def to_dict(self) -> dict[str, Any]:
        external_actor = self.external_actor.to_dict()

        response = self.response.to_dict()

        response_handle = self.response_handle

        field_dict: dict[str, Any] = {}

        field_dict.update(
            {
                "external_actor": external_actor,
                "response": response,
                "response_handle": response_handle,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        from ..models.external_actor_assertion import (
            ExternalActorAssertion,
        )
        from ..models.external_inquiry_respond_request_response import (
            ExternalInquiryRespondRequestResponse,
        )

        d = dict(src_dict)
        external_actor = ExternalActorAssertion.from_dict(d.pop("external_actor"))

        response = ExternalInquiryRespondRequestResponse.from_dict(d.pop("response"))

        response_handle = d.pop("response_handle")

        external_inquiry_respond_request = cls(
            external_actor=external_actor,
            response=response,
            response_handle=response_handle,
        )

        return external_inquiry_respond_request
