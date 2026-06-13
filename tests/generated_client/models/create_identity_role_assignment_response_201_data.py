from __future__ import annotations

import datetime
from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

T = TypeVar("T", bound="CreateIdentityRoleAssignmentResponse201Data")


@_attrs_define
class CreateIdentityRoleAssignmentResponse201Data:
    """
    Attributes:
        created (datetime.datetime):
        id (int):
        identity_id (int):
        managed (bool):
        role (str):
        source (str):
        updated (datetime.datetime):
    """

    created: datetime.datetime
    id: int
    identity_id: int
    managed: bool
    role: str
    source: str
    updated: datetime.datetime
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        created = self.created.isoformat()

        id = self.id

        identity_id = self.identity_id

        managed = self.managed

        role = self.role

        source = self.source

        updated = self.updated.isoformat()

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "created": created,
                "id": id,
                "identity_id": identity_id,
                "managed": managed,
                "role": role,
                "source": source,
                "updated": updated,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        created = datetime.datetime.fromisoformat(d.pop("created"))

        id = d.pop("id")

        identity_id = d.pop("identity_id")

        managed = d.pop("managed")

        role = d.pop("role")

        source = d.pop("source")

        updated = datetime.datetime.fromisoformat(d.pop("updated"))

        create_identity_role_assignment_response_201_data = cls(
            created=created,
            id=id,
            identity_id=identity_id,
            managed=managed,
            role=role,
            source=source,
            updated=updated,
        )

        create_identity_role_assignment_response_201_data.additional_properties = d
        return create_identity_role_assignment_response_201_data

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
