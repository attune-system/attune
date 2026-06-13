from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.permission_set_role_assignment_response import (
        PermissionSetRoleAssignmentResponse,
    )


T = TypeVar("T", bound="PermissionSetSummary")


@_attrs_define
class PermissionSetSummary:
    """
    Attributes:
        grants (Any):
        id (int):
        ref (str):
        roles (list[PermissionSetRoleAssignmentResponse]):
        description (None | str | Unset):
        label (None | str | Unset):
        pack_ref (None | str | Unset):
    """

    grants: Any
    id: int
    ref: str
    roles: list[PermissionSetRoleAssignmentResponse]
    description: None | str | Unset = UNSET
    label: None | str | Unset = UNSET
    pack_ref: None | str | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        grants = self.grants

        id = self.id

        ref = self.ref

        roles = []
        for roles_item_data in self.roles:
            roles_item = roles_item_data.to_dict()
            roles.append(roles_item)

        description: None | str | Unset
        if isinstance(self.description, Unset):
            description = UNSET
        else:
            description = self.description

        label: None | str | Unset
        if isinstance(self.label, Unset):
            label = UNSET
        else:
            label = self.label

        pack_ref: None | str | Unset
        if isinstance(self.pack_ref, Unset):
            pack_ref = UNSET
        else:
            pack_ref = self.pack_ref

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "grants": grants,
                "id": id,
                "ref": ref,
                "roles": roles,
            }
        )
        if description is not UNSET:
            field_dict["description"] = description
        if label is not UNSET:
            field_dict["label"] = label
        if pack_ref is not UNSET:
            field_dict["pack_ref"] = pack_ref

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.permission_set_role_assignment_response import (
            PermissionSetRoleAssignmentResponse,
        )

        d = dict(src_dict)
        grants = d.pop("grants")

        id = d.pop("id")

        ref = d.pop("ref")

        roles = []
        _roles = d.pop("roles")
        for roles_item_data in _roles:
            roles_item = PermissionSetRoleAssignmentResponse.from_dict(roles_item_data)

            roles.append(roles_item)

        def _parse_description(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        description = _parse_description(d.pop("description", UNSET))

        def _parse_label(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        label = _parse_label(d.pop("label", UNSET))

        def _parse_pack_ref(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        pack_ref = _parse_pack_ref(d.pop("pack_ref", UNSET))

        permission_set_summary = cls(
            grants=grants,
            id=id,
            ref=ref,
            roles=roles,
            description=description,
            label=label,
            pack_ref=pack_ref,
        )

        permission_set_summary.additional_properties = d
        return permission_set_summary

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
