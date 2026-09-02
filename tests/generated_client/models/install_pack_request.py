from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

from ..types import UNSET, Unset

T = TypeVar("T", bound="InstallPackRequest")


@_attrs_define
class InstallPackRequest:
    """Request DTO for installing a pack from remote source

    Attributes:
        source (str): Repository URL or source location Example: https://github.com/attune/pack-slack.git.
        force (bool | Unset): Replace an existing pack with the same ref Example: False.
        no_registry (bool | Unset): Require an explicit URL or existing local path instead of registry lookup.
        ref_spec (None | str | Unset): Git branch, tag, or commit reference Example: main.
        registry_id (int | None | Unset): Restrict registry-reference resolution to one managed index.
        skip_deps (bool | Unset): Skip dependency validation (not recommended) Example: False.
        skip_tests (bool | Unset): Skip running pack tests during installation Example: False.
    """

    source: str
    force: bool | Unset = UNSET
    no_registry: bool | Unset = UNSET
    ref_spec: None | str | Unset = UNSET
    registry_id: int | None | Unset = UNSET
    skip_deps: bool | Unset = UNSET
    skip_tests: bool | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        source = self.source

        force = self.force

        no_registry = self.no_registry

        ref_spec: None | str | Unset
        if isinstance(self.ref_spec, Unset):
            ref_spec = UNSET
        else:
            ref_spec = self.ref_spec

        registry_id: int | None | Unset
        if isinstance(self.registry_id, Unset):
            registry_id = UNSET
        else:
            registry_id = self.registry_id

        skip_deps = self.skip_deps

        skip_tests = self.skip_tests

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "source": source,
            }
        )
        if force is not UNSET:
            field_dict["force"] = force
        if no_registry is not UNSET:
            field_dict["no_registry"] = no_registry
        if ref_spec is not UNSET:
            field_dict["ref_spec"] = ref_spec
        if registry_id is not UNSET:
            field_dict["registry_id"] = registry_id
        if skip_deps is not UNSET:
            field_dict["skip_deps"] = skip_deps
        if skip_tests is not UNSET:
            field_dict["skip_tests"] = skip_tests

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        d = dict(src_dict)
        source = d.pop("source")

        force = d.pop("force", UNSET)

        no_registry = d.pop("no_registry", UNSET)

        def _parse_ref_spec(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        ref_spec = _parse_ref_spec(d.pop("ref_spec", UNSET))

        def _parse_registry_id(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        registry_id = _parse_registry_id(d.pop("registry_id", UNSET))

        skip_deps = d.pop("skip_deps", UNSET)

        skip_tests = d.pop("skip_tests", UNSET)

        install_pack_request = cls(
            source=source,
            force=force,
            no_registry=no_registry,
            ref_spec=ref_spec,
            registry_id=registry_id,
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
