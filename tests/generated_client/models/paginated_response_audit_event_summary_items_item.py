from __future__ import annotations

import datetime
from collections.abc import Mapping
from typing import Any, TypeVar, cast
from uuid import UUID

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="PaginatedResponseAuditEventSummaryItemsItem")


@_attrs_define
class PaginatedResponseAuditEventSummaryItemsItem:
    """Compact summary of an audit event for list views.

    Attributes:
        category (str):
        created (datetime.datetime):
        event_type (str):
        id (int):
        outcome (str):
        actor_identity (int | None | Unset):
        actor_login (None | str | Unset):
        http_method (None | str | Unset):
        http_path (None | str | Unset):
        http_status (int | None | Unset):
        request_id (None | Unset | UUID):
        resource_ref (None | str | Unset):
        resource_type (None | str | Unset):
    """

    category: str
    created: datetime.datetime
    event_type: str
    id: int
    outcome: str
    actor_identity: int | None | Unset = UNSET
    actor_login: None | str | Unset = UNSET
    http_method: None | str | Unset = UNSET
    http_path: None | str | Unset = UNSET
    http_status: int | None | Unset = UNSET
    request_id: None | Unset | UUID = UNSET
    resource_ref: None | str | Unset = UNSET
    resource_type: None | str | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        category = self.category

        created = self.created.isoformat()

        event_type = self.event_type

        id = self.id

        outcome = self.outcome

        actor_identity: int | None | Unset
        if isinstance(self.actor_identity, Unset):
            actor_identity = UNSET
        else:
            actor_identity = self.actor_identity

        actor_login: None | str | Unset
        if isinstance(self.actor_login, Unset):
            actor_login = UNSET
        else:
            actor_login = self.actor_login

        http_method: None | str | Unset
        if isinstance(self.http_method, Unset):
            http_method = UNSET
        else:
            http_method = self.http_method

        http_path: None | str | Unset
        if isinstance(self.http_path, Unset):
            http_path = UNSET
        else:
            http_path = self.http_path

        http_status: int | None | Unset
        if isinstance(self.http_status, Unset):
            http_status = UNSET
        else:
            http_status = self.http_status

        request_id: None | str | Unset
        if isinstance(self.request_id, Unset):
            request_id = UNSET
        elif isinstance(self.request_id, UUID):
            request_id = str(self.request_id)
        else:
            request_id = self.request_id

        resource_ref: None | str | Unset
        if isinstance(self.resource_ref, Unset):
            resource_ref = UNSET
        else:
            resource_ref = self.resource_ref

        resource_type: None | str | Unset
        if isinstance(self.resource_type, Unset):
            resource_type = UNSET
        else:
            resource_type = self.resource_type

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "category": category,
                "created": created,
                "event_type": event_type,
                "id": id,
                "outcome": outcome,
            }
        )
        if actor_identity is not UNSET:
            field_dict["actor_identity"] = actor_identity
        if actor_login is not UNSET:
            field_dict["actor_login"] = actor_login
        if http_method is not UNSET:
            field_dict["http_method"] = http_method
        if http_path is not UNSET:
            field_dict["http_path"] = http_path
        if http_status is not UNSET:
            field_dict["http_status"] = http_status
        if request_id is not UNSET:
            field_dict["request_id"] = request_id
        if resource_ref is not UNSET:
            field_dict["resource_ref"] = resource_ref
        if resource_type is not UNSET:
            field_dict["resource_type"] = resource_type

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        category = d.pop("category")

        created = datetime.datetime.fromisoformat(d.pop("created"))

        event_type = d.pop("event_type")

        id = d.pop("id")

        outcome = d.pop("outcome")

        def _parse_actor_identity(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        actor_identity = _parse_actor_identity(d.pop("actor_identity", UNSET))

        def _parse_actor_login(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        actor_login = _parse_actor_login(d.pop("actor_login", UNSET))

        def _parse_http_method(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        http_method = _parse_http_method(d.pop("http_method", UNSET))

        def _parse_http_path(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        http_path = _parse_http_path(d.pop("http_path", UNSET))

        def _parse_http_status(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        http_status = _parse_http_status(d.pop("http_status", UNSET))

        def _parse_request_id(data: object) -> None | Unset | UUID:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                request_id_type_0 = UUID(data)

                return request_id_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(None | Unset | UUID, data)

        request_id = _parse_request_id(d.pop("request_id", UNSET))

        def _parse_resource_ref(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        resource_ref = _parse_resource_ref(d.pop("resource_ref", UNSET))

        def _parse_resource_type(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        resource_type = _parse_resource_type(d.pop("resource_type", UNSET))

        paginated_response_audit_event_summary_items_item = cls(
            category=category,
            created=created,
            event_type=event_type,
            id=id,
            outcome=outcome,
            actor_identity=actor_identity,
            actor_login=actor_login,
            http_method=http_method,
            http_path=http_path,
            http_status=http_status,
            request_id=request_id,
            resource_ref=resource_ref,
            resource_type=resource_type,
        )

        paginated_response_audit_event_summary_items_item.additional_properties = d
        return paginated_response_audit_event_summary_items_item

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
