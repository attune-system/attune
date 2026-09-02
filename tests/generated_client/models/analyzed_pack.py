from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

T = TypeVar("T", bound="AnalyzedPack")


@_attrs_define
class AnalyzedPack:
    """Information about an analyzed pack

    Attributes:
        dependency_count (int): Number of dependencies
        has_dependencies (bool): Whether pack has dependencies
        pack_path (str): Pack directory path
        pack_ref (str): Pack reference
    """

    dependency_count: int
    has_dependencies: bool
    pack_path: str
    pack_ref: str
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        dependency_count = self.dependency_count

        has_dependencies = self.has_dependencies

        pack_path = self.pack_path

        pack_ref = self.pack_ref

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "dependency_count": dependency_count,
                "has_dependencies": has_dependencies,
                "pack_path": pack_path,
                "pack_ref": pack_ref,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        d = dict(src_dict)
        dependency_count = d.pop("dependency_count")

        has_dependencies = d.pop("has_dependencies")

        pack_path = d.pop("pack_path")

        pack_ref = d.pop("pack_ref")

        analyzed_pack = cls(
            dependency_count=dependency_count,
            has_dependencies=has_dependencies,
            pack_path=pack_path,
            pack_ref=pack_ref,
        )

        analyzed_pack.additional_properties = d
        return analyzed_pack

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
