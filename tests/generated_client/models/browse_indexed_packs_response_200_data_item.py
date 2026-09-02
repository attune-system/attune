from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

if TYPE_CHECKING:
    from ..models.pack_index_entry import PackIndexEntry
    from ..models.pack_registry_index_summary import PackRegistryIndexSummary


T = TypeVar("T", bound="BrowseIndexedPacksResponse200DataItem")


@_attrs_define
class BrowseIndexedPacksResponse200DataItem:
    """Indexed pack summary with the registry it was resolved from.

    Attributes:
        pack (PackIndexEntry): Pack entry in a registry index
        registry (PackRegistryIndexSummary):
    """

    pack: PackIndexEntry
    registry: PackRegistryIndexSummary
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        pack = self.pack.to_dict()

        registry = self.registry.to_dict()

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "pack": pack,
                "registry": registry,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        from ..models.pack_index_entry import PackIndexEntry
        from ..models.pack_registry_index_summary import (
            PackRegistryIndexSummary,
        )

        d = dict(src_dict)
        pack = PackIndexEntry.from_dict(d.pop("pack"))

        registry = PackRegistryIndexSummary.from_dict(d.pop("registry"))

        browse_indexed_packs_response_200_data_item = cls(
            pack=pack,
            registry=registry,
        )

        browse_indexed_packs_response_200_data_item.additional_properties = d
        return browse_indexed_packs_response_200_data_item

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
