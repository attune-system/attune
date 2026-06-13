from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.user_info import UserInfo


T = TypeVar("T", bound="LdapLoginResponse200Data")


@_attrs_define
class LdapLoginResponse200Data:
    """Token response

    Attributes:
        access_token (str): Access token (JWT) Example: eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9....
        expires_in (int): Access token expiration in seconds Example: 3600.
        refresh_token (str): Refresh token Example: eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9....
        token_type (str): Token type (always "Bearer") Example: Bearer.
        user (None | Unset | UserInfo):
    """

    access_token: str
    expires_in: int
    refresh_token: str
    token_type: str
    user: None | Unset | UserInfo = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.user_info import UserInfo

        access_token = self.access_token

        expires_in = self.expires_in

        refresh_token = self.refresh_token

        token_type = self.token_type

        user: dict[str, Any] | None | Unset
        if isinstance(self.user, Unset):
            user = UNSET
        elif isinstance(self.user, UserInfo):
            user = self.user.to_dict()
        else:
            user = self.user

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "access_token": access_token,
                "expires_in": expires_in,
                "refresh_token": refresh_token,
                "token_type": token_type,
            }
        )
        if user is not UNSET:
            field_dict["user"] = user

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.user_info import UserInfo

        d = dict(src_dict)
        access_token = d.pop("access_token")

        expires_in = d.pop("expires_in")

        refresh_token = d.pop("refresh_token")

        token_type = d.pop("token_type")

        def _parse_user(data: object) -> None | Unset | UserInfo:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                user_type_1 = UserInfo.from_dict(data)

                return user_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(None | Unset | UserInfo, data)

        user = _parse_user(d.pop("user", UNSET))

        ldap_login_response_200_data = cls(
            access_token=access_token,
            expires_in=expires_in,
            refresh_token=refresh_token,
            token_type=token_type,
            user=user,
        )

        ldap_login_response_200_data.additional_properties = d
        return ldap_login_response_200_data

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
