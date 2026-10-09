from __future__ import annotations

import datetime
from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

if TYPE_CHECKING:
    from ..models.permission_identity_target import PermissionIdentityTarget
    from ..models.permission_role_target import PermissionRoleTarget


T = TypeVar("T", bound="PaginatedResponsePermissionBindingResponseItemsItem")


@_attrs_define
class PaginatedResponsePermissionBindingResponseItemsItem:
    """
    Attributes:
        created (datetime.datetime):
        id (int):
        permission_set_id (int):
        permission_set_ref (str):
        target (PermissionIdentityTarget | PermissionRoleTarget):
    """

    created: datetime.datetime
    id: int
    permission_set_id: int
    permission_set_ref: str
    target: PermissionIdentityTarget | PermissionRoleTarget
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.permission_identity_target import (
            PermissionIdentityTarget,
        )

        created = self.created.isoformat()

        id = self.id

        permission_set_id = self.permission_set_id

        permission_set_ref = self.permission_set_ref

        target: dict[str, Any]
        if isinstance(self.target, PermissionIdentityTarget):
            target = self.target.to_dict()
        else:
            target = self.target.to_dict()

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "created": created,
                "id": id,
                "permission_set_id": permission_set_id,
                "permission_set_ref": permission_set_ref,
                "target": target,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        from ..models.permission_identity_target import (
            PermissionIdentityTarget,
        )
        from ..models.permission_role_target import (
            PermissionRoleTarget,
        )

        d = dict(src_dict)
        created = datetime.datetime.fromisoformat(d.pop("created"))

        id = d.pop("id")

        permission_set_id = d.pop("permission_set_id")

        permission_set_ref = d.pop("permission_set_ref")

        def _parse_target(
            data: object,
        ) -> PermissionIdentityTarget | PermissionRoleTarget:
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_permission_binding_target_type_0 = (
                    PermissionIdentityTarget.from_dict(data)
                )

                return componentsschemas_permission_binding_target_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            if not isinstance(data, dict):
                raise TypeError()
            componentsschemas_permission_binding_target_type_1 = (
                PermissionRoleTarget.from_dict(data)
            )

            return componentsschemas_permission_binding_target_type_1

        target = _parse_target(d.pop("target"))

        paginated_response_permission_binding_response_items_item = cls(
            created=created,
            id=id,
            permission_set_id=permission_set_id,
            permission_set_ref=permission_set_ref,
            target=target,
        )

        paginated_response_permission_binding_response_items_item.additional_properties = d
        return paginated_response_permission_binding_response_items_item

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
