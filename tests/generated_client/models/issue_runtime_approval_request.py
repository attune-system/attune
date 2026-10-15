from __future__ import annotations

import datetime
from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from typing_extensions import Self

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.issue_runtime_approval_request_grants_item import (
        IssueRuntimeApprovalRequestGrantsItem,
    )


T = TypeVar("T", bound="IssueRuntimeApprovalRequest")


@_attrs_define
class IssueRuntimeApprovalRequest:
    """
    Attributes:
        grants (list[IssueRuntimeApprovalRequestGrantsItem]): Fixed approved grants. Human-relative scopes are not
            accepted.
        expires_at (datetime.datetime | None | Unset): Omit for no automatic expiry. Explicit revocation remains
            available.
    """

    grants: list[IssueRuntimeApprovalRequestGrantsItem]
    expires_at: datetime.datetime | None | Unset = UNSET

    def to_dict(self) -> dict[str, Any]:
        grants = []
        for grants_item_data in self.grants:
            grants_item = grants_item_data.to_dict()
            grants.append(grants_item)

        expires_at: None | str | Unset
        if isinstance(self.expires_at, Unset):
            expires_at = UNSET
        elif isinstance(self.expires_at, datetime.datetime):
            expires_at = self.expires_at.isoformat()
        else:
            expires_at = self.expires_at

        field_dict: dict[str, Any] = {}

        field_dict.update(
            {
                "grants": grants,
            }
        )
        if expires_at is not UNSET:
            field_dict["expires_at"] = expires_at

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        from ..models.issue_runtime_approval_request_grants_item import (
            IssueRuntimeApprovalRequestGrantsItem,
        )

        d = dict(src_dict)
        grants = []
        _grants = d.pop("grants")
        for grants_item_data in _grants:
            grants_item = IssueRuntimeApprovalRequestGrantsItem.from_dict(
                grants_item_data
            )

            grants.append(grants_item)

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

        issue_runtime_approval_request = cls(
            grants=grants,
            expires_at=expires_at,
        )

        return issue_runtime_approval_request
