from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

T = TypeVar("T", bound="ApplyLiveCacheResponse")


@_attrs_define
class ApplyLiveCacheResponse:
    """
    Attributes:
        record_count (int):
        replayed (bool):
        revision (int):
        size_bytes (int):
        source_revision (None | str):
    """

    record_count: int
    replayed: bool
    revision: int
    size_bytes: int
    source_revision: None | str
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        record_count = self.record_count

        replayed = self.replayed

        revision = self.revision

        size_bytes = self.size_bytes

        source_revision: None | str
        source_revision = self.source_revision

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "record_count": record_count,
                "replayed": replayed,
                "revision": revision,
                "size_bytes": size_bytes,
                "source_revision": source_revision,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        d = dict(src_dict)
        record_count = d.pop("record_count")

        replayed = d.pop("replayed")

        revision = d.pop("revision")

        size_bytes = d.pop("size_bytes")

        def _parse_source_revision(data: object) -> None | str:
            if data is None:
                return data
            return cast(None | str, data)

        source_revision = _parse_source_revision(d.pop("source_revision"))

        apply_live_cache_response = cls(
            record_count=record_count,
            replayed=replayed,
            revision=revision,
            size_bytes=size_bytes,
            source_revision=source_revision,
        )

        apply_live_cache_response.additional_properties = d
        return apply_live_cache_response

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
