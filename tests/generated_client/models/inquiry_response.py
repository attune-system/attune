from __future__ import annotations

import datetime
from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.inquiry_status import InquiryStatus
from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.inquiry_response_response_schema_type_0 import (
        InquiryResponseResponseSchemaType0,
    )
    from ..models.inquiry_response_response_type_0 import InquiryResponseResponseType0


T = TypeVar("T", bound="InquiryResponse")


@_attrs_define
class InquiryResponse:
    """Full inquiry response with all details

    Attributes:
        created (datetime.datetime): Creation timestamp Example: 2024-01-13T10:30:00Z.
        execution (int):
        id (int):
        prompt (str): Prompt text displayed to the user Example: Approve deployment to production?.
        response (InquiryResponseResponseType0 | None): Response data provided by the user
        response_schema (InquiryResponseResponseSchemaType0 | None): JSON schema for expected response
        status (InquiryStatus):
        updated (datetime.datetime): Last update timestamp Example: 2024-01-13T10:45:00Z.
        assigned_to (int | None | Unset):
        responded_at (datetime.datetime | None | Unset): When the inquiry was responded to Example:
            2024-01-13T10:45:00Z.
        timeout_at (datetime.datetime | None | Unset): When the inquiry expires Example: 2024-01-13T11:30:00Z.
    """

    created: datetime.datetime
    execution: int
    id: int
    prompt: str
    response: InquiryResponseResponseType0 | None
    response_schema: InquiryResponseResponseSchemaType0 | None
    status: InquiryStatus
    updated: datetime.datetime
    assigned_to: int | None | Unset = UNSET
    responded_at: datetime.datetime | None | Unset = UNSET
    timeout_at: datetime.datetime | None | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.inquiry_response_response_schema_type_0 import (
            InquiryResponseResponseSchemaType0,
        )
        from ..models.inquiry_response_response_type_0 import (
            InquiryResponseResponseType0,
        )

        created = self.created.isoformat()

        execution = self.execution

        id = self.id

        prompt = self.prompt

        response: dict[str, Any] | None
        if isinstance(self.response, InquiryResponseResponseType0):
            response = self.response.to_dict()
        else:
            response = self.response

        response_schema: dict[str, Any] | None
        if isinstance(self.response_schema, InquiryResponseResponseSchemaType0):
            response_schema = self.response_schema.to_dict()
        else:
            response_schema = self.response_schema

        status = self.status.value

        updated = self.updated.isoformat()

        assigned_to: int | None | Unset
        if isinstance(self.assigned_to, Unset):
            assigned_to = UNSET
        else:
            assigned_to = self.assigned_to

        responded_at: None | str | Unset
        if isinstance(self.responded_at, Unset):
            responded_at = UNSET
        elif isinstance(self.responded_at, datetime.datetime):
            responded_at = self.responded_at.isoformat()
        else:
            responded_at = self.responded_at

        timeout_at: None | str | Unset
        if isinstance(self.timeout_at, Unset):
            timeout_at = UNSET
        elif isinstance(self.timeout_at, datetime.datetime):
            timeout_at = self.timeout_at.isoformat()
        else:
            timeout_at = self.timeout_at

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "created": created,
                "execution": execution,
                "id": id,
                "prompt": prompt,
                "response": response,
                "response_schema": response_schema,
                "status": status,
                "updated": updated,
            }
        )
        if assigned_to is not UNSET:
            field_dict["assigned_to"] = assigned_to
        if responded_at is not UNSET:
            field_dict["responded_at"] = responded_at
        if timeout_at is not UNSET:
            field_dict["timeout_at"] = timeout_at

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.inquiry_response_response_schema_type_0 import (
            InquiryResponseResponseSchemaType0,
        )
        from ..models.inquiry_response_response_type_0 import (
            InquiryResponseResponseType0,
        )

        d = dict(src_dict)
        created = datetime.datetime.fromisoformat(d.pop("created"))

        execution = d.pop("execution")

        id = d.pop("id")

        prompt = d.pop("prompt")

        def _parse_response(data: object) -> InquiryResponseResponseType0 | None:
            if data is None:
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                response_type_0 = InquiryResponseResponseType0.from_dict(data)

                return response_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(InquiryResponseResponseType0 | None, data)

        response = _parse_response(d.pop("response"))

        def _parse_response_schema(
            data: object,
        ) -> InquiryResponseResponseSchemaType0 | None:
            if data is None:
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                response_schema_type_0 = InquiryResponseResponseSchemaType0.from_dict(
                    data
                )

                return response_schema_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(InquiryResponseResponseSchemaType0 | None, data)

        response_schema = _parse_response_schema(d.pop("response_schema"))

        status = InquiryStatus(d.pop("status"))

        updated = datetime.datetime.fromisoformat(d.pop("updated"))

        def _parse_assigned_to(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        assigned_to = _parse_assigned_to(d.pop("assigned_to", UNSET))

        def _parse_responded_at(data: object) -> datetime.datetime | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                responded_at_type_0 = datetime.datetime.fromisoformat(data)

                return responded_at_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(datetime.datetime | None | Unset, data)

        responded_at = _parse_responded_at(d.pop("responded_at", UNSET))

        def _parse_timeout_at(data: object) -> datetime.datetime | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                timeout_at_type_0 = datetime.datetime.fromisoformat(data)

                return timeout_at_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(datetime.datetime | None | Unset, data)

        timeout_at = _parse_timeout_at(d.pop("timeout_at", UNSET))

        inquiry_response = cls(
            created=created,
            execution=execution,
            id=id,
            prompt=prompt,
            response=response,
            response_schema=response_schema,
            status=status,
            updated=updated,
            assigned_to=assigned_to,
            responded_at=responded_at,
            timeout_at=timeout_at,
        )

        inquiry_response.additional_properties = d
        return inquiry_response

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
