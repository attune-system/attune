from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

T = TypeVar("T", bound="ApiResponseBuildInfoData")


@_attrs_define
class ApiResponseBuildInfoData:
    """Identity compiled into this binary, never read from deployment-time environment variables.

    Attributes:
        git_sha (str): Full source commit SHA, or "unknown" when the build had no revision metadata.
        version (str): Semantic version of the platform workspace.
    """

    git_sha: str
    version: str
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        git_sha = self.git_sha

        version = self.version

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "git_sha": git_sha,
                "version": version,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        d = dict(src_dict)
        git_sha = d.pop("git_sha")

        version = d.pop("version")

        api_response_build_info_data = cls(
            git_sha=git_sha,
            version=version,
        )

        api_response_build_info_data.additional_properties = d
        return api_response_build_info_data

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
