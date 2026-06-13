from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="InstallPackRequest")


@_attrs_define
class InstallPackRequest:
    """Request DTO for installing a pack from remote source

    Attributes:
        source (str): Repository URL or source location Example: https://github.com/attune/pack-slack.git.
        ref_spec (None | str | Unset): Git branch, tag, or commit reference Example: main.
        skip_deps (bool | Unset): Skip dependency validation (not recommended)
        skip_tests (bool | Unset): Skip running pack tests during installation
    """

    source: str
    ref_spec: None | str | Unset = UNSET
    skip_deps: bool | Unset = UNSET
    skip_tests: bool | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        source = self.source

        ref_spec: None | str | Unset
        if isinstance(self.ref_spec, Unset):
            ref_spec = UNSET
        else:
            ref_spec = self.ref_spec

        skip_deps = self.skip_deps

        skip_tests = self.skip_tests

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "source": source,
            }
        )
        if ref_spec is not UNSET:
            field_dict["ref_spec"] = ref_spec
        if skip_deps is not UNSET:
            field_dict["skip_deps"] = skip_deps
        if skip_tests is not UNSET:
            field_dict["skip_tests"] = skip_tests

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        source = d.pop("source")

        def _parse_ref_spec(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        ref_spec = _parse_ref_spec(d.pop("ref_spec", UNSET))

        skip_deps = d.pop("skip_deps", UNSET)

        skip_tests = d.pop("skip_tests", UNSET)

        install_pack_request = cls(
            source=source,
            ref_spec=ref_spec,
            skip_deps=skip_deps,
            skip_tests=skip_tests,
        )

        install_pack_request.additional_properties = d
        return install_pack_request

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
