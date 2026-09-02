from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

if TYPE_CHECKING:
    from ..models.analyzed_pack import AnalyzedPack
    from ..models.api_response_get_pack_dependencies_response_data_runtime_requirements import (
        ApiResponseGetPackDependenciesResponseDataRuntimeRequirements,
    )
    from ..models.dependency_error import DependencyError
    from ..models.pack_dependency import PackDependency


T = TypeVar("T", bound="ApiResponseGetPackDependenciesResponseData")


@_attrs_define
class ApiResponseGetPackDependenciesResponseData:
    """Response DTO for get pack dependencies operation

    Attributes:
        analyzed_packs (list[AnalyzedPack]): Packs that were analyzed
        dependencies (list[PackDependency]): All dependencies found
        errors (list[DependencyError]): Errors encountered during analysis
        missing_dependencies (list[PackDependency]): Dependencies not yet installed
        runtime_requirements (ApiResponseGetPackDependenciesResponseDataRuntimeRequirements): Runtime requirements by
            pack
    """

    analyzed_packs: list[AnalyzedPack]
    dependencies: list[PackDependency]
    errors: list[DependencyError]
    missing_dependencies: list[PackDependency]
    runtime_requirements: ApiResponseGetPackDependenciesResponseDataRuntimeRequirements
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        analyzed_packs = []
        for analyzed_packs_item_data in self.analyzed_packs:
            analyzed_packs_item = analyzed_packs_item_data.to_dict()
            analyzed_packs.append(analyzed_packs_item)

        dependencies = []
        for dependencies_item_data in self.dependencies:
            dependencies_item = dependencies_item_data.to_dict()
            dependencies.append(dependencies_item)

        errors = []
        for errors_item_data in self.errors:
            errors_item = errors_item_data.to_dict()
            errors.append(errors_item)

        missing_dependencies = []
        for missing_dependencies_item_data in self.missing_dependencies:
            missing_dependencies_item = missing_dependencies_item_data.to_dict()
            missing_dependencies.append(missing_dependencies_item)

        runtime_requirements = self.runtime_requirements.to_dict()

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "analyzed_packs": analyzed_packs,
                "dependencies": dependencies,
                "errors": errors,
                "missing_dependencies": missing_dependencies,
                "runtime_requirements": runtime_requirements,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        from ..models.analyzed_pack import AnalyzedPack
        from ..models.api_response_get_pack_dependencies_response_data_runtime_requirements import (
            ApiResponseGetPackDependenciesResponseDataRuntimeRequirements,
        )
        from ..models.dependency_error import DependencyError
        from ..models.pack_dependency import PackDependency

        d = dict(src_dict)
        analyzed_packs = []
        _analyzed_packs = d.pop("analyzed_packs")
        for analyzed_packs_item_data in _analyzed_packs:
            analyzed_packs_item = AnalyzedPack.from_dict(analyzed_packs_item_data)

            analyzed_packs.append(analyzed_packs_item)

        dependencies = []
        _dependencies = d.pop("dependencies")
        for dependencies_item_data in _dependencies:
            dependencies_item = PackDependency.from_dict(dependencies_item_data)

            dependencies.append(dependencies_item)

        errors = []
        _errors = d.pop("errors")
        for errors_item_data in _errors:
            errors_item = DependencyError.from_dict(errors_item_data)

            errors.append(errors_item)

        missing_dependencies = []
        _missing_dependencies = d.pop("missing_dependencies")
        for missing_dependencies_item_data in _missing_dependencies:
            missing_dependencies_item = PackDependency.from_dict(
                missing_dependencies_item_data
            )

            missing_dependencies.append(missing_dependencies_item)

        runtime_requirements = (
            ApiResponseGetPackDependenciesResponseDataRuntimeRequirements.from_dict(
                d.pop("runtime_requirements")
            )
        )

        api_response_get_pack_dependencies_response_data = cls(
            analyzed_packs=analyzed_packs,
            dependencies=dependencies,
            errors=errors,
            missing_dependencies=missing_dependencies,
            runtime_requirements=runtime_requirements,
        )

        api_response_get_pack_dependencies_response_data.additional_properties = d
        return api_response_get_pack_dependencies_response_data

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
