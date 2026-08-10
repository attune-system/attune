from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..types import UNSET, Unset
from typing import cast
import datetime






T = TypeVar("T", bound="CacheEntryUpload")



@_attrs_define
class CacheEntryUpload:
    """ A record inside an upload chunk.

        Attributes:
            external_id (str):
            value (Any):
            source_checksum (None | str | Unset):
            source_updated_at (datetime.datetime | None | Unset):
     """

    external_id: str
    value: Any
    source_checksum: None | str | Unset = UNSET
    source_updated_at: datetime.datetime | None | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)





    def to_dict(self) -> dict[str, Any]:
        external_id = self.external_id

        value = self.value

        source_checksum: None | str | Unset
        if isinstance(self.source_checksum, Unset):
            source_checksum = UNSET
        else:
            source_checksum = self.source_checksum

        source_updated_at: None | str | Unset
        if isinstance(self.source_updated_at, Unset):
            source_updated_at = UNSET
        elif isinstance(self.source_updated_at, datetime.datetime):
            source_updated_at = self.source_updated_at.isoformat()
        else:
            source_updated_at = self.source_updated_at


        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update({
            "external_id": external_id,
            "value": value,
        })
        if source_checksum is not UNSET:
            field_dict["source_checksum"] = source_checksum
        if source_updated_at is not UNSET:
            field_dict["source_updated_at"] = source_updated_at

        return field_dict



    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        external_id = d.pop("external_id")

        value = d.pop("value")

        def _parse_source_checksum(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        source_checksum = _parse_source_checksum(d.pop("source_checksum", UNSET))


        def _parse_source_updated_at(data: object) -> datetime.datetime | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                source_updated_at_type_0 = datetime.datetime.fromisoformat(data)



                return source_updated_at_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(datetime.datetime | None | Unset, data)

        source_updated_at = _parse_source_updated_at(d.pop("source_updated_at", UNSET))


        cache_entry_upload = cls(
            external_id=external_id,
            value=value,
            source_checksum=source_checksum,
            source_updated_at=source_updated_at,
        )


        cache_entry_upload.additional_properties = d
        return cache_entry_upload

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
