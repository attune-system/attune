from __future__ import annotations

import datetime
from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

from ..models.dashboard_scope_type import DashboardScopeType
from ..models.dashboard_visibility import DashboardVisibility
from ..types import UNSET, Unset

T = TypeVar("T", bound="ApiResponseVecDashboardListItemResponseDataItem")


@_attrs_define
class ApiResponseVecDashboardListItemResponseDataItem:
    """
    Attributes:
        id (int):
        is_default_home (bool):
        label (str):
        ref (str):
        revision (int):
        scope_ref (str):
        scope_type (DashboardScopeType):
        tags (list[str]):
        updated (datetime.datetime):
        visibility (DashboardVisibility):
        description (None | str | Unset):
        retired_at (datetime.datetime | None | Unset):
    """

    id: int
    is_default_home: bool
    label: str
    ref: str
    revision: int
    scope_ref: str
    scope_type: DashboardScopeType
    tags: list[str]
    updated: datetime.datetime
    visibility: DashboardVisibility
    description: None | str | Unset = UNSET
    retired_at: datetime.datetime | None | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        id = self.id

        is_default_home = self.is_default_home

        label = self.label

        ref = self.ref

        revision = self.revision

        scope_ref = self.scope_ref

        scope_type = self.scope_type.value

        tags = self.tags

        updated = self.updated.isoformat()

        visibility = self.visibility.value

        description: None | str | Unset
        if isinstance(self.description, Unset):
            description = UNSET
        else:
            description = self.description

        retired_at: None | str | Unset
        if isinstance(self.retired_at, Unset):
            retired_at = UNSET
        elif isinstance(self.retired_at, datetime.datetime):
            retired_at = self.retired_at.isoformat()
        else:
            retired_at = self.retired_at

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "id": id,
                "is_default_home": is_default_home,
                "label": label,
                "ref": ref,
                "revision": revision,
                "scope_ref": scope_ref,
                "scope_type": scope_type,
                "tags": tags,
                "updated": updated,
                "visibility": visibility,
            }
        )
        if description is not UNSET:
            field_dict["description"] = description
        if retired_at is not UNSET:
            field_dict["retired_at"] = retired_at

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        d = dict(src_dict)
        id = d.pop("id")

        is_default_home = d.pop("is_default_home")

        label = d.pop("label")

        ref = d.pop("ref")

        revision = d.pop("revision")

        scope_ref = d.pop("scope_ref")

        scope_type = DashboardScopeType(d.pop("scope_type"))

        tags = cast(list[str], d.pop("tags"))

        updated = datetime.datetime.fromisoformat(d.pop("updated"))

        visibility = DashboardVisibility(d.pop("visibility"))

        def _parse_description(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        description = _parse_description(d.pop("description", UNSET))

        def _parse_retired_at(data: object) -> datetime.datetime | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                retired_at_type_0 = datetime.datetime.fromisoformat(data)

                return retired_at_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(datetime.datetime | None | Unset, data)

        retired_at = _parse_retired_at(d.pop("retired_at", UNSET))

        api_response_vec_dashboard_list_item_response_data_item = cls(
            id=id,
            is_default_home=is_default_home,
            label=label,
            ref=ref,
            revision=revision,
            scope_ref=scope_ref,
            scope_type=scope_type,
            tags=tags,
            updated=updated,
            visibility=visibility,
            description=description,
            retired_at=retired_at,
        )

        api_response_vec_dashboard_list_item_response_data_item.additional_properties = d
        return api_response_vec_dashboard_list_item_response_data_item

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
