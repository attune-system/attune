from __future__ import annotations

import datetime
from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.paginated_response_runtime_approval_response_items_item_grants_item import (
        PaginatedResponseRuntimeApprovalResponseItemsItemGrantsItem,
    )


T = TypeVar("T", bound="PaginatedResponseRuntimeApprovalResponseItemsItem")


@_attrs_define
class PaginatedResponseRuntimeApprovalResponseItemsItem:
    """
    Attributes:
        active (bool):
        approval_digest (str):
        grants (list[PaginatedResponseRuntimeApprovalResponseItemsItemGrantsItem]):
        id (int):
        issuer_identity_id (int):
        issuer_login (str):
        subject_pack_id (int):
        expires_at (datetime.datetime | None | Unset):
        revoked_at (datetime.datetime | None | Unset):
    """

    active: bool
    approval_digest: str
    grants: list[PaginatedResponseRuntimeApprovalResponseItemsItemGrantsItem]
    id: int
    issuer_identity_id: int
    issuer_login: str
    subject_pack_id: int
    expires_at: datetime.datetime | None | Unset = UNSET
    revoked_at: datetime.datetime | None | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        active = self.active

        approval_digest = self.approval_digest

        grants = []
        for grants_item_data in self.grants:
            grants_item = grants_item_data.to_dict()
            grants.append(grants_item)

        id = self.id

        issuer_identity_id = self.issuer_identity_id

        issuer_login = self.issuer_login

        subject_pack_id = self.subject_pack_id

        expires_at: None | str | Unset
        if isinstance(self.expires_at, Unset):
            expires_at = UNSET
        elif isinstance(self.expires_at, datetime.datetime):
            expires_at = self.expires_at.isoformat()
        else:
            expires_at = self.expires_at

        revoked_at: None | str | Unset
        if isinstance(self.revoked_at, Unset):
            revoked_at = UNSET
        elif isinstance(self.revoked_at, datetime.datetime):
            revoked_at = self.revoked_at.isoformat()
        else:
            revoked_at = self.revoked_at

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "active": active,
                "approval_digest": approval_digest,
                "grants": grants,
                "id": id,
                "issuer_identity_id": issuer_identity_id,
                "issuer_login": issuer_login,
                "subject_pack_id": subject_pack_id,
            }
        )
        if expires_at is not UNSET:
            field_dict["expires_at"] = expires_at
        if revoked_at is not UNSET:
            field_dict["revoked_at"] = revoked_at

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        from ..models.paginated_response_runtime_approval_response_items_item_grants_item import (
            PaginatedResponseRuntimeApprovalResponseItemsItemGrantsItem,
        )

        d = dict(src_dict)
        active = d.pop("active")

        approval_digest = d.pop("approval_digest")

        grants = []
        _grants = d.pop("grants")
        for grants_item_data in _grants:
            grants_item = (
                PaginatedResponseRuntimeApprovalResponseItemsItemGrantsItem.from_dict(
                    grants_item_data
                )
            )

            grants.append(grants_item)

        id = d.pop("id")

        issuer_identity_id = d.pop("issuer_identity_id")

        issuer_login = d.pop("issuer_login")

        subject_pack_id = d.pop("subject_pack_id")

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

        paginated_response_runtime_approval_response_items_item = cls(
            active=active,
            approval_digest=approval_digest,
            grants=grants,
            id=id,
            issuer_identity_id=issuer_identity_id,
            issuer_login=issuer_login,
            subject_pack_id=subject_pack_id,
            expires_at=expires_at,
            revoked_at=revoked_at,
        )

        paginated_response_runtime_approval_response_items_item.additional_properties = d
        return paginated_response_runtime_approval_response_items_item

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
