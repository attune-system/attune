from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

T = TypeVar("T", bound="PythonEnvironment")


@_attrs_define
class PythonEnvironment:
    """Python environment details

    Attributes:
        package_count (int): Number of packages installed
        python_version (str): Python version used
        requirements_installed (bool): Whether requirements were installed
        virtualenv_path (str): Path to virtualenv
    """

    package_count: int
    python_version: str
    requirements_installed: bool
    virtualenv_path: str
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        package_count = self.package_count

        python_version = self.python_version

        requirements_installed = self.requirements_installed

        virtualenv_path = self.virtualenv_path

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "package_count": package_count,
                "python_version": python_version,
                "requirements_installed": requirements_installed,
                "virtualenv_path": virtualenv_path,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        d = dict(src_dict)
        package_count = d.pop("package_count")

        python_version = d.pop("python_version")

        requirements_installed = d.pop("requirements_installed")

        virtualenv_path = d.pop("virtualenv_path")

        python_environment = cls(
            package_count=package_count,
            python_version=python_version,
            requirements_installed=requirements_installed,
            virtualenv_path=virtualenv_path,
        )

        python_environment.additional_properties = d
        return python_environment

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
