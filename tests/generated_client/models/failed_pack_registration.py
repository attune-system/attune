from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

from ..types import UNSET, Unset

T = TypeVar("T", bound="FailedPackRegistration")


@_attrs_define
class FailedPackRegistration:
    """Failed pack registration

    Attributes:
        error (str): Error message
        error_stage (str): Error stage
        pack_path (str): Pack path
        pack_ref (str): Pack reference
        details (Any | None | Unset):
    """

    error: str
    error_stage: str
    pack_path: str
    pack_ref: str
    details: Any | None | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        error = self.error

        error_stage = self.error_stage

        pack_path = self.pack_path

        pack_ref = self.pack_ref

        details: Any | None | Unset
        if isinstance(self.details, Unset):
            details = UNSET
        else:
            details = self.details

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "error": error,
                "error_stage": error_stage,
                "pack_path": pack_path,
                "pack_ref": pack_ref,
            }
        )
        if details is not UNSET:
            field_dict["details"] = details

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        d = dict(src_dict)
        error = d.pop("error")

        error_stage = d.pop("error_stage")

        pack_path = d.pop("pack_path")

        pack_ref = d.pop("pack_ref")

        def _parse_details(data: object) -> Any | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Any | None | Unset, data)

        details = _parse_details(d.pop("details", UNSET))

        failed_pack_registration = cls(
            error=error,
            error_stage=error_stage,
            pack_path=pack_path,
            pack_ref=pack_ref,
            details=details,
        )

        failed_pack_registration.additional_properties = d
        return failed_pack_registration

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
