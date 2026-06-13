from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="ProviderProfileResponse")


@_attrs_define
class ProviderProfileResponse:
    """Sanitized user information supplied by an external identity provider.

    Attributes:
        groups (list[str]): Provider groups associated with this identity.
        provider (str): Provider backing this identity. Example: oidc.
        display_name (None | str | Unset): Provider-issued display name. Example: Jane Operator.
        distinguished_name (None | str | Unset): LDAP distinguished name, when available. Example:
            uid=jane,ou=people,dc=example,dc=com.
        email (None | str | Unset): Provider-issued email address. Example: jane.operator@example.com.
        email_verified (bool | None | Unset): Whether the provider reported the email address as verified. Example:
            True.
        issuer (None | str | Unset): OIDC issuer URL, when available. Example: https://idp.example.com.
        login (None | str | Unset): Provider-issued login or preferred username. Example: jane.operator.
        subject (None | str | Unset): OIDC subject identifier, when available. Example: 00u123456789.
    """

    groups: list[str]
    provider: str
    display_name: None | str | Unset = UNSET
    distinguished_name: None | str | Unset = UNSET
    email: None | str | Unset = UNSET
    email_verified: bool | None | Unset = UNSET
    issuer: None | str | Unset = UNSET
    login: None | str | Unset = UNSET
    subject: None | str | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        groups = self.groups

        provider = self.provider

        display_name: None | str | Unset
        if isinstance(self.display_name, Unset):
            display_name = UNSET
        else:
            display_name = self.display_name

        distinguished_name: None | str | Unset
        if isinstance(self.distinguished_name, Unset):
            distinguished_name = UNSET
        else:
            distinguished_name = self.distinguished_name

        email: None | str | Unset
        if isinstance(self.email, Unset):
            email = UNSET
        else:
            email = self.email

        email_verified: bool | None | Unset
        if isinstance(self.email_verified, Unset):
            email_verified = UNSET
        else:
            email_verified = self.email_verified

        issuer: None | str | Unset
        if isinstance(self.issuer, Unset):
            issuer = UNSET
        else:
            issuer = self.issuer

        login: None | str | Unset
        if isinstance(self.login, Unset):
            login = UNSET
        else:
            login = self.login

        subject: None | str | Unset
        if isinstance(self.subject, Unset):
            subject = UNSET
        else:
            subject = self.subject

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "groups": groups,
                "provider": provider,
            }
        )
        if display_name is not UNSET:
            field_dict["display_name"] = display_name
        if distinguished_name is not UNSET:
            field_dict["distinguished_name"] = distinguished_name
        if email is not UNSET:
            field_dict["email"] = email
        if email_verified is not UNSET:
            field_dict["email_verified"] = email_verified
        if issuer is not UNSET:
            field_dict["issuer"] = issuer
        if login is not UNSET:
            field_dict["login"] = login
        if subject is not UNSET:
            field_dict["subject"] = subject

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        groups = cast(list[str], d.pop("groups"))

        provider = d.pop("provider")

        def _parse_display_name(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        display_name = _parse_display_name(d.pop("display_name", UNSET))

        def _parse_distinguished_name(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        distinguished_name = _parse_distinguished_name(
            d.pop("distinguished_name", UNSET)
        )

        def _parse_email(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        email = _parse_email(d.pop("email", UNSET))

        def _parse_email_verified(data: object) -> bool | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(bool | None | Unset, data)

        email_verified = _parse_email_verified(d.pop("email_verified", UNSET))

        def _parse_issuer(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        issuer = _parse_issuer(d.pop("issuer", UNSET))

        def _parse_login(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        login = _parse_login(d.pop("login", UNSET))

        def _parse_subject(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        subject = _parse_subject(d.pop("subject", UNSET))

        provider_profile_response = cls(
            groups=groups,
            provider=provider,
            display_name=display_name,
            distinguished_name=distinguished_name,
            email=email,
            email_verified=email_verified,
            issuer=issuer,
            login=login,
            subject=subject,
        )

        provider_profile_response.additional_properties = d
        return provider_profile_response

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
