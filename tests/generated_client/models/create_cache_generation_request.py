from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..models.owner_type import OwnerType
from ..types import UNSET, Unset
from typing import cast






T = TypeVar("T", bound="CreateCacheGenerationRequest")



@_attrs_define
class CreateCacheGenerationRequest:
    """ Create (begin) a staging generation.

        Attributes:
            client_refresh_id (str):
            expected_active_generation_id (int | None):
            expected_chunk_count (int): Declared chunk count for contiguity validation at seal time.
            owner_type (OwnerType):
            expected_record_count (int | None | Unset):
            expected_size_bytes (int | None | Unset):
            owner_ref (None | str | Unset):
            source_revision (None | str | Unset):
     """

    client_refresh_id: str
    expected_active_generation_id: int | None
    expected_chunk_count: int
    owner_type: OwnerType
    expected_record_count: int | None | Unset = UNSET
    expected_size_bytes: int | None | Unset = UNSET
    owner_ref: None | str | Unset = UNSET
    source_revision: None | str | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)





    def to_dict(self) -> dict[str, Any]:
        client_refresh_id = self.client_refresh_id

        expected_active_generation_id: int | None
        expected_active_generation_id = self.expected_active_generation_id

        expected_chunk_count = self.expected_chunk_count

        owner_type = self.owner_type.value

        expected_record_count: int | None | Unset
        if isinstance(self.expected_record_count, Unset):
            expected_record_count = UNSET
        else:
            expected_record_count = self.expected_record_count

        expected_size_bytes: int | None | Unset
        if isinstance(self.expected_size_bytes, Unset):
            expected_size_bytes = UNSET
        else:
            expected_size_bytes = self.expected_size_bytes

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
        field_dict.update(self.additional_properties)
        field_dict.update({
            "client_refresh_id": client_refresh_id,
            "expected_active_generation_id": expected_active_generation_id,
            "expected_chunk_count": expected_chunk_count,
            "owner_type": owner_type,
        })
        if expected_record_count is not UNSET:
            field_dict["expected_record_count"] = expected_record_count
        if expected_size_bytes is not UNSET:
            field_dict["expected_size_bytes"] = expected_size_bytes
        if owner_ref is not UNSET:
            field_dict["owner_ref"] = owner_ref
        if source_revision is not UNSET:
            field_dict["source_revision"] = source_revision

        return field_dict



    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        client_refresh_id = d.pop("client_refresh_id")

        def _parse_expected_active_generation_id(data: object) -> int | None:
            if data is None:
                return data
            return cast(int | None, data)

        expected_active_generation_id = _parse_expected_active_generation_id(d.pop("expected_active_generation_id"))


        expected_chunk_count = d.pop("expected_chunk_count")

        owner_type = OwnerType(d.pop("owner_type"))




        def _parse_expected_record_count(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        expected_record_count = _parse_expected_record_count(d.pop("expected_record_count", UNSET))


        def _parse_expected_size_bytes(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        expected_size_bytes = _parse_expected_size_bytes(d.pop("expected_size_bytes", UNSET))


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


        create_cache_generation_request = cls(
            client_refresh_id=client_refresh_id,
            expected_active_generation_id=expected_active_generation_id,
            expected_chunk_count=expected_chunk_count,
            owner_type=owner_type,
            expected_record_count=expected_record_count,
            expected_size_bytes=expected_size_bytes,
            owner_ref=owner_ref,
            source_revision=source_revision,
        )


        create_cache_generation_request.additional_properties = d
        return create_cache_generation_request

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
