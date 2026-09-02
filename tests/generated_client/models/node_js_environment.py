from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

T = TypeVar("T", bound="NodeJsEnvironment")


@_attrs_define
class NodeJsEnvironment:
    """Node.js environment details

    Attributes:
        dependencies_installed (bool): Whether dependencies were installed
        node_modules_path (str): Path to node_modules
        nodejs_version (str): Node.js version used
        package_count (int): Number of packages installed
    """

    dependencies_installed: bool
    node_modules_path: str
    nodejs_version: str
    package_count: int
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        dependencies_installed = self.dependencies_installed

        node_modules_path = self.node_modules_path

        nodejs_version = self.nodejs_version

        package_count = self.package_count

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "dependencies_installed": dependencies_installed,
                "node_modules_path": node_modules_path,
                "nodejs_version": nodejs_version,
                "package_count": package_count,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        d = dict(src_dict)
        dependencies_installed = d.pop("dependencies_installed")

        node_modules_path = d.pop("node_modules_path")

        nodejs_version = d.pop("nodejs_version")

        package_count = d.pop("package_count")

        node_js_environment = cls(
            dependencies_installed=dependencies_installed,
            node_modules_path=node_modules_path,
            nodejs_version=nodejs_version,
            package_count=package_count,
        )

        node_js_environment.additional_properties = d
        return node_js_environment

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
