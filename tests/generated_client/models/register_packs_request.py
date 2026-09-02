from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

from ..types import UNSET, Unset

T = TypeVar("T", bound="RegisterPacksRequest")


@_attrs_define
class RegisterPacksRequest:
    """Request DTO for registering multiple packs

    Attributes:
        pack_paths (list[str]): List of pack directory paths to register Example: ['/tmp/attune-packs/slack'].
        force (bool | Unset): Force registration (replace if exists) Example: False.
        packs_base_dir (None | str | Unset): Base directory for permanent storage Example: /opt/attune/packs.
        skip_tests (bool | Unset): Skip running pack tests Example: False.
        skip_validation (bool | Unset): Skip schema validation Example: False.
    """

    pack_paths: list[str]
    force: bool | Unset = UNSET
    packs_base_dir: None | str | Unset = UNSET
    skip_tests: bool | Unset = UNSET
    skip_validation: bool | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        pack_paths = self.pack_paths

        force = self.force

        packs_base_dir: None | str | Unset
        if isinstance(self.packs_base_dir, Unset):
            packs_base_dir = UNSET
        else:
            packs_base_dir = self.packs_base_dir

        skip_tests = self.skip_tests

        skip_validation = self.skip_validation

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "pack_paths": pack_paths,
            }
        )
        if force is not UNSET:
            field_dict["force"] = force
        if packs_base_dir is not UNSET:
            field_dict["packs_base_dir"] = packs_base_dir
        if skip_tests is not UNSET:
            field_dict["skip_tests"] = skip_tests
        if skip_validation is not UNSET:
            field_dict["skip_validation"] = skip_validation

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        d = dict(src_dict)
        pack_paths = cast(list[str], d.pop("pack_paths"))

        force = d.pop("force", UNSET)

        def _parse_packs_base_dir(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        packs_base_dir = _parse_packs_base_dir(d.pop("packs_base_dir", UNSET))

        skip_tests = d.pop("skip_tests", UNSET)

        skip_validation = d.pop("skip_validation", UNSET)

        register_packs_request = cls(
            pack_paths=pack_paths,
            force=force,
            packs_base_dir=packs_base_dir,
            skip_tests=skip_tests,
            skip_validation=skip_validation,
        )

        register_packs_request.additional_properties = d
        return register_packs_request

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
