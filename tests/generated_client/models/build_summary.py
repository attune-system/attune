from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

T = TypeVar("T", bound="BuildSummary")


@_attrs_define
class BuildSummary:
    """Build summary statistics

    Attributes:
        failure_count (int): Failed builds
        nodejs_envs_built (int): Node.js environments built
        python_envs_built (int): Python environments built
        success_count (int): Successfully built
        total_duration_ms (int): Total duration in milliseconds
        total_packs (int): Total packs processed
    """

    failure_count: int
    nodejs_envs_built: int
    python_envs_built: int
    success_count: int
    total_duration_ms: int
    total_packs: int
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        failure_count = self.failure_count

        nodejs_envs_built = self.nodejs_envs_built

        python_envs_built = self.python_envs_built

        success_count = self.success_count

        total_duration_ms = self.total_duration_ms

        total_packs = self.total_packs

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "failure_count": failure_count,
                "nodejs_envs_built": nodejs_envs_built,
                "python_envs_built": python_envs_built,
                "success_count": success_count,
                "total_duration_ms": total_duration_ms,
                "total_packs": total_packs,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        d = dict(src_dict)
        failure_count = d.pop("failure_count")

        nodejs_envs_built = d.pop("nodejs_envs_built")

        python_envs_built = d.pop("python_envs_built")

        success_count = d.pop("success_count")

        total_duration_ms = d.pop("total_duration_ms")

        total_packs = d.pop("total_packs")

        build_summary = cls(
            failure_count=failure_count,
            nodejs_envs_built=nodejs_envs_built,
            python_envs_built=python_envs_built,
            success_count=success_count,
            total_duration_ms=total_duration_ms,
            total_packs=total_packs,
        )

        build_summary.additional_properties = d
        return build_summary

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
