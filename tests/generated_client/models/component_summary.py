from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define
from typing_extensions import Self

T = TypeVar("T", bound="ComponentSummary")


@_attrs_define
class ComponentSummary:
    """Component summary (action, sensor, trigger, etc.)

    Attributes:
        description (str): Brief description
        name (str): Component name
    """

    description: str
    name: str

    def to_dict(self) -> dict[str, Any]:
        description = self.description

        name = self.name

        field_dict: dict[str, Any] = {}

        field_dict.update(
            {
                "description": description,
                "name": name,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        d = dict(src_dict)
        description = d.pop("description")

        name = d.pop("name")

        component_summary = cls(
            description=description,
            name=name,
        )

        return component_summary
