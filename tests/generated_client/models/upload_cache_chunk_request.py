from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

from ..models.owner_type import OwnerType
from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.cache_entry_upload import CacheEntryUpload


T = TypeVar("T", bound="UploadCacheChunkRequest")


@_attrs_define
class UploadCacheChunkRequest:
    """Upload one numbered ingest chunk.

    Attributes:
        owner_type (OwnerType):
        deleted_external_ids (list[str] | Unset): Delete these IDs from a staging clone. Missing IDs are harmless.
        entries (list[CacheEntryUpload] | Unset): Inserts for a full snapshot, or complete replacement values for a
            clone.
        owner_ref (None | str | Unset):
    """

    owner_type: OwnerType
    deleted_external_ids: list[str] | Unset = UNSET
    entries: list[CacheEntryUpload] | Unset = UNSET
    owner_ref: None | str | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        owner_type = self.owner_type.value

        deleted_external_ids: list[str] | Unset = UNSET
        if not isinstance(self.deleted_external_ids, Unset):
            deleted_external_ids = self.deleted_external_ids

        entries: list[dict[str, Any]] | Unset = UNSET
        if not isinstance(self.entries, Unset):
            entries = []
            for entries_item_data in self.entries:
                entries_item = entries_item_data.to_dict()
                entries.append(entries_item)

        owner_ref: None | str | Unset
        if isinstance(self.owner_ref, Unset):
            owner_ref = UNSET
        else:
            owner_ref = self.owner_ref

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "owner_type": owner_type,
            }
        )
        if deleted_external_ids is not UNSET:
            field_dict["deleted_external_ids"] = deleted_external_ids
        if entries is not UNSET:
            field_dict["entries"] = entries
        if owner_ref is not UNSET:
            field_dict["owner_ref"] = owner_ref

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        from ..models.cache_entry_upload import CacheEntryUpload

        d = dict(src_dict)
        owner_type = OwnerType(d.pop("owner_type"))

        deleted_external_ids = cast(list[str], d.pop("deleted_external_ids", UNSET))

        _entries = d.pop("entries", UNSET)
        entries: list[CacheEntryUpload] | Unset = UNSET
        if _entries is not UNSET:
            entries = []
            for entries_item_data in _entries:
                entries_item = CacheEntryUpload.from_dict(entries_item_data)

                entries.append(entries_item)

        def _parse_owner_ref(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        owner_ref = _parse_owner_ref(d.pop("owner_ref", UNSET))

        upload_cache_chunk_request = cls(
            owner_type=owner_type,
            deleted_external_ids=deleted_external_ids,
            entries=entries,
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
