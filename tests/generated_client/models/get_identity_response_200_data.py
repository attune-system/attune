from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.identity_role_assignment_response import (
        IdentityRoleAssignmentResponse,
    )
    from ..models.permission_assignment_response import PermissionAssignmentResponse


T = TypeVar("T", bound="GetIdentityResponse200Data")


@_attrs_define
class GetIdentityResponse200Data:
    """
    Attributes:
        attributes (Any):
        direct_permissions (list[PermissionAssignmentResponse]):
        frozen (bool):
        id (int):
        login (str):
        roles (list[IdentityRoleAssignmentResponse]):
        display_name (None | str | Unset):
    """

    attributes: Any
    direct_permissions: list[PermissionAssignmentResponse]
    frozen: bool
    id: int
    login: str
    roles: list[IdentityRoleAssignmentResponse]
    display_name: None | str | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        attributes = self.attributes

        direct_permissions = []
        for direct_permissions_item_data in self.direct_permissions:
            direct_permissions_item = direct_permissions_item_data.to_dict()
            direct_permissions.append(direct_permissions_item)

        frozen = self.frozen

        id = self.id

        login = self.login

        roles = []
        for roles_item_data in self.roles:
            roles_item = roles_item_data.to_dict()
            roles.append(roles_item)

        display_name: None | str | Unset
        if isinstance(self.display_name, Unset):
            display_name = UNSET
        else:
            display_name = self.display_name

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "attributes": attributes,
                "direct_permissions": direct_permissions,
                "frozen": frozen,
                "id": id,
                "login": login,
                "roles": roles,
            }
        )
        if display_name is not UNSET:
            field_dict["display_name"] = display_name

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.identity_role_assignment_response import (
            IdentityRoleAssignmentResponse,
        )
        from ..models.permission_assignment_response import PermissionAssignmentResponse

        d = dict(src_dict)
        attributes = d.pop("attributes")

        direct_permissions = []
        _direct_permissions = d.pop("direct_permissions")
        for direct_permissions_item_data in _direct_permissions:
            direct_permissions_item = PermissionAssignmentResponse.from_dict(
                direct_permissions_item_data
            )

            direct_permissions.append(direct_permissions_item)

        frozen = d.pop("frozen")

        id = d.pop("id")

        login = d.pop("login")

        roles = []
        _roles = d.pop("roles")
        for roles_item_data in _roles:
            roles_item = IdentityRoleAssignmentResponse.from_dict(roles_item_data)

            roles.append(roles_item)

        def _parse_display_name(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        display_name = _parse_display_name(d.pop("display_name", UNSET))

        get_identity_response_200_data = cls(
            attributes=attributes,
            direct_permissions=direct_permissions,
            frozen=frozen,
            id=id,
            login=login,
            roles=roles,
            display_name=display_name,
        )

        get_identity_response_200_data.additional_properties = d
        return get_identity_response_200_data

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
