from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

T = TypeVar("T", bound="PackDependency")


@_attrs_define
class PackDependency:
    """Pack dependency information

    Attributes:
        already_installed (bool): Whether dependency is already installed
        pack_ref (str): Pack reference
        required_by (str): Pack that requires this dependency
        version_spec (str): Version specification
    """

    already_installed: bool
    pack_ref: str
    required_by: str
    version_spec: str
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        already_installed = self.already_installed

        pack_ref = self.pack_ref

        required_by = self.required_by

        version_spec = self.version_spec

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "already_installed": already_installed,
                "pack_ref": pack_ref,
                "required_by": required_by,
                "version_spec": version_spec,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        d = dict(src_dict)
        already_installed = d.pop("already_installed")

        pack_ref = d.pop("pack_ref")

        required_by = d.pop("required_by")

        version_spec = d.pop("version_spec")

        pack_dependency = cls(
            already_installed=already_installed,
            pack_ref=pack_ref,
            required_by=required_by,
            version_spec=version_spec,
        )

        pack_dependency.additional_properties = d
        return pack_dependency

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
