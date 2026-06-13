from __future__ import annotations

import datetime
from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast
from uuid import UUID

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.api_response_vec_audit_event_response_data_item_correlation_chain_type_0 import (
        ApiResponseVecAuditEventResponseDataItemCorrelationChainType0,
    )
    from ..models.api_response_vec_audit_event_response_data_item_details_type_0 import (
        ApiResponseVecAuditEventResponseDataItemDetailsType0,
    )


T = TypeVar("T", bound="ApiResponseVecAuditEventResponseDataItem")


@_attrs_define
class ApiResponseVecAuditEventResponseDataItem:
    """Full audit event with all fields.

    Attributes:
        category (str): High-level category. Example: auth.
        correlation_chain (ApiResponseVecAuditEventResponseDataItemCorrelationChainType0 | None): Optional cascade chain
            (rule_id, enforcement_id, execution_id, …).
        created (datetime.datetime): Event creation timestamp. Example: 2024-01-13T10:30:00Z.
        details (ApiResponseVecAuditEventResponseDataItemDetailsType0 | None): Event-specific structured metadata.
            Secrets are redacted.
        event_type (str): Dotted event-type identifier (e.g., `auth.login.success`). Example: auth.login.success.
        id (int):
        outcome (str): Outcome (`success`, `failure`, or `denied`). Example: success.
        actor_identity (int | None | Unset):
        actor_ip (None | str | Unset): Source IP of the request.
        actor_login (None | str | Unset): Snapshot of `identity.login` at time of the event.
        actor_token_type (None | str | Unset): Token type (`access`, `execution`, `sensor`, `refresh`).
        actor_user_agent (None | str | Unset): User-Agent header from the request.
        duration_ms (int | None | Unset): Request duration in milliseconds.
        http_method (None | str | Unset): HTTP method (NULL for non-API events).
        http_path (None | str | Unset): HTTP path.
        http_status (int | None | Unset): HTTP status code.
        request_id (None | Unset | UUID): Correlation request ID assigned by the API middleware.
        resource_id (int | None | Unset):
        resource_ref (None | str | Unset): Resource reference snapshot (forensic).
        resource_type (None | str | Unset): Logical resource type (e.g., `pack`, `key`, `execution`).
    """

    category: str
    correlation_chain: (
        ApiResponseVecAuditEventResponseDataItemCorrelationChainType0 | None
    )
    created: datetime.datetime
    details: ApiResponseVecAuditEventResponseDataItemDetailsType0 | None
    event_type: str
    id: int
    outcome: str
    actor_identity: int | None | Unset = UNSET
    actor_ip: None | str | Unset = UNSET
    actor_login: None | str | Unset = UNSET
    actor_token_type: None | str | Unset = UNSET
    actor_user_agent: None | str | Unset = UNSET
    duration_ms: int | None | Unset = UNSET
    http_method: None | str | Unset = UNSET
    http_path: None | str | Unset = UNSET
    http_status: int | None | Unset = UNSET
    request_id: None | Unset | UUID = UNSET
    resource_id: int | None | Unset = UNSET
    resource_ref: None | str | Unset = UNSET
    resource_type: None | str | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.api_response_vec_audit_event_response_data_item_correlation_chain_type_0 import (
            ApiResponseVecAuditEventResponseDataItemCorrelationChainType0,
        )
        from ..models.api_response_vec_audit_event_response_data_item_details_type_0 import (
            ApiResponseVecAuditEventResponseDataItemDetailsType0,
        )

        category = self.category

        correlation_chain: dict[str, Any] | None
        if isinstance(
            self.correlation_chain,
            ApiResponseVecAuditEventResponseDataItemCorrelationChainType0,
        ):
            correlation_chain = self.correlation_chain.to_dict()
        else:
            correlation_chain = self.correlation_chain

        created = self.created.isoformat()

        details: dict[str, Any] | None
        if isinstance(
            self.details, ApiResponseVecAuditEventResponseDataItemDetailsType0
        ):
            details = self.details.to_dict()
        else:
            details = self.details

        event_type = self.event_type

        id = self.id

        outcome = self.outcome

        actor_identity: int | None | Unset
        if isinstance(self.actor_identity, Unset):
            actor_identity = UNSET
        else:
            actor_identity = self.actor_identity

        actor_ip: None | str | Unset
        if isinstance(self.actor_ip, Unset):
            actor_ip = UNSET
        else:
            actor_ip = self.actor_ip

        actor_login: None | str | Unset
        if isinstance(self.actor_login, Unset):
            actor_login = UNSET
        else:
            actor_login = self.actor_login

        actor_token_type: None | str | Unset
        if isinstance(self.actor_token_type, Unset):
            actor_token_type = UNSET
        else:
            actor_token_type = self.actor_token_type

        actor_user_agent: None | str | Unset
        if isinstance(self.actor_user_agent, Unset):
            actor_user_agent = UNSET
        else:
            actor_user_agent = self.actor_user_agent

        duration_ms: int | None | Unset
        if isinstance(self.duration_ms, Unset):
            duration_ms = UNSET
        else:
            duration_ms = self.duration_ms

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

        resource_id: int | None | Unset
        if isinstance(self.resource_id, Unset):
            resource_id = UNSET
        else:
            resource_id = self.resource_id

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
                "correlation_chain": correlation_chain,
                "created": created,
                "details": details,
                "event_type": event_type,
                "id": id,
                "outcome": outcome,
            }
        )
        if actor_identity is not UNSET:
            field_dict["actor_identity"] = actor_identity
        if actor_ip is not UNSET:
            field_dict["actor_ip"] = actor_ip
        if actor_login is not UNSET:
            field_dict["actor_login"] = actor_login
        if actor_token_type is not UNSET:
            field_dict["actor_token_type"] = actor_token_type
        if actor_user_agent is not UNSET:
            field_dict["actor_user_agent"] = actor_user_agent
        if duration_ms is not UNSET:
            field_dict["duration_ms"] = duration_ms
        if http_method is not UNSET:
            field_dict["http_method"] = http_method
        if http_path is not UNSET:
            field_dict["http_path"] = http_path
        if http_status is not UNSET:
            field_dict["http_status"] = http_status
        if request_id is not UNSET:
            field_dict["request_id"] = request_id
        if resource_id is not UNSET:
            field_dict["resource_id"] = resource_id
        if resource_ref is not UNSET:
            field_dict["resource_ref"] = resource_ref
        if resource_type is not UNSET:
            field_dict["resource_type"] = resource_type

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.api_response_vec_audit_event_response_data_item_correlation_chain_type_0 import (
            ApiResponseVecAuditEventResponseDataItemCorrelationChainType0,
        )
        from ..models.api_response_vec_audit_event_response_data_item_details_type_0 import (
            ApiResponseVecAuditEventResponseDataItemDetailsType0,
        )

        d = dict(src_dict)
        category = d.pop("category")

        def _parse_correlation_chain(
            data: object,
        ) -> ApiResponseVecAuditEventResponseDataItemCorrelationChainType0 | None:
            if data is None:
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                correlation_chain_type_0 = ApiResponseVecAuditEventResponseDataItemCorrelationChainType0.from_dict(
                    data
                )

                return correlation_chain_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(
                ApiResponseVecAuditEventResponseDataItemCorrelationChainType0 | None,
                data,
            )

        correlation_chain = _parse_correlation_chain(d.pop("correlation_chain"))

        created = datetime.datetime.fromisoformat(d.pop("created"))

        def _parse_details(
            data: object,
        ) -> ApiResponseVecAuditEventResponseDataItemDetailsType0 | None:
            if data is None:
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                details_type_0 = (
                    ApiResponseVecAuditEventResponseDataItemDetailsType0.from_dict(data)
                )

                return details_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(
                ApiResponseVecAuditEventResponseDataItemDetailsType0 | None, data
            )

        details = _parse_details(d.pop("details"))

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

        def _parse_actor_ip(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        actor_ip = _parse_actor_ip(d.pop("actor_ip", UNSET))

        def _parse_actor_login(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        actor_login = _parse_actor_login(d.pop("actor_login", UNSET))

        def _parse_actor_token_type(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        actor_token_type = _parse_actor_token_type(d.pop("actor_token_type", UNSET))

        def _parse_actor_user_agent(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        actor_user_agent = _parse_actor_user_agent(d.pop("actor_user_agent", UNSET))

        def _parse_duration_ms(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        duration_ms = _parse_duration_ms(d.pop("duration_ms", UNSET))

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

        def _parse_resource_id(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        resource_id = _parse_resource_id(d.pop("resource_id", UNSET))

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

        api_response_vec_audit_event_response_data_item = cls(
            category=category,
            correlation_chain=correlation_chain,
            created=created,
            details=details,
            event_type=event_type,
            id=id,
            outcome=outcome,
            actor_identity=actor_identity,
            actor_ip=actor_ip,
            actor_login=actor_login,
            actor_token_type=actor_token_type,
            actor_user_agent=actor_user_agent,
            duration_ms=duration_ms,
            http_method=http_method,
            http_path=http_path,
            http_status=http_status,
            request_id=request_id,
            resource_id=resource_id,
            resource_ref=resource_ref,
            resource_type=resource_type,
        )

        api_response_vec_audit_event_response_data_item.additional_properties = d
        return api_response_vec_audit_event_response_data_item

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
