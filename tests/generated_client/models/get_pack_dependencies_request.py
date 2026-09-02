from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

from ..types import UNSET, Unset

T = TypeVar("T", bound="GetPackDependenciesRequest")


@_attrs_define
class GetPackDependenciesRequest:
    """Request DTO for getting pack dependencies

    Attributes:
        pack_paths (list[str]): List of pack directory paths to analyze Example: ['/tmp/attune-packs/slack'].
        skip_validation (bool | Unset): Skip pack.yaml validation Example: False.
    """

    pack_paths: list[str]
    skip_validation: bool | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        pack_paths = self.pack_paths

        skip_validation = self.skip_validation

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "pack_paths": pack_paths,
            }
        )
        if skip_validation is not UNSET:
            field_dict["skip_validation"] = skip_validation

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        d = dict(src_dict)
        pack_paths = cast(list[str], d.pop("pack_paths"))

        skip_validation = d.pop("skip_validation", UNSET)

        get_pack_dependencies_request = cls(
            pack_paths=pack_paths,
            skip_validation=skip_validation,
        )

        get_pack_dependencies_request.additional_properties = d
        return get_pack_dependencies_request

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
