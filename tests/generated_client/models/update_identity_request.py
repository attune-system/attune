from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="UpdateIdentityRequest")


@_attrs_define
class UpdateIdentityRequest:
    """
    Attributes:
        attributes (Any | None | Unset):
        display_name (None | str | Unset):
        frozen (bool | None | Unset):
        password (None | str | Unset):
    """

    attributes: Any | None | Unset = UNSET
    display_name: None | str | Unset = UNSET
    frozen: bool | None | Unset = UNSET
    password: None | str | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        attributes: Any | None | Unset
        if isinstance(self.attributes, Unset):
            attributes = UNSET
        else:
            attributes = self.attributes

        display_name: None | str | Unset
        if isinstance(self.display_name, Unset):
            display_name = UNSET
        else:
            display_name = self.display_name

        frozen: bool | None | Unset
        if isinstance(self.frozen, Unset):
            frozen = UNSET
        else:
            frozen = self.frozen

        password: None | str | Unset
        if isinstance(self.password, Unset):
            password = UNSET
        else:
            password = self.password

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update({})
        if attributes is not UNSET:
            field_dict["attributes"] = attributes
        if display_name is not UNSET:
            field_dict["display_name"] = display_name
        if frozen is not UNSET:
            field_dict["frozen"] = frozen
        if password is not UNSET:
            field_dict["password"] = password

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)

        def _parse_attributes(data: object) -> Any | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Any | None | Unset, data)

        attributes = _parse_attributes(d.pop("attributes", UNSET))

        def _parse_display_name(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        display_name = _parse_display_name(d.pop("display_name", UNSET))

        def _parse_frozen(data: object) -> bool | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(bool | None | Unset, data)

        frozen = _parse_frozen(d.pop("frozen", UNSET))

        def _parse_password(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        password = _parse_password(d.pop("password", UNSET))

        update_identity_request = cls(
            attributes=attributes,
            display_name=display_name,
            frozen=frozen,
            password=password,
        )

        update_identity_request.additional_properties = d
        return update_identity_request

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
