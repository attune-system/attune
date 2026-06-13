from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

T = TypeVar("T", bound="AgentArchInfo")


@_attrs_define
class AgentArchInfo:
    """Per-architecture binary info

    Attributes:
        arch (str): Architecture name
        available (bool): Whether this binary is available
        size_bytes (int): Binary size in bytes
    """

    arch: str
    available: bool
    size_bytes: int
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        arch = self.arch

        available = self.available

        size_bytes = self.size_bytes

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "arch": arch,
                "available": available,
                "size_bytes": size_bytes,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        arch = d.pop("arch")

        available = d.pop("available")

        size_bytes = d.pop("size_bytes")

        agent_arch_info = cls(
            arch=arch,
            available=available,
            size_bytes=size_bytes,
        )

        agent_arch_info.additional_properties = d
        return agent_arch_info

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
