from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..models.dashboard_scope_type import DashboardScopeType
from ..models.dashboard_visibility import DashboardVisibility
from ..types import UNSET, Unset
from typing import cast

if TYPE_CHECKING:
  from ..models.nullable_string_patch_type_1 import NullableStringPatchType1
  from ..models.set_string import SetString
  from ..models.update_dashboard_request_spec_type_0 import UpdateDashboardRequestSpecType0





T = TypeVar("T", bound="UpdateDashboardRequest")



@_attrs_define
class UpdateDashboardRequest:
    """ 
        Attributes:
            expected_revision (int):  Example: 3.
            spec (None | UpdateDashboardRequestSpecType0):
            description (None | NullableStringPatchType1 | SetString | str | Unset):
            enabled (bool | None | Unset):  Example: True.
            is_default_home (bool | None | Unset):
            label (None | str | Unset):  Example: Operations Home (Updated).
            scope_ref (None | str | Unset):  Example: core.
            scope_type (DashboardScopeType | None | Unset):
            spec_version (int | None | Unset):  Example: 2.
            tags (list[str] | None | Unset):  Example: ['operations', 'home'].
            visibility (DashboardVisibility | None | Unset):
     """

    expected_revision: int
    spec: None | UpdateDashboardRequestSpecType0
    description: None | NullableStringPatchType1 | SetString | str | Unset = UNSET
    enabled: bool | None | Unset = UNSET
    is_default_home: bool | None | Unset = UNSET
    label: None | str | Unset = UNSET
    scope_ref: None | str | Unset = UNSET
    scope_type: DashboardScopeType | None | Unset = UNSET
    spec_version: int | None | Unset = UNSET
    tags: list[str] | None | Unset = UNSET
    visibility: DashboardVisibility | None | Unset = UNSET





    def to_dict(self) -> dict[str, Any]:
        from ..models.nullable_string_patch_type_1 import NullableStringPatchType1
        from ..models.set_string import SetString
        from ..models.update_dashboard_request_spec_type_0 import UpdateDashboardRequestSpecType0
        expected_revision = self.expected_revision

        spec: dict[str, Any] | None
        if isinstance(self.spec, UpdateDashboardRequestSpecType0):
            spec = self.spec.to_dict()
        else:
            spec = self.spec

        description: dict[str, Any] | None | str | Unset
        if isinstance(self.description, Unset):
            description = UNSET
        elif isinstance(self.description, SetString):
            description = self.description.to_dict()
        elif isinstance(self.description, NullableStringPatchType1):
            description = self.description.to_dict()
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

        label: None | str | Unset
        if isinstance(self.label, Unset):
            label = UNSET
        else:
            label = self.label

        scope_ref: None | str | Unset
        if isinstance(self.scope_ref, Unset):
            scope_ref = UNSET
        else:
            scope_ref = self.scope_ref

        scope_type: None | str | Unset
        if isinstance(self.scope_type, Unset):
            scope_type = UNSET
        elif isinstance(self.scope_type, DashboardScopeType):
            scope_type = self.scope_type.value
        else:
            scope_type = self.scope_type

        spec_version: int | None | Unset
        if isinstance(self.spec_version, Unset):
            spec_version = UNSET
        else:
            spec_version = self.spec_version

        tags: list[str] | None | Unset
        if isinstance(self.tags, Unset):
            tags = UNSET
        elif isinstance(self.tags, list):
            tags = self.tags


        else:
            tags = self.tags

        visibility: None | str | Unset
        if isinstance(self.visibility, Unset):
            visibility = UNSET
        elif isinstance(self.visibility, DashboardVisibility):
            visibility = self.visibility.value
        else:
            visibility = self.visibility


        field_dict: dict[str, Any] = {}

        field_dict.update({
            "expected_revision": expected_revision,
            "spec": spec,
        })
        if description is not UNSET:
            field_dict["description"] = description
        if enabled is not UNSET:
            field_dict["enabled"] = enabled
        if is_default_home is not UNSET:
            field_dict["is_default_home"] = is_default_home
        if label is not UNSET:
            field_dict["label"] = label
        if scope_ref is not UNSET:
            field_dict["scope_ref"] = scope_ref
        if scope_type is not UNSET:
            field_dict["scope_type"] = scope_type
        if spec_version is not UNSET:
            field_dict["spec_version"] = spec_version
        if tags is not UNSET:
            field_dict["tags"] = tags
        if visibility is not UNSET:
            field_dict["visibility"] = visibility

        return field_dict



    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.nullable_string_patch_type_1 import NullableStringPatchType1
        from ..models.set_string import SetString
        from ..models.update_dashboard_request_spec_type_0 import UpdateDashboardRequestSpecType0
        d = dict(src_dict)
        expected_revision = d.pop("expected_revision")

        def _parse_spec(data: object) -> None | UpdateDashboardRequestSpecType0:
            if data is None:
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                spec_type_0 = UpdateDashboardRequestSpecType0.from_dict(data)



                return spec_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(None | UpdateDashboardRequestSpecType0, data)

        spec = _parse_spec(d.pop("spec"))


        def _parse_description(data: object) -> None | NullableStringPatchType1 | SetString | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_nullable_string_patch_set_string = SetString.from_dict(data)



                return componentsschemas_nullable_string_patch_set_string
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_nullable_string_patch_type_1 = NullableStringPatchType1.from_dict(data)



                return componentsschemas_nullable_string_patch_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(None | NullableStringPatchType1 | SetString | str | Unset, data)

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


        def _parse_label(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        label = _parse_label(d.pop("label", UNSET))


        def _parse_scope_ref(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        scope_ref = _parse_scope_ref(d.pop("scope_ref", UNSET))


        def _parse_scope_type(data: object) -> DashboardScopeType | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                scope_type_type_1 = DashboardScopeType(data)



                return scope_type_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(DashboardScopeType | None | Unset, data)

        scope_type = _parse_scope_type(d.pop("scope_type", UNSET))


        def _parse_spec_version(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        spec_version = _parse_spec_version(d.pop("spec_version", UNSET))


        def _parse_tags(data: object) -> list[str] | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, list):
                    raise TypeError()
                tags_type_0 = cast(list[str], data)

                return tags_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(list[str] | None | Unset, data)

        tags = _parse_tags(d.pop("tags", UNSET))


        def _parse_visibility(data: object) -> DashboardVisibility | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                visibility_type_1 = DashboardVisibility(data)



                return visibility_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(DashboardVisibility | None | Unset, data)

        visibility = _parse_visibility(d.pop("visibility", UNSET))


        update_dashboard_request = cls(
            expected_revision=expected_revision,
            spec=spec,
            description=description,
            enabled=enabled,
            is_default_home=is_default_home,
            label=label,
            scope_ref=scope_ref,
            scope_type=scope_type,
            spec_version=spec_version,
            tags=tags,
            visibility=visibility,
        )

        return update_dashboard_request

