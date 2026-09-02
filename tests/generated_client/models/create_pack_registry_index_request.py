from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.create_pack_registry_index_request_headers import (
        CreatePackRegistryIndexRequestHeaders,
    )


T = TypeVar("T", bound="CreatePackRegistryIndexRequest")


@_attrs_define
class CreatePackRegistryIndexRequest:
    """Request to add a configured pack registry index.

    Attributes:
        url (str):  Example: https://registry.example.com/attune/index.json.
        enabled (bool | Unset):  Example: True.
        headers (CreatePackRegistryIndexRequestHeaders | Unset):
        name (None | str | Unset):  Example: Attune Community.
        position (int | None | Unset): Optional explicit search order position. Omit to append to the end. Example: 0.
    """

    url: str
    enabled: bool | Unset = UNSET
    headers: CreatePackRegistryIndexRequestHeaders | Unset = UNSET
    name: None | str | Unset = UNSET
    position: int | None | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        url = self.url

        enabled = self.enabled

        headers: dict[str, Any] | Unset = UNSET
        if not isinstance(self.headers, Unset):
            headers = self.headers.to_dict()

        name: None | str | Unset
        if isinstance(self.name, Unset):
            name = UNSET
        else:
            name = self.name

        position: int | None | Unset
        if isinstance(self.position, Unset):
            position = UNSET
        else:
            position = self.position

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "url": url,
            }
        )
        if enabled is not UNSET:
            field_dict["enabled"] = enabled
        if headers is not UNSET:
            field_dict["headers"] = headers
        if name is not UNSET:
            field_dict["name"] = name
        if position is not UNSET:
            field_dict["position"] = position

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        from ..models.create_pack_registry_index_request_headers import (
            CreatePackRegistryIndexRequestHeaders,
        )

        d = dict(src_dict)
        url = d.pop("url")

        enabled = d.pop("enabled", UNSET)

        _headers = d.pop("headers", UNSET)
        headers: CreatePackRegistryIndexRequestHeaders | Unset
        if isinstance(_headers, Unset):
            headers = UNSET
        else:
            headers = CreatePackRegistryIndexRequestHeaders.from_dict(_headers)

        def _parse_name(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        name = _parse_name(d.pop("name", UNSET))

        def _parse_position(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        position = _parse_position(d.pop("position", UNSET))

        create_pack_registry_index_request = cls(
            url=url,
            enabled=enabled,
            headers=headers,
            name=name,
            position=position,
        )

        create_pack_registry_index_request.additional_properties = d
        return create_pack_registry_index_request

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
