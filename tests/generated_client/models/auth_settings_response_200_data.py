from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="AuthSettingsResponse200Data")


@_attrs_define
class AuthSettingsResponse200Data:
    """Public authentication settings for the login page.

    Attributes:
        authentication_enabled (bool): Whether authentication is enabled for the server. Example: True.
        ldap_enabled (bool): Whether LDAP login is configured and enabled.
        ldap_visible_by_default (bool): Whether LDAP login should be shown by default.
        local_password_enabled (bool): Whether local username/password login is configured. Example: True.
        local_password_visible_by_default (bool): Whether local username/password login should be shown by default.
            Example: True.
        oidc_enabled (bool): Whether OIDC login is configured and enabled.
        oidc_visible_by_default (bool): Whether OIDC login should be shown by default.
        self_registration_enabled (bool): Whether unauthenticated self-service registration is allowed.
        ldap_provider_icon_url (None | str | Unset): Optional icon URL shown beside the provider label. Example:
            https://ldap.example.com/assets/logo.svg.
        ldap_provider_label (None | str | Unset): User-facing provider label for the login button. Example: Company
            LDAP.
        ldap_provider_name (None | str | Unset): Provider name for `?auth=<provider>`. Example: ldap.
        oidc_provider_icon_url (None | str | Unset): Optional icon URL shown beside the provider label. Example:
            https://auth.example.com/assets/logo.svg.
        oidc_provider_label (None | str | Unset): User-facing provider label for the login button. Example: Example SSO.
        oidc_provider_name (None | str | Unset): Provider name for `?auth=<provider>`. Example: sso.
    """

    authentication_enabled: bool
    ldap_enabled: bool
    ldap_visible_by_default: bool
    local_password_enabled: bool
    local_password_visible_by_default: bool
    oidc_enabled: bool
    oidc_visible_by_default: bool
    self_registration_enabled: bool
    ldap_provider_icon_url: None | str | Unset = UNSET
    ldap_provider_label: None | str | Unset = UNSET
    ldap_provider_name: None | str | Unset = UNSET
    oidc_provider_icon_url: None | str | Unset = UNSET
    oidc_provider_label: None | str | Unset = UNSET
    oidc_provider_name: None | str | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        authentication_enabled = self.authentication_enabled

        ldap_enabled = self.ldap_enabled

        ldap_visible_by_default = self.ldap_visible_by_default

        local_password_enabled = self.local_password_enabled

        local_password_visible_by_default = self.local_password_visible_by_default

        oidc_enabled = self.oidc_enabled

        oidc_visible_by_default = self.oidc_visible_by_default

        self_registration_enabled = self.self_registration_enabled

        ldap_provider_icon_url: None | str | Unset
        if isinstance(self.ldap_provider_icon_url, Unset):
            ldap_provider_icon_url = UNSET
        else:
            ldap_provider_icon_url = self.ldap_provider_icon_url

        ldap_provider_label: None | str | Unset
        if isinstance(self.ldap_provider_label, Unset):
            ldap_provider_label = UNSET
        else:
            ldap_provider_label = self.ldap_provider_label

        ldap_provider_name: None | str | Unset
        if isinstance(self.ldap_provider_name, Unset):
            ldap_provider_name = UNSET
        else:
            ldap_provider_name = self.ldap_provider_name

        oidc_provider_icon_url: None | str | Unset
        if isinstance(self.oidc_provider_icon_url, Unset):
            oidc_provider_icon_url = UNSET
        else:
            oidc_provider_icon_url = self.oidc_provider_icon_url

        oidc_provider_label: None | str | Unset
        if isinstance(self.oidc_provider_label, Unset):
            oidc_provider_label = UNSET
        else:
            oidc_provider_label = self.oidc_provider_label

        oidc_provider_name: None | str | Unset
        if isinstance(self.oidc_provider_name, Unset):
            oidc_provider_name = UNSET
        else:
            oidc_provider_name = self.oidc_provider_name

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "authentication_enabled": authentication_enabled,
                "ldap_enabled": ldap_enabled,
                "ldap_visible_by_default": ldap_visible_by_default,
                "local_password_enabled": local_password_enabled,
                "local_password_visible_by_default": local_password_visible_by_default,
                "oidc_enabled": oidc_enabled,
                "oidc_visible_by_default": oidc_visible_by_default,
                "self_registration_enabled": self_registration_enabled,
            }
        )
        if ldap_provider_icon_url is not UNSET:
            field_dict["ldap_provider_icon_url"] = ldap_provider_icon_url
        if ldap_provider_label is not UNSET:
            field_dict["ldap_provider_label"] = ldap_provider_label
        if ldap_provider_name is not UNSET:
            field_dict["ldap_provider_name"] = ldap_provider_name
        if oidc_provider_icon_url is not UNSET:
            field_dict["oidc_provider_icon_url"] = oidc_provider_icon_url
        if oidc_provider_label is not UNSET:
            field_dict["oidc_provider_label"] = oidc_provider_label
        if oidc_provider_name is not UNSET:
            field_dict["oidc_provider_name"] = oidc_provider_name

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        authentication_enabled = d.pop("authentication_enabled")

        ldap_enabled = d.pop("ldap_enabled")

        ldap_visible_by_default = d.pop("ldap_visible_by_default")

        local_password_enabled = d.pop("local_password_enabled")

        local_password_visible_by_default = d.pop("local_password_visible_by_default")

        oidc_enabled = d.pop("oidc_enabled")

        oidc_visible_by_default = d.pop("oidc_visible_by_default")

        self_registration_enabled = d.pop("self_registration_enabled")

        def _parse_ldap_provider_icon_url(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        ldap_provider_icon_url = _parse_ldap_provider_icon_url(
            d.pop("ldap_provider_icon_url", UNSET)
        )

        def _parse_ldap_provider_label(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        ldap_provider_label = _parse_ldap_provider_label(
            d.pop("ldap_provider_label", UNSET)
        )

        def _parse_ldap_provider_name(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        ldap_provider_name = _parse_ldap_provider_name(
            d.pop("ldap_provider_name", UNSET)
        )

        def _parse_oidc_provider_icon_url(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        oidc_provider_icon_url = _parse_oidc_provider_icon_url(
            d.pop("oidc_provider_icon_url", UNSET)
        )

        def _parse_oidc_provider_label(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        oidc_provider_label = _parse_oidc_provider_label(
            d.pop("oidc_provider_label", UNSET)
        )

        def _parse_oidc_provider_name(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        oidc_provider_name = _parse_oidc_provider_name(
            d.pop("oidc_provider_name", UNSET)
        )

        auth_settings_response_200_data = cls(
            authentication_enabled=authentication_enabled,
            ldap_enabled=ldap_enabled,
            ldap_visible_by_default=ldap_visible_by_default,
            local_password_enabled=local_password_enabled,
            local_password_visible_by_default=local_password_visible_by_default,
            oidc_enabled=oidc_enabled,
            oidc_visible_by_default=oidc_visible_by_default,
            self_registration_enabled=self_registration_enabled,
            ldap_provider_icon_url=ldap_provider_icon_url,
            ldap_provider_label=ldap_provider_label,
            ldap_provider_name=ldap_provider_name,
            oidc_provider_icon_url=oidc_provider_icon_url,
            oidc_provider_label=oidc_provider_label,
            oidc_provider_name=oidc_provider_name,
        )

        auth_settings_response_200_data.additional_properties = d
        return auth_settings_response_200_data

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
