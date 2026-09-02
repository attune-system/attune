from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.allocate_file_version_by_ref_response_201_data import (
        AllocateFileVersionByRefResponse201Data,
    )


T = TypeVar("T", bound="AllocateFileVersionByRefResponse201")


@_attrs_define
class AllocateFileVersionByRefResponse201:
    """Standard API response wrapper

    Attributes:
        data (AllocateFileVersionByRefResponse201Data): Response DTO for an artifact version (without binary content)
        message (None | str | Unset): Optional message
    """

    data: AllocateFileVersionByRefResponse201Data
    message: None | str | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        data = self.data.to_dict()

        message: None | str | Unset
        if isinstance(self.message, Unset):
            message = UNSET
        else:
            message = self.message

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "data": data,
            }
        )
        if message is not UNSET:
            field_dict["message"] = message

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        from ..models.allocate_file_version_by_ref_response_201_data import (
            AllocateFileVersionByRefResponse201Data,
        )

        d = dict(src_dict)
        data = AllocateFileVersionByRefResponse201Data.from_dict(d.pop("data"))

        def _parse_message(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        message = _parse_message(d.pop("message", UNSET))

        allocate_file_version_by_ref_response_201 = cls(
            data=data,
            message=message,
        )

        allocate_file_version_by_ref_response_201.additional_properties = d
        return allocate_file_version_by_ref_response_201

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
