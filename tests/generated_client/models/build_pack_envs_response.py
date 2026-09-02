from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

if TYPE_CHECKING:
    from ..models.build_summary import BuildSummary
    from ..models.built_environment import BuiltEnvironment
    from ..models.failed_environment import FailedEnvironment


T = TypeVar("T", bound="BuildPackEnvsResponse")


@_attrs_define
class BuildPackEnvsResponse:
    """Response DTO for build pack environments operation

    Attributes:
        built_environments (list[BuiltEnvironment]): Successfully built environments
        failed_environments (list[FailedEnvironment]): Failed environment builds
        summary (BuildSummary): Build summary statistics
    """

    built_environments: list[BuiltEnvironment]
    failed_environments: list[FailedEnvironment]
    summary: BuildSummary
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        built_environments = []
        for built_environments_item_data in self.built_environments:
            built_environments_item = built_environments_item_data.to_dict()
            built_environments.append(built_environments_item)

        failed_environments = []
        for failed_environments_item_data in self.failed_environments:
            failed_environments_item = failed_environments_item_data.to_dict()
            failed_environments.append(failed_environments_item)

        summary = self.summary.to_dict()

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "built_environments": built_environments,
                "failed_environments": failed_environments,
                "summary": summary,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        from ..models.build_summary import BuildSummary
        from ..models.built_environment import BuiltEnvironment
        from ..models.failed_environment import FailedEnvironment

        d = dict(src_dict)
        built_environments = []
        _built_environments = d.pop("built_environments")
        for built_environments_item_data in _built_environments:
            built_environments_item = BuiltEnvironment.from_dict(
                built_environments_item_data
            )

            built_environments.append(built_environments_item)

        failed_environments = []
        _failed_environments = d.pop("failed_environments")
        for failed_environments_item_data in _failed_environments:
            failed_environments_item = FailedEnvironment.from_dict(
                failed_environments_item_data
            )

            failed_environments.append(failed_environments_item)

        summary = BuildSummary.from_dict(d.pop("summary"))

        build_pack_envs_response = cls(
            built_environments=built_environments,
            failed_environments=failed_environments,
            summary=summary,
        )

        build_pack_envs_response.additional_properties = d
        return build_pack_envs_response

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
