from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

from ..models.permission_role_target_type import PermissionRoleTargetType

T = TypeVar("T", bound="PermissionRoleTarget")


@_attrs_define
class PermissionRoleTarget:
    """
    Attributes:
        role (str):
        type_ (PermissionRoleTargetType):
    """

    role: str
    type_: PermissionRoleTargetType
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        role = self.role

        type_ = self.type_.value

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "role": role,
                "type": type_,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        d = dict(src_dict)
        role = d.pop("role")

        type_ = PermissionRoleTargetType(d.pop("type"))

        permission_role_target = cls(
            role=role,
            type_=type_,
        )

        permission_role_target.additional_properties = d
        return permission_role_target

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
