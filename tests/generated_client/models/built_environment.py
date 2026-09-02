from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

if TYPE_CHECKING:
    from ..models.environments import Environments


T = TypeVar("T", bound="BuiltEnvironment")


@_attrs_define
class BuiltEnvironment:
    """Information about a built environment

    Attributes:
        duration_ms (int): Build duration in milliseconds
        environments (Environments): Environment details
        pack_path (str): Pack directory path
        pack_ref (str): Pack reference
    """

    duration_ms: int
    environments: Environments
    pack_path: str
    pack_ref: str
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        duration_ms = self.duration_ms

        environments = self.environments.to_dict()

        pack_path = self.pack_path

        pack_ref = self.pack_ref

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "duration_ms": duration_ms,
                "environments": environments,
                "pack_path": pack_path,
                "pack_ref": pack_ref,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        from ..models.environments import Environments

        d = dict(src_dict)
        duration_ms = d.pop("duration_ms")

        environments = Environments.from_dict(d.pop("environments"))

        pack_path = d.pop("pack_path")

        pack_ref = d.pop("pack_ref")

        built_environment = cls(
            duration_ms=duration_ms,
            environments=environments,
            pack_path=pack_path,
            pack_ref=pack_ref,
        )

        built_environment.additional_properties = d
        return built_environment

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
