from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

T = TypeVar("T", bound="RegistrationSummary")


@_attrs_define
class RegistrationSummary:
    """Registration summary

    Attributes:
        duration_ms (int): Duration in milliseconds
        failure_count (int): Failed registrations
        success_count (int): Successfully registered
        total_components (int): Total components registered
        total_packs (int): Total packs processed
    """

    duration_ms: int
    failure_count: int
    success_count: int
    total_components: int
    total_packs: int
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        duration_ms = self.duration_ms

        failure_count = self.failure_count

        success_count = self.success_count

        total_components = self.total_components

        total_packs = self.total_packs

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "duration_ms": duration_ms,
                "failure_count": failure_count,
                "success_count": success_count,
                "total_components": total_components,
                "total_packs": total_packs,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        d = dict(src_dict)
        duration_ms = d.pop("duration_ms")

        failure_count = d.pop("failure_count")

        success_count = d.pop("success_count")

        total_components = d.pop("total_components")

        total_packs = d.pop("total_packs")

        registration_summary = cls(
            duration_ms=duration_ms,
            failure_count=failure_count,
            success_count=success_count,
            total_components=total_components,
            total_packs=total_packs,
        )

        registration_summary.additional_properties = d
        return registration_summary

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
