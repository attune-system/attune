from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.inquiry_status import InquiryStatus
from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.update_inquiry_request_response_type_0 import (
        UpdateInquiryRequestResponseType0,
    )


T = TypeVar("T", bound="UpdateInquiryRequest")


@_attrs_define
class UpdateInquiryRequest:
    """Request to update an inquiry

    Attributes:
        response (None | UpdateInquiryRequestResponseType0): Update the response data
        assigned_to (int | None | Unset):
        status (InquiryStatus | None | Unset):
    """

    response: None | UpdateInquiryRequestResponseType0
    assigned_to: int | None | Unset = UNSET
    status: InquiryStatus | None | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.update_inquiry_request_response_type_0 import (
            UpdateInquiryRequestResponseType0,
        )

        response: dict[str, Any] | None
        if isinstance(self.response, UpdateInquiryRequestResponseType0):
            response = self.response.to_dict()
        else:
            response = self.response

        assigned_to: int | None | Unset
        if isinstance(self.assigned_to, Unset):
            assigned_to = UNSET
        else:
            assigned_to = self.assigned_to

        status: None | str | Unset
        if isinstance(self.status, Unset):
            status = UNSET
        elif isinstance(self.status, InquiryStatus):
            status = self.status.value
        else:
            status = self.status

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "response": response,
            }
        )
        if assigned_to is not UNSET:
            field_dict["assigned_to"] = assigned_to
        if status is not UNSET:
            field_dict["status"] = status

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.update_inquiry_request_response_type_0 import (
            UpdateInquiryRequestResponseType0,
        )

        d = dict(src_dict)

        def _parse_response(data: object) -> None | UpdateInquiryRequestResponseType0:
            if data is None:
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                response_type_0 = UpdateInquiryRequestResponseType0.from_dict(data)

                return response_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(None | UpdateInquiryRequestResponseType0, data)

        response = _parse_response(d.pop("response"))

        def _parse_assigned_to(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        assigned_to = _parse_assigned_to(d.pop("assigned_to", UNSET))

        def _parse_status(data: object) -> InquiryStatus | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                status_type_1 = InquiryStatus(data)

                return status_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(InquiryStatus | None | Unset, data)

        status = _parse_status(d.pop("status", UNSET))

        update_inquiry_request = cls(
            response=response,
            assigned_to=assigned_to,
            status=status,
        )

        update_inquiry_request.additional_properties = d
        return update_inquiry_request

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
