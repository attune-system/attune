from __future__ import annotations

import datetime
from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.policy_method import PolicyMethod
from ..models.policy_scope_kind import PolicyScopeKind
from ..types import UNSET, Unset

T = TypeVar("T", bound="PolicyResponse")


@_attrs_define
class PolicyResponse:
    """
    Attributes:
        created (datetime.datetime):  Example: 2024-01-13T10:30:00Z.
        id (int):  Example: 1.
        method (PolicyMethod):
        name (str):  Example: Limit core.echo concurrency.
        parameters (list[str]):  Example: ['customer_id'].
        ref (str):  Example: core.echo_concurrency.
        scope (PolicyScopeKind):
        tags (list[str]):  Example: ['operator-managed'].
        threshold (int):  Example: 3.
        updated (datetime.datetime):  Example: 2024-01-13T10:30:00Z.
        action (int | None | Unset):  Example: 1.
        action_ref (None | str | Unset):  Example: core.echo.
        description (None | str | Unset):  Example: Keeps core.echo executions within downstream capacity.
        pack (int | None | Unset):  Example: 1.
        pack_ref (None | str | Unset):  Example: core.
    """

    created: datetime.datetime
    id: int
    method: PolicyMethod
    name: str
    parameters: list[str]
    ref: str
    scope: PolicyScopeKind
    tags: list[str]
    threshold: int
    updated: datetime.datetime
    action: int | None | Unset = UNSET
    action_ref: None | str | Unset = UNSET
    description: None | str | Unset = UNSET
    pack: int | None | Unset = UNSET
    pack_ref: None | str | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        created = self.created.isoformat()

        id = self.id

        method = self.method.value

        name = self.name

        parameters = self.parameters

        ref = self.ref

        scope = self.scope.value

        tags = self.tags

        threshold = self.threshold

        updated = self.updated.isoformat()

        action: int | None | Unset
        if isinstance(self.action, Unset):
            action = UNSET
        else:
            action = self.action

        action_ref: None | str | Unset
        if isinstance(self.action_ref, Unset):
            action_ref = UNSET
        else:
            action_ref = self.action_ref

        description: None | str | Unset
        if isinstance(self.description, Unset):
            description = UNSET
        else:
            description = self.description

        pack: int | None | Unset
        if isinstance(self.pack, Unset):
            pack = UNSET
        else:
            pack = self.pack

        pack_ref: None | str | Unset
        if isinstance(self.pack_ref, Unset):
            pack_ref = UNSET
        else:
            pack_ref = self.pack_ref

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "created": created,
                "id": id,
                "method": method,
                "name": name,
                "parameters": parameters,
                "ref": ref,
                "scope": scope,
                "tags": tags,
                "threshold": threshold,
                "updated": updated,
            }
        )
        if action is not UNSET:
            field_dict["action"] = action
        if action_ref is not UNSET:
            field_dict["action_ref"] = action_ref
        if description is not UNSET:
            field_dict["description"] = description
        if pack is not UNSET:
            field_dict["pack"] = pack
        if pack_ref is not UNSET:
            field_dict["pack_ref"] = pack_ref

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        created = datetime.datetime.fromisoformat(d.pop("created"))

        id = d.pop("id")

        method = PolicyMethod(d.pop("method"))

        name = d.pop("name")

        parameters = cast(list[str], d.pop("parameters"))

        ref = d.pop("ref")

        scope = PolicyScopeKind(d.pop("scope"))

        tags = cast(list[str], d.pop("tags"))

        threshold = d.pop("threshold")

        updated = datetime.datetime.fromisoformat(d.pop("updated"))

        def _parse_action(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        action = _parse_action(d.pop("action", UNSET))

        def _parse_action_ref(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        action_ref = _parse_action_ref(d.pop("action_ref", UNSET))

        def _parse_description(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        description = _parse_description(d.pop("description", UNSET))

        def _parse_pack(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        pack = _parse_pack(d.pop("pack", UNSET))

        def _parse_pack_ref(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        pack_ref = _parse_pack_ref(d.pop("pack_ref", UNSET))

        policy_response = cls(
            created=created,
            id=id,
            method=method,
            name=name,
            parameters=parameters,
            ref=ref,
            scope=scope,
            tags=tags,
            threshold=threshold,
            updated=updated,
            action=action,
            action_ref=action_ref,
            description=description,
            pack=pack,
            pack_ref=pack_ref,
        )

        policy_response.additional_properties = d
        return policy_response

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
