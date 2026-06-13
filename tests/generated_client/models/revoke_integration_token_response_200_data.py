from __future__ import annotations

import datetime
from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="RevokeIntegrationTokenResponse200Data")


@_attrs_define
class RevokeIntegrationTokenResponse200Data:
    """
    Attributes:
        active (bool):
        created (datetime.datetime):
        id (int):
        identity_id (int):
        label (str):
        token_prefix (str):
        token_suffix (str):
        updated (datetime.datetime):
        created_by (int | None | Unset):
        description (None | str | Unset):
        expires_at (datetime.datetime | None | Unset):
        last_used_at (datetime.datetime | None | Unset):
        last_used_ip (None | str | Unset):
        revocation_reason (None | str | Unset):
        revoked_at (datetime.datetime | None | Unset):
        revoked_by (int | None | Unset):
    """

    active: bool
    created: datetime.datetime
    id: int
    identity_id: int
    label: str
    token_prefix: str
    token_suffix: str
    updated: datetime.datetime
    created_by: int | None | Unset = UNSET
    description: None | str | Unset = UNSET
    expires_at: datetime.datetime | None | Unset = UNSET
    last_used_at: datetime.datetime | None | Unset = UNSET
    last_used_ip: None | str | Unset = UNSET
    revocation_reason: None | str | Unset = UNSET
    revoked_at: datetime.datetime | None | Unset = UNSET
    revoked_by: int | None | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        active = self.active

        created = self.created.isoformat()

        id = self.id

        identity_id = self.identity_id

        label = self.label

        token_prefix = self.token_prefix

        token_suffix = self.token_suffix

        updated = self.updated.isoformat()

        created_by: int | None | Unset
        if isinstance(self.created_by, Unset):
            created_by = UNSET
        else:
            created_by = self.created_by

        description: None | str | Unset
        if isinstance(self.description, Unset):
            description = UNSET
        else:
            description = self.description

        expires_at: None | str | Unset
        if isinstance(self.expires_at, Unset):
            expires_at = UNSET
        elif isinstance(self.expires_at, datetime.datetime):
            expires_at = self.expires_at.isoformat()
        else:
            expires_at = self.expires_at

        last_used_at: None | str | Unset
        if isinstance(self.last_used_at, Unset):
            last_used_at = UNSET
        elif isinstance(self.last_used_at, datetime.datetime):
            last_used_at = self.last_used_at.isoformat()
        else:
            last_used_at = self.last_used_at

        last_used_ip: None | str | Unset
        if isinstance(self.last_used_ip, Unset):
            last_used_ip = UNSET
        else:
            last_used_ip = self.last_used_ip

        revocation_reason: None | str | Unset
        if isinstance(self.revocation_reason, Unset):
            revocation_reason = UNSET
        else:
            revocation_reason = self.revocation_reason

        revoked_at: None | str | Unset
        if isinstance(self.revoked_at, Unset):
            revoked_at = UNSET
        elif isinstance(self.revoked_at, datetime.datetime):
            revoked_at = self.revoked_at.isoformat()
        else:
            revoked_at = self.revoked_at

        revoked_by: int | None | Unset
        if isinstance(self.revoked_by, Unset):
            revoked_by = UNSET
        else:
            revoked_by = self.revoked_by

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "active": active,
                "created": created,
                "id": id,
                "identity_id": identity_id,
                "label": label,
                "token_prefix": token_prefix,
                "token_suffix": token_suffix,
                "updated": updated,
            }
        )
        if created_by is not UNSET:
            field_dict["created_by"] = created_by
        if description is not UNSET:
            field_dict["description"] = description
        if expires_at is not UNSET:
            field_dict["expires_at"] = expires_at
        if last_used_at is not UNSET:
            field_dict["last_used_at"] = last_used_at
        if last_used_ip is not UNSET:
            field_dict["last_used_ip"] = last_used_ip
        if revocation_reason is not UNSET:
            field_dict["revocation_reason"] = revocation_reason
        if revoked_at is not UNSET:
            field_dict["revoked_at"] = revoked_at
        if revoked_by is not UNSET:
            field_dict["revoked_by"] = revoked_by

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        active = d.pop("active")

        created = datetime.datetime.fromisoformat(d.pop("created"))

        id = d.pop("id")

        identity_id = d.pop("identity_id")

        label = d.pop("label")

        token_prefix = d.pop("token_prefix")

        token_suffix = d.pop("token_suffix")

        updated = datetime.datetime.fromisoformat(d.pop("updated"))

        def _parse_created_by(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        created_by = _parse_created_by(d.pop("created_by", UNSET))

        def _parse_description(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        description = _parse_description(d.pop("description", UNSET))

        def _parse_expires_at(data: object) -> datetime.datetime | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                expires_at_type_0 = datetime.datetime.fromisoformat(data)

                return expires_at_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(datetime.datetime | None | Unset, data)

        expires_at = _parse_expires_at(d.pop("expires_at", UNSET))

        def _parse_last_used_at(data: object) -> datetime.datetime | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                last_used_at_type_0 = datetime.datetime.fromisoformat(data)

                return last_used_at_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(datetime.datetime | None | Unset, data)

        last_used_at = _parse_last_used_at(d.pop("last_used_at", UNSET))

        def _parse_last_used_ip(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        last_used_ip = _parse_last_used_ip(d.pop("last_used_ip", UNSET))

        def _parse_revocation_reason(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        revocation_reason = _parse_revocation_reason(d.pop("revocation_reason", UNSET))

        def _parse_revoked_at(data: object) -> datetime.datetime | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                revoked_at_type_0 = datetime.datetime.fromisoformat(data)

                return revoked_at_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(datetime.datetime | None | Unset, data)

        revoked_at = _parse_revoked_at(d.pop("revoked_at", UNSET))

        def _parse_revoked_by(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        revoked_by = _parse_revoked_by(d.pop("revoked_by", UNSET))

        revoke_integration_token_response_200_data = cls(
            active=active,
            created=created,
            id=id,
            identity_id=identity_id,
            label=label,
            token_prefix=token_prefix,
            token_suffix=token_suffix,
            updated=updated,
            created_by=created_by,
            description=description,
            expires_at=expires_at,
            last_used_at=last_used_at,
            last_used_ip=last_used_ip,
            revocation_reason=revocation_reason,
            revoked_at=revoked_at,
            revoked_by=revoked_by,
        )

        revoke_integration_token_response_200_data.additional_properties = d
        return revoke_integration_token_response_200_data

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
