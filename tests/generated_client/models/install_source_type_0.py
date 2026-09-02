from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

from ..models.install_source_type_0_type import InstallSourceType0Type

T = TypeVar("T", bound="InstallSourceType0")


@_attrs_define
class InstallSourceType0:
    """Git repository source

    Attributes:
        checksum (str): Checksum in format "algorithm:hash"
        ref (str): Git ref (tag, branch, commit)
        type_ (InstallSourceType0Type):
        url (str): Git repository URL
    """

    checksum: str
    ref: str
    type_: InstallSourceType0Type
    url: str
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        checksum = self.checksum

        ref = self.ref

        type_ = self.type_.value

        url = self.url

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "checksum": checksum,
                "ref": ref,
                "type": type_,
                "url": url,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        d = dict(src_dict)
        checksum = d.pop("checksum")

        ref = d.pop("ref")

        type_ = InstallSourceType0Type(d.pop("type"))

        url = d.pop("url")

        install_source_type_0 = cls(
            checksum=checksum,
            ref=ref,
            type_=type_,
            url=url,
        )

        install_source_type_0.additional_properties = d
        return install_source_type_0

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
