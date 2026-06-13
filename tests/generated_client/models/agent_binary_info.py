from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

if TYPE_CHECKING:
    from ..models.agent_arch_info import AgentArchInfo


T = TypeVar("T", bound="AgentBinaryInfo")


@_attrs_define
class AgentBinaryInfo:
    """Agent binary metadata

    Attributes:
        architectures (list[AgentArchInfo]): Available architectures
        version (str): Agent version (from build)
    """

    architectures: list[AgentArchInfo]
    version: str
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        architectures = []
        for architectures_item_data in self.architectures:
            architectures_item = architectures_item_data.to_dict()
            architectures.append(architectures_item)

        version = self.version

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "architectures": architectures,
                "version": version,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.agent_arch_info import AgentArchInfo

        d = dict(src_dict)
        architectures = []
        _architectures = d.pop("architectures")
        for architectures_item_data in _architectures:
            architectures_item = AgentArchInfo.from_dict(architectures_item_data)

            architectures.append(architectures_item)

        version = d.pop("version")

        agent_binary_info = cls(
            architectures=architectures,
            version=version,
        )

        agent_binary_info.additional_properties = d
        return agent_binary_info

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
