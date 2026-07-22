from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..models.owner_type import OwnerType
from ..types import UNSET, Unset
from typing import cast

if TYPE_CHECKING:
  from ..models.cache_entry_upload import CacheEntryUpload





T = TypeVar("T", bound="UploadCacheChunkRequest")



@_attrs_define
class UploadCacheChunkRequest:
    """ Upload one numbered ingest chunk.

        Attributes:
            entries (list[CacheEntryUpload]):
            owner_type (OwnerType):
            owner_ref (None | str | Unset):
     """

    entries: list[CacheEntryUpload]
    owner_type: OwnerType
    owner_ref: None | str | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)





    def to_dict(self) -> dict[str, Any]:
        from ..models.cache_entry_upload import CacheEntryUpload
        entries = []
        for entries_item_data in self.entries:
            entries_item = entries_item_data.to_dict()
            entries.append(entries_item)



        owner_type = self.owner_type.value

        owner_ref: None | str | Unset
        if isinstance(self.owner_ref, Unset):
            owner_ref = UNSET
        else:
            owner_ref = self.owner_ref


        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update({
            "entries": entries,
            "owner_type": owner_type,
        })
        if owner_ref is not UNSET:
            field_dict["owner_ref"] = owner_ref

        return field_dict



    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.cache_entry_upload import CacheEntryUpload
        d = dict(src_dict)
        entries = []
        _entries = d.pop("entries")
        for entries_item_data in (_entries):
            entries_item = CacheEntryUpload.from_dict(entries_item_data)



            entries.append(entries_item)


        owner_type = OwnerType(d.pop("owner_type"))




        def _parse_owner_ref(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        owner_ref = _parse_owner_ref(d.pop("owner_ref", UNSET))


        upload_cache_chunk_request = cls(
            entries=entries,
            owner_type=owner_type,
            owner_ref=owner_ref,
        )


        upload_cache_chunk_request.additional_properties = d
        return upload_cache_chunk_request

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
