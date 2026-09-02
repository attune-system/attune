from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

T = TypeVar("T", bound="FailedEnvironment")


@_attrs_define
class FailedEnvironment:
    """Failed environment build

    Attributes:
        error (str): Error message
        pack_path (str): Pack directory path
        pack_ref (str): Pack reference
        runtime (str): Runtime that failed
    """

    error: str
    pack_path: str
    pack_ref: str
    runtime: str
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        error = self.error

        pack_path = self.pack_path

        pack_ref = self.pack_ref

        runtime = self.runtime

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "error": error,
                "pack_path": pack_path,
                "pack_ref": pack_ref,
                "runtime": runtime,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        d = dict(src_dict)
        error = d.pop("error")

        pack_path = d.pop("pack_path")

        pack_ref = d.pop("pack_ref")

        runtime = d.pop("runtime")

        failed_environment = cls(
            error=error,
            pack_path=pack_path,
            pack_ref=pack_ref,
            runtime=runtime,
        )

        failed_environment.additional_properties = d
        return failed_environment

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
