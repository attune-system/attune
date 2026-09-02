from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

from ..models.install_source_type_1_type import InstallSourceType1Type

T = TypeVar("T", bound="InstallSourceType1")


@_attrs_define
class InstallSourceType1:
    """Archive (zip, tar.gz) source

    Attributes:
        checksum (str): Checksum in format "algorithm:hash"
        type_ (InstallSourceType1Type):
        url (str): Archive URL
    """

    checksum: str
    type_: InstallSourceType1Type
    url: str
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        checksum = self.checksum

        type_ = self.type_.value

        url = self.url

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "checksum": checksum,
                "type": type_,
                "url": url,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        d = dict(src_dict)
        checksum = d.pop("checksum")

        type_ = InstallSourceType1Type(d.pop("type"))

        url = d.pop("url")

        install_source_type_1 = cls(
            checksum=checksum,
            type_=type_,
            url=url,
        )

        install_source_type_1.additional_properties = d
        return install_source_type_1

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
