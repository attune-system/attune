from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..models.policy_scope_type import PolicyScopeType
from ..types import UNSET, Unset
from typing import cast






T = TypeVar("T", bound="PolicyScopeRequest")



@_attrs_define
class PolicyScopeRequest:
    """ 
        Attributes:
            type_ (PolicyScopeType):
            action_ref (None | str | Unset):  Example: core.echo.
            pack_ref (None | str | Unset):  Example: core.
     """

    type_: PolicyScopeType
    action_ref: None | str | Unset = UNSET
    pack_ref: None | str | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)





    def to_dict(self) -> dict[str, Any]:
        type_ = self.type_.value

        action_ref: None | str | Unset
        if isinstance(self.action_ref, Unset):
            action_ref = UNSET
        else:
            action_ref = self.action_ref

        pack_ref: None | str | Unset
        if isinstance(self.pack_ref, Unset):
            pack_ref = UNSET
        else:
            pack_ref = self.pack_ref


        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update({
            "type": type_,
        })
        if action_ref is not UNSET:
            field_dict["action_ref"] = action_ref
        if pack_ref is not UNSET:
            field_dict["pack_ref"] = pack_ref

        return field_dict



    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        type_ = PolicyScopeType(d.pop("type"))




        def _parse_action_ref(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        action_ref = _parse_action_ref(d.pop("action_ref", UNSET))


        def _parse_pack_ref(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        pack_ref = _parse_pack_ref(d.pop("pack_ref", UNSET))


        policy_scope_request = cls(
            type_=type_,
            action_ref=action_ref,
            pack_ref=pack_ref,
        )


        policy_scope_request.additional_properties = d
        return policy_scope_request

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
