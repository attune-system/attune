from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="CreatePermissionAssignmentRequest")


@_attrs_define
class CreatePermissionAssignmentRequest:
    """
    Attributes:
        permission_set_ref (str):
        identity_id (int | None | Unset):
        identity_login (None | str | Unset):
    """

    permission_set_ref: str
    identity_id: int | None | Unset = UNSET
    identity_login: None | str | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        permission_set_ref = self.permission_set_ref

        identity_id: int | None | Unset
        if isinstance(self.identity_id, Unset):
            identity_id = UNSET
        else:
            identity_id = self.identity_id

        identity_login: None | str | Unset
        if isinstance(self.identity_login, Unset):
            identity_login = UNSET
        else:
            identity_login = self.identity_login

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "permission_set_ref": permission_set_ref,
            }
        )
        if identity_id is not UNSET:
            field_dict["identity_id"] = identity_id
        if identity_login is not UNSET:
            field_dict["identity_login"] = identity_login

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        permission_set_ref = d.pop("permission_set_ref")

        def _parse_identity_id(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        identity_id = _parse_identity_id(d.pop("identity_id", UNSET))

        def _parse_identity_login(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        identity_login = _parse_identity_login(d.pop("identity_login", UNSET))

        create_permission_assignment_request = cls(
            permission_set_ref=permission_set_ref,
            identity_id=identity_id,
            identity_login=identity_login,
        )

        create_permission_assignment_request.additional_properties = d
        return create_permission_assignment_request

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
