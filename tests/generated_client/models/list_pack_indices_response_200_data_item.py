from __future__ import annotations

import datetime
from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.list_pack_indices_response_200_data_item_headers import (
        ListPackIndicesResponse200DataItemHeaders,
    )


T = TypeVar("T", bound="ListPackIndicesResponse200DataItem")


@_attrs_define
class ListPackIndicesResponse200DataItem:
    """API-managed pack registry index configuration.

    Attributes:
        created (datetime.datetime):
        enabled (bool):
        headers (ListPackIndicesResponse200DataItemHeaders):
        id (int):
        position (int):
        updated (datetime.datetime):
        url (str):
        name (None | str | Unset):
    """

    created: datetime.datetime
    enabled: bool
    headers: ListPackIndicesResponse200DataItemHeaders
    id: int
    position: int
    updated: datetime.datetime
    url: str
    name: None | str | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        created = self.created.isoformat()

        enabled = self.enabled

        headers = self.headers.to_dict()

        id = self.id

        position = self.position

        updated = self.updated.isoformat()

        url = self.url

        name: None | str | Unset
        if isinstance(self.name, Unset):
            name = UNSET
        else:
            name = self.name

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "created": created,
                "enabled": enabled,
                "headers": headers,
                "id": id,
                "position": position,
                "updated": updated,
                "url": url,
            }
        )
        if name is not UNSET:
            field_dict["name"] = name

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        from ..models.list_pack_indices_response_200_data_item_headers import (
            ListPackIndicesResponse200DataItemHeaders,
        )

        d = dict(src_dict)
        created = datetime.datetime.fromisoformat(d.pop("created"))

        enabled = d.pop("enabled")

        headers = ListPackIndicesResponse200DataItemHeaders.from_dict(d.pop("headers"))

        id = d.pop("id")

        position = d.pop("position")

        updated = datetime.datetime.fromisoformat(d.pop("updated"))

        url = d.pop("url")

        def _parse_name(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        name = _parse_name(d.pop("name", UNSET))

        list_pack_indices_response_200_data_item = cls(
            created=created,
            enabled=enabled,
            headers=headers,
            id=id,
            position=position,
            updated=updated,
            url=url,
            name=name,
        )

        list_pack_indices_response_200_data_item.additional_properties = d
        return list_pack_indices_response_200_data_item

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
