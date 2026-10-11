from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from typing_extensions import Self

from ..models.owner_type import OwnerType
from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.cache_entry_upload import CacheEntryUpload


T = TypeVar("T", bound="ApplyLiveCacheRequest")


@_attrs_define
class ApplyLiveCacheRequest:
    """Atomic mutations to a durable live namespace. No snapshot lifecycle is used.

    Attributes:
        expected_revision (int): Required optimistic guard. Use zero for the first batch.
        owner_type (OwnerType):
        request_id (str):
        deleted_external_ids (list[str] | Unset):
        entries (list[CacheEntryUpload] | Unset):
        owner_ref (None | str | Unset):
        source_revision (None | str | Unset): Omission or null retains the current upstream cursor.
    """

    expected_revision: int
    owner_type: OwnerType
    request_id: str
    deleted_external_ids: list[str] | Unset = UNSET
    entries: list[CacheEntryUpload] | Unset = UNSET
    owner_ref: None | str | Unset = UNSET
    source_revision: None | str | Unset = UNSET

    def to_dict(self) -> dict[str, Any]:
        expected_revision = self.expected_revision

        owner_type = self.owner_type.value

        request_id = self.request_id

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

        source_revision: None | str | Unset
        if isinstance(self.source_revision, Unset):
            source_revision = UNSET
        else:
            source_revision = self.source_revision

        field_dict: dict[str, Any] = {}

        field_dict.update(
            {
                "expected_revision": expected_revision,
                "owner_type": owner_type,
                "request_id": request_id,
            }
        )
        if deleted_external_ids is not UNSET:
            field_dict["deleted_external_ids"] = deleted_external_ids
        if entries is not UNSET:
            field_dict["entries"] = entries
        if owner_ref is not UNSET:
            field_dict["owner_ref"] = owner_ref
        if source_revision is not UNSET:
            field_dict["source_revision"] = source_revision

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        from ..models.cache_entry_upload import CacheEntryUpload

        d = dict(src_dict)
        expected_revision = d.pop("expected_revision")

        owner_type = OwnerType(d.pop("owner_type"))

        request_id = d.pop("request_id")

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

        def _parse_source_revision(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        source_revision = _parse_source_revision(d.pop("source_revision", UNSET))

        apply_live_cache_request = cls(
            expected_revision=expected_revision,
            owner_type=owner_type,
            request_id=request_id,
            deleted_external_ids=deleted_external_ids,
            entries=entries,
            owner_ref=owner_ref,
            source_revision=source_revision,
        )

        return apply_live_cache_request
