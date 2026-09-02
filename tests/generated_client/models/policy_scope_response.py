from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

from ..models.policy_scope_type import PolicyScopeType
from ..types import UNSET, Unset

T = TypeVar("T", bound="PolicyScopeResponse")


@_attrs_define
class PolicyScopeResponse:
    """
    Attributes:
        type_ (PolicyScopeType):
        action (int | None | Unset):  Example: 1.
        action_ref (None | str | Unset):  Example: core.echo.
        pack (int | None | Unset):  Example: 1.
        pack_ref (None | str | Unset):  Example: core.
    """

    type_: PolicyScopeType
    action: int | None | Unset = UNSET
    action_ref: None | str | Unset = UNSET
    pack: int | None | Unset = UNSET
    pack_ref: None | str | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        type_ = self.type_.value

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
                "type": type_,
            }
        )
        if action is not UNSET:
            field_dict["action"] = action
        if action_ref is not UNSET:
            field_dict["action_ref"] = action_ref
        if pack is not UNSET:
            field_dict["pack"] = pack
        if pack_ref is not UNSET:
            field_dict["pack_ref"] = pack_ref

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        d = dict(src_dict)
        type_ = PolicyScopeType(d.pop("type"))

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

        policy_scope_response = cls(
            type_=type_,
            action=action,
            action_ref=action_ref,
            pack=pack,
            pack_ref=pack_ref,
        )

        policy_scope_response.additional_properties = d
        return policy_scope_response

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
