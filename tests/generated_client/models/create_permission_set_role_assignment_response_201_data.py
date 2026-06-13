from __future__ import annotations

import datetime
from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="CreatePermissionSetRoleAssignmentResponse201Data")


@_attrs_define
class CreatePermissionSetRoleAssignmentResponse201Data:
    """
    Attributes:
        created (datetime.datetime):
        id (int):
        permission_set_id (int):
        role (str):
        permission_set_ref (None | str | Unset):
    """

    created: datetime.datetime
    id: int
    permission_set_id: int
    role: str
    permission_set_ref: None | str | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        created = self.created.isoformat()

        id = self.id

        permission_set_id = self.permission_set_id

        role = self.role

        permission_set_ref: None | str | Unset
        if isinstance(self.permission_set_ref, Unset):
            permission_set_ref = UNSET
        else:
            permission_set_ref = self.permission_set_ref

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "created": created,
                "id": id,
                "permission_set_id": permission_set_id,
                "role": role,
            }
        )
        if permission_set_ref is not UNSET:
            field_dict["permission_set_ref"] = permission_set_ref

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        created = datetime.datetime.fromisoformat(d.pop("created"))

        id = d.pop("id")

        permission_set_id = d.pop("permission_set_id")

        role = d.pop("role")

        def _parse_permission_set_ref(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        permission_set_ref = _parse_permission_set_ref(
            d.pop("permission_set_ref", UNSET)
        )

        create_permission_set_role_assignment_response_201_data = cls(
            created=created,
            id=id,
            permission_set_id=permission_set_id,
            role=role,
            permission_set_ref=permission_set_ref,
        )

        create_permission_set_role_assignment_response_201_data.additional_properties = d
        return create_permission_set_role_assignment_response_201_data

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
