from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.effective_permission_response import EffectivePermissionResponse
    from ..models.provider_profile_response import ProviderProfileResponse


T = TypeVar("T", bound="ApiResponseCurrentUserResponseData")


@_attrs_define
class ApiResponseCurrentUserResponseData:
    """Current user response

    Attributes:
        assigned_permission_set_refs (list[str]): Permission set refs assigned to this identity, including role-derived
            assignments.
        auth_provider (str): Authentication provider backing this identity. Example: local.
        can_change_password (bool): Whether this identity can change its password through Attune. Example: True.
        effective_permissions (list[EffectivePermissionResponse]): Effective resource-level permissions assigned to this
            identity.
        id (int): Identity ID Example: 1.
        is_local (bool): Whether this identity is managed locally by Attune. Example: True.
        login (str): Identity login Example: admin.
        display_name (None | str | Unset): Display name Example: Administrator.
        provider_profile (None | ProviderProfileResponse | Unset):
    """

    assigned_permission_set_refs: list[str]
    auth_provider: str
    can_change_password: bool
    effective_permissions: list[EffectivePermissionResponse]
    id: int
    is_local: bool
    login: str
    display_name: None | str | Unset = UNSET
    provider_profile: None | ProviderProfileResponse | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.provider_profile_response import ProviderProfileResponse

        assigned_permission_set_refs = self.assigned_permission_set_refs

        auth_provider = self.auth_provider

        can_change_password = self.can_change_password

        effective_permissions = []
        for effective_permissions_item_data in self.effective_permissions:
            effective_permissions_item = effective_permissions_item_data.to_dict()
            effective_permissions.append(effective_permissions_item)

        id = self.id

        is_local = self.is_local

        login = self.login

        display_name: None | str | Unset
        if isinstance(self.display_name, Unset):
            display_name = UNSET
        else:
            display_name = self.display_name

        provider_profile: dict[str, Any] | None | Unset
        if isinstance(self.provider_profile, Unset):
            provider_profile = UNSET
        elif isinstance(self.provider_profile, ProviderProfileResponse):
            provider_profile = self.provider_profile.to_dict()
        else:
            provider_profile = self.provider_profile

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "assigned_permission_set_refs": assigned_permission_set_refs,
                "auth_provider": auth_provider,
                "can_change_password": can_change_password,
                "effective_permissions": effective_permissions,
                "id": id,
                "is_local": is_local,
                "login": login,
            }
        )
        if display_name is not UNSET:
            field_dict["display_name"] = display_name
        if provider_profile is not UNSET:
            field_dict["provider_profile"] = provider_profile

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.effective_permission_response import EffectivePermissionResponse
        from ..models.provider_profile_response import ProviderProfileResponse

        d = dict(src_dict)
        assigned_permission_set_refs = cast(
            list[str], d.pop("assigned_permission_set_refs")
        )

        auth_provider = d.pop("auth_provider")

        can_change_password = d.pop("can_change_password")

        effective_permissions = []
        _effective_permissions = d.pop("effective_permissions")
        for effective_permissions_item_data in _effective_permissions:
            effective_permissions_item = EffectivePermissionResponse.from_dict(
                effective_permissions_item_data
            )

            effective_permissions.append(effective_permissions_item)

        id = d.pop("id")

        is_local = d.pop("is_local")

        login = d.pop("login")

        def _parse_display_name(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        display_name = _parse_display_name(d.pop("display_name", UNSET))

        def _parse_provider_profile(
            data: object,
        ) -> None | ProviderProfileResponse | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                provider_profile_type_1 = ProviderProfileResponse.from_dict(data)

                return provider_profile_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(None | ProviderProfileResponse | Unset, data)

        provider_profile = _parse_provider_profile(d.pop("provider_profile", UNSET))

        api_response_current_user_response_data = cls(
            assigned_permission_set_refs=assigned_permission_set_refs,
            auth_provider=auth_provider,
            can_change_password=can_change_password,
            effective_permissions=effective_permissions,
            id=id,
            is_local=is_local,
            login=login,
            display_name=display_name,
            provider_profile=provider_profile,
        )

        api_response_current_user_response_data.additional_properties = d
        return api_response_current_user_response_data

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
