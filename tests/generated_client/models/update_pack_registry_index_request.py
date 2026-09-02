from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.update_pack_registry_index_request_headers_type_0 import (
        UpdatePackRegistryIndexRequestHeadersType0,
    )


T = TypeVar("T", bound="UpdatePackRegistryIndexRequest")


@_attrs_define
class UpdatePackRegistryIndexRequest:
    """Request to update a configured pack registry index.

    Attributes:
        headers (None | UpdatePackRegistryIndexRequestHeadersType0):
        enabled (bool | None | Unset):
        name (None | str | Unset):
        position (int | None | Unset):
        url (None | str | Unset):
    """

    headers: None | UpdatePackRegistryIndexRequestHeadersType0
    enabled: bool | None | Unset = UNSET
    name: None | str | Unset = UNSET
    position: int | None | Unset = UNSET
    url: None | str | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.update_pack_registry_index_request_headers_type_0 import (
            UpdatePackRegistryIndexRequestHeadersType0,
        )

        headers: dict[str, Any] | None
        if isinstance(self.headers, UpdatePackRegistryIndexRequestHeadersType0):
            headers = self.headers.to_dict()
        else:
            headers = self.headers

        enabled: bool | None | Unset
        if isinstance(self.enabled, Unset):
            enabled = UNSET
        else:
            enabled = self.enabled

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

        url: None | str | Unset
        if isinstance(self.url, Unset):
            url = UNSET
        else:
            url = self.url

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "headers": headers,
            }
        )
        if enabled is not UNSET:
            field_dict["enabled"] = enabled
        if name is not UNSET:
            field_dict["name"] = name
        if position is not UNSET:
            field_dict["position"] = position
        if url is not UNSET:
            field_dict["url"] = url

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        from ..models.update_pack_registry_index_request_headers_type_0 import (
            UpdatePackRegistryIndexRequestHeadersType0,
        )

        d = dict(src_dict)

        def _parse_headers(
            data: object,
        ) -> None | UpdatePackRegistryIndexRequestHeadersType0:
            if data is None:
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                headers_type_0 = UpdatePackRegistryIndexRequestHeadersType0.from_dict(
                    data
                )

                return headers_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(None | UpdatePackRegistryIndexRequestHeadersType0, data)

        headers = _parse_headers(d.pop("headers"))

        def _parse_enabled(data: object) -> bool | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(bool | None | Unset, data)

        enabled = _parse_enabled(d.pop("enabled", UNSET))

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

        def _parse_url(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        url = _parse_url(d.pop("url", UNSET))

        update_pack_registry_index_request = cls(
            headers=headers,
            enabled=enabled,
            name=name,
            position=position,
            url=url,
        )

        update_pack_registry_index_request.additional_properties = d
        return update_pack_registry_index_request

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
