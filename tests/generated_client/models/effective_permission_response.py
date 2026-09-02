from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.effective_permission_response_constraints_type_0 import (
        EffectivePermissionResponseConstraintsType0,
    )


T = TypeVar("T", bound="EffectivePermissionResponse")


@_attrs_define
class EffectivePermissionResponse:
    """Effective permissions assigned to an identity.

    Each entry corresponds to one effective grant and can include optional
    constraints when the grant is scoped (for example to specific packs or refs).

        Attributes:
            actions (list[str]): Actions allowed for the resource. Example: ['read', 'update'].
            resource (str): RBAC resource name. Example: queues.
            constraints (EffectivePermissionResponseConstraintsType0 | None | Unset): Optional grant constraints describing
                permission scope granularity.
    """

    actions: list[str]
    resource: str
    constraints: EffectivePermissionResponseConstraintsType0 | None | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.effective_permission_response_constraints_type_0 import (
            EffectivePermissionResponseConstraintsType0,
        )

        actions = self.actions

        resource = self.resource

        constraints: dict[str, Any] | None | Unset
        if isinstance(self.constraints, Unset):
            constraints = UNSET
        elif isinstance(self.constraints, EffectivePermissionResponseConstraintsType0):
            constraints = self.constraints.to_dict()
        else:
            constraints = self.constraints

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "actions": actions,
                "resource": resource,
            }
        )
        if constraints is not UNSET:
            field_dict["constraints"] = constraints

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        from ..models.effective_permission_response_constraints_type_0 import (
            EffectivePermissionResponseConstraintsType0,
        )

        d = dict(src_dict)
        actions = cast(list[str], d.pop("actions"))

        resource = d.pop("resource")

        def _parse_constraints(
            data: object,
        ) -> EffectivePermissionResponseConstraintsType0 | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                constraints_type_0 = (
                    EffectivePermissionResponseConstraintsType0.from_dict(data)
                )

                return constraints_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(
                EffectivePermissionResponseConstraintsType0 | None | Unset, data
            )

        constraints = _parse_constraints(d.pop("constraints", UNSET))

        effective_permission_response = cls(
            actions=actions,
            resource=resource,
            constraints=constraints,
        )

        effective_permission_response.additional_properties = d
        return effective_permission_response

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
