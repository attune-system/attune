from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="PaginatedResponseIdentitySummaryItemsItem")


@_attrs_define
class PaginatedResponseIdentitySummaryItemsItem:
    """
    Attributes:
        attributes (Any):
        frozen (bool):
        id (int):
        login (str):
        roles (list[str]):
        display_name (None | str | Unset):
    """

    attributes: Any
    frozen: bool
    id: int
    login: str
    roles: list[str]
    display_name: None | str | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        attributes = self.attributes

        frozen = self.frozen

        id = self.id

        login = self.login

        roles = self.roles

        display_name: None | str | Unset
        if isinstance(self.display_name, Unset):
            display_name = UNSET
        else:
            display_name = self.display_name

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "attributes": attributes,
                "frozen": frozen,
                "id": id,
                "login": login,
                "roles": roles,
            }
        )
        if display_name is not UNSET:
            field_dict["display_name"] = display_name

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        attributes = d.pop("attributes")

        frozen = d.pop("frozen")

        id = d.pop("id")

        login = d.pop("login")

        roles = cast(list[str], d.pop("roles"))

        def _parse_display_name(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        display_name = _parse_display_name(d.pop("display_name", UNSET))

        paginated_response_identity_summary_items_item = cls(
            attributes=attributes,
            frozen=frozen,
            id=id,
            login=login,
            roles=roles,
            display_name=display_name,
        )

        paginated_response_identity_summary_items_item.additional_properties = d
        return paginated_response_identity_summary_items_item

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
