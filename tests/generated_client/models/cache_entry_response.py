from __future__ import annotations

import datetime
from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

T = TypeVar("T", bound="CacheEntryResponse")


@_attrs_define
class CacheEntryResponse:
    """A single cache record. Extra descriptive fields beyond `external_id`/`value`
    are ignored by minimal clients.

        Attributes:
            external_id (str):
            size_bytes (int):
            source_checksum (None | str):
            source_updated_at (datetime.datetime | None):
            value (Any):
    """

    external_id: str
    size_bytes: int
    source_checksum: None | str
    source_updated_at: datetime.datetime | None
    value: Any
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        external_id = self.external_id

        size_bytes = self.size_bytes

        source_checksum: None | str
        source_checksum = self.source_checksum

        source_updated_at: None | str
        if isinstance(self.source_updated_at, datetime.datetime):
            source_updated_at = self.source_updated_at.isoformat()
        else:
            source_updated_at = self.source_updated_at

        value = self.value

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "external_id": external_id,
                "size_bytes": size_bytes,
                "source_checksum": source_checksum,
                "source_updated_at": source_updated_at,
                "value": value,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        d = dict(src_dict)
        external_id = d.pop("external_id")

        size_bytes = d.pop("size_bytes")

        def _parse_source_checksum(data: object) -> None | str:
            if data is None:
                return data
            return cast(None | str, data)

        source_checksum = _parse_source_checksum(d.pop("source_checksum"))

        def _parse_source_updated_at(data: object) -> datetime.datetime | None:
            if data is None:
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                source_updated_at_type_0 = datetime.datetime.fromisoformat(data)

                return source_updated_at_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(datetime.datetime | None, data)

        source_updated_at = _parse_source_updated_at(d.pop("source_updated_at"))

        value = d.pop("value")

        cache_entry_response = cls(
            external_id=external_id,
            size_bytes=size_bytes,
            source_checksum=source_checksum,
            source_updated_at=source_updated_at,
            value=value,
        )

        cache_entry_response.additional_properties = d
        return cache_entry_response

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
