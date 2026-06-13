from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="RegisterPackRequest")


@_attrs_define
class RegisterPackRequest:
    """Request DTO for registering a pack from local filesystem

    Attributes:
        path (str): Local filesystem path to the pack directory Example: /path/to/packs/mypack.
        force (bool | Unset): Force registration even if tests fail
        skip_tests (bool | Unset): Skip running pack tests during registration
    """

    path: str
    force: bool | Unset = UNSET
    skip_tests: bool | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        path = self.path

        force = self.force

        skip_tests = self.skip_tests

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "path": path,
            }
        )
        if force is not UNSET:
            field_dict["force"] = force
        if skip_tests is not UNSET:
            field_dict["skip_tests"] = skip_tests

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        path = d.pop("path")

        force = d.pop("force", UNSET)

        skip_tests = d.pop("skip_tests", UNSET)

        register_pack_request = cls(
            path=path,
            force=force,
            skip_tests=skip_tests,
        )

        register_pack_request.additional_properties = d
        return register_pack_request

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
