from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from typing_extensions import Self

from ..models.dashboard_scope_type import DashboardScopeType
from ..models.dashboard_visibility import DashboardVisibility
from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.create_dashboard_request_spec import CreateDashboardRequestSpec


T = TypeVar("T", bound="CreateDashboardRequest")


@_attrs_define
class CreateDashboardRequest:
    """
    Attributes:
        label (str):  Example: Operations Home.
        ref (str):  Example: core.operations_home.
        scope_type (DashboardScopeType):  Default: DashboardScopeType.GLOBAL.
        spec (CreateDashboardRequestSpec):
        visibility (DashboardVisibility):
        description (None | str | Unset):  Example: Operational overview for the platform.
        enabled (bool | None | Unset):  Default: True. Example: True.
        is_default_home (bool | None | Unset):  Default: False. Example: False.
        scope_ref (None | str | Unset):  Example: global.
        spec_version (int | None | Unset):  Default: 1. Example: 1.
        tags (list[str] | Unset):  Example: ['operations', 'overview'].
    """

    label: str
    ref: str
    spec: CreateDashboardRequestSpec
    visibility: DashboardVisibility
    scope_type: DashboardScopeType = DashboardScopeType.GLOBAL
    description: None | str | Unset = UNSET
    enabled: bool | None | Unset = True
    is_default_home: bool | None | Unset = False
    scope_ref: None | str | Unset = UNSET
    spec_version: int | None | Unset = 1
    tags: list[str] | Unset = UNSET

    def to_dict(self) -> dict[str, Any]:
        label = self.label

        ref = self.ref

        scope_type = self.scope_type.value

        spec = self.spec.to_dict()

        visibility = self.visibility.value

        description: None | str | Unset
        if isinstance(self.description, Unset):
            description = UNSET
        else:
            description = self.description

        enabled: bool | None | Unset
        if isinstance(self.enabled, Unset):
            enabled = UNSET
        else:
            enabled = self.enabled

        is_default_home: bool | None | Unset
        if isinstance(self.is_default_home, Unset):
            is_default_home = UNSET
        else:
            is_default_home = self.is_default_home

        scope_ref: None | str | Unset
        if isinstance(self.scope_ref, Unset):
            scope_ref = UNSET
        else:
            scope_ref = self.scope_ref

        spec_version: int | None | Unset
        if isinstance(self.spec_version, Unset):
            spec_version = UNSET
        else:
            spec_version = self.spec_version

        tags: list[str] | Unset = UNSET
        if not isinstance(self.tags, Unset):
            tags = self.tags

        field_dict: dict[str, Any] = {}

        field_dict.update(
            {
                "label": label,
                "ref": ref,
                "scope_type": scope_type,
                "spec": spec,
                "visibility": visibility,
            }
        )
        if description is not UNSET:
            field_dict["description"] = description
        if enabled is not UNSET:
            field_dict["enabled"] = enabled
        if is_default_home is not UNSET:
            field_dict["is_default_home"] = is_default_home
        if scope_ref is not UNSET:
            field_dict["scope_ref"] = scope_ref
        if spec_version is not UNSET:
            field_dict["spec_version"] = spec_version
        if tags is not UNSET:
            field_dict["tags"] = tags

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        from ..models.create_dashboard_request_spec import (
            CreateDashboardRequestSpec,
        )

        d = dict(src_dict)
        label = d.pop("label")

        ref = d.pop("ref")

        scope_type = DashboardScopeType(d.pop("scope_type"))

        spec = CreateDashboardRequestSpec.from_dict(d.pop("spec"))

        visibility = DashboardVisibility(d.pop("visibility"))

        def _parse_description(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        description = _parse_description(d.pop("description", UNSET))

        def _parse_enabled(data: object) -> bool | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(bool | None | Unset, data)

        enabled = _parse_enabled(d.pop("enabled", UNSET))

        def _parse_is_default_home(data: object) -> bool | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(bool | None | Unset, data)

        is_default_home = _parse_is_default_home(d.pop("is_default_home", UNSET))

        def _parse_scope_ref(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        scope_ref = _parse_scope_ref(d.pop("scope_ref", UNSET))

        def _parse_spec_version(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        spec_version = _parse_spec_version(d.pop("spec_version", UNSET))

        tags = cast(list[str], d.pop("tags", UNSET))

        create_dashboard_request = cls(
            label=label,
            ref=ref,
            scope_type=scope_type,
            spec=spec,
            visibility=visibility,
            description=description,
            enabled=enabled,
            is_default_home=is_default_home,
            scope_ref=scope_ref,
            spec_version=spec_version,
            tags=tags,
        )

        return create_dashboard_request
