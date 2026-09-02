from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

from ..models.owner_type import OwnerType
from ..types import UNSET, Unset

T = TypeVar("T", bound="SealCacheGenerationRequest")


@_attrs_define
class SealCacheGenerationRequest:
    """Seal a staging generation into `ready`.

    Attributes:
        expected_chunk_count (int):
        owner_type (OwnerType):
        expected_record_count (int | None | Unset):
        expected_size_bytes (int | None | Unset):
        owner_ref (None | str | Unset):
    """

    expected_chunk_count: int
    owner_type: OwnerType
    expected_record_count: int | None | Unset = UNSET
    expected_size_bytes: int | None | Unset = UNSET
    owner_ref: None | str | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
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

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "expected_chunk_count": expected_chunk_count,
                "owner_type": owner_type,
            }
        )
        if expected_record_count is not UNSET:
            field_dict["expected_record_count"] = expected_record_count
        if expected_size_bytes is not UNSET:
            field_dict["expected_size_bytes"] = expected_size_bytes
        if owner_ref is not UNSET:
            field_dict["owner_ref"] = owner_ref

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        d = dict(src_dict)
        expected_chunk_count = d.pop("expected_chunk_count")

        owner_type = OwnerType(d.pop("owner_type"))

        def _parse_expected_record_count(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        expected_record_count = _parse_expected_record_count(
            d.pop("expected_record_count", UNSET)
        )

        def _parse_expected_size_bytes(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        expected_size_bytes = _parse_expected_size_bytes(
            d.pop("expected_size_bytes", UNSET)
        )

        def _parse_owner_ref(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        owner_ref = _parse_owner_ref(d.pop("owner_ref", UNSET))

        seal_cache_generation_request = cls(
            expected_chunk_count=expected_chunk_count,
            owner_type=owner_type,
            expected_record_count=expected_record_count,
            expected_size_bytes=expected_size_bytes,
            owner_ref=owner_ref,
        )

        seal_cache_generation_request.additional_properties = d
        return seal_cache_generation_request

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
