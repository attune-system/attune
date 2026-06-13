from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.policy_method import PolicyMethod
from ..types import UNSET, Unset

T = TypeVar("T", bound="CreatePolicyRequest")


@_attrs_define
class CreatePolicyRequest:
    """
    Attributes:
        method (PolicyMethod):
        name (str):  Example: Limit core.echo concurrency.
        ref (str):  Example: core.echo_concurrency.
        threshold (int):  Example: 3.
        action_ref (None | str | Unset):  Example: core.echo.
        description (None | str | Unset):  Example: Keeps core.echo executions within downstream capacity.
        pack_ref (None | str | Unset):  Example: core.
        parameters (list[str] | Unset):  Example: ['customer_id'].
        tags (list[str] | Unset):  Example: ['operator-managed'].
    """

    method: PolicyMethod
    name: str
    ref: str
    threshold: int
    action_ref: None | str | Unset = UNSET
    description: None | str | Unset = UNSET
    pack_ref: None | str | Unset = UNSET
    parameters: list[str] | Unset = UNSET
    tags: list[str] | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        method = self.method.value

        name = self.name

        ref = self.ref

        threshold = self.threshold

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

        pack_ref: None | str | Unset
        if isinstance(self.pack_ref, Unset):
            pack_ref = UNSET
        else:
            pack_ref = self.pack_ref

        parameters: list[str] | Unset = UNSET
        if not isinstance(self.parameters, Unset):
            parameters = self.parameters

        tags: list[str] | Unset = UNSET
        if not isinstance(self.tags, Unset):
            tags = self.tags

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "method": method,
                "name": name,
                "ref": ref,
                "threshold": threshold,
            }
        )
        if action_ref is not UNSET:
            field_dict["action_ref"] = action_ref
        if description is not UNSET:
            field_dict["description"] = description
        if pack_ref is not UNSET:
            field_dict["pack_ref"] = pack_ref
        if parameters is not UNSET:
            field_dict["parameters"] = parameters
        if tags is not UNSET:
            field_dict["tags"] = tags

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        method = PolicyMethod(d.pop("method"))

        name = d.pop("name")

        ref = d.pop("ref")

        threshold = d.pop("threshold")

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

        def _parse_pack_ref(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        pack_ref = _parse_pack_ref(d.pop("pack_ref", UNSET))

        parameters = cast(list[str], d.pop("parameters", UNSET))

        tags = cast(list[str], d.pop("tags", UNSET))

        create_policy_request = cls(
            method=method,
            name=name,
            ref=ref,
            threshold=threshold,
            action_ref=action_ref,
            description=description,
            pack_ref=pack_ref,
            parameters=parameters,
            tags=tags,
        )

        create_policy_request.additional_properties = d
        return create_policy_request

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
