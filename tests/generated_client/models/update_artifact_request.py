from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

from ..models.artifact_type import ArtifactType
from ..models.artifact_visibility import ArtifactVisibility
from ..models.owner_type import OwnerType
from ..models.retention_policy_type import RetentionPolicyType
from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.artifact_json_patch_type_0 import ArtifactJsonPatchType0
    from ..models.artifact_json_patch_type_1 import ArtifactJsonPatchType1
    from ..models.artifact_string_patch_type_0 import ArtifactStringPatchType0
    from ..models.artifact_string_patch_type_1 import ArtifactStringPatchType1


T = TypeVar("T", bound="UpdateArtifactRequest")


@_attrs_define
class UpdateArtifactRequest:
    """Request DTO for updating an existing artifact

    Attributes:
        content_type (ArtifactStringPatchType0 | ArtifactStringPatchType1 | None | Unset):
        data (ArtifactJsonPatchType0 | ArtifactJsonPatchType1 | None | Unset):
        description (ArtifactStringPatchType0 | ArtifactStringPatchType1 | None | Unset):
        name (ArtifactStringPatchType0 | ArtifactStringPatchType1 | None | Unset):
        owner (None | str | Unset): Updated owner identifier
        retention_limit (int | None | Unset): Updated retention limit
        retention_policy (None | RetentionPolicyType | Unset):
        scope (None | OwnerType | Unset):
        type_ (ArtifactType | None | Unset):
        visibility (ArtifactVisibility | None | Unset):
    """

    content_type: ArtifactStringPatchType0 | ArtifactStringPatchType1 | None | Unset = (
        UNSET
    )
    data: ArtifactJsonPatchType0 | ArtifactJsonPatchType1 | None | Unset = UNSET
    description: ArtifactStringPatchType0 | ArtifactStringPatchType1 | None | Unset = (
        UNSET
    )
    name: ArtifactStringPatchType0 | ArtifactStringPatchType1 | None | Unset = UNSET
    owner: None | str | Unset = UNSET
    retention_limit: int | None | Unset = UNSET
    retention_policy: None | RetentionPolicyType | Unset = UNSET
    scope: None | OwnerType | Unset = UNSET
    type_: ArtifactType | None | Unset = UNSET
    visibility: ArtifactVisibility | None | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.artifact_json_patch_type_0 import (
            ArtifactJsonPatchType0,
        )
        from ..models.artifact_json_patch_type_1 import (
            ArtifactJsonPatchType1,
        )
        from ..models.artifact_string_patch_type_0 import (
            ArtifactStringPatchType0,
        )
        from ..models.artifact_string_patch_type_1 import (
            ArtifactStringPatchType1,
        )

        content_type: dict[str, Any] | None | Unset
        if isinstance(self.content_type, Unset):
            content_type = UNSET
        elif isinstance(self.content_type, ArtifactStringPatchType0) or isinstance(
            self.content_type, ArtifactStringPatchType1
        ):
            content_type = self.content_type.to_dict()
        else:
            content_type = self.content_type

        data: dict[str, Any] | None | Unset
        if isinstance(self.data, Unset):
            data = UNSET
        elif isinstance(self.data, ArtifactJsonPatchType0) or isinstance(
            self.data, ArtifactJsonPatchType1
        ):
            data = self.data.to_dict()
        else:
            data = self.data

        description: dict[str, Any] | None | Unset
        if isinstance(self.description, Unset):
            description = UNSET
        elif isinstance(self.description, ArtifactStringPatchType0) or isinstance(
            self.description, ArtifactStringPatchType1
        ):
            description = self.description.to_dict()
        else:
            description = self.description

        name: dict[str, Any] | None | Unset
        if isinstance(self.name, Unset):
            name = UNSET
        elif isinstance(self.name, ArtifactStringPatchType0) or isinstance(
            self.name, ArtifactStringPatchType1
        ):
            name = self.name.to_dict()
        else:
            name = self.name

        owner: None | str | Unset
        if isinstance(self.owner, Unset):
            owner = UNSET
        else:
            owner = self.owner

        retention_limit: int | None | Unset
        if isinstance(self.retention_limit, Unset):
            retention_limit = UNSET
        else:
            retention_limit = self.retention_limit

        retention_policy: None | str | Unset
        if isinstance(self.retention_policy, Unset):
            retention_policy = UNSET
        elif isinstance(self.retention_policy, RetentionPolicyType):
            retention_policy = self.retention_policy.value
        else:
            retention_policy = self.retention_policy

        scope: None | str | Unset
        if isinstance(self.scope, Unset):
            scope = UNSET
        elif isinstance(self.scope, OwnerType):
            scope = self.scope.value
        else:
            scope = self.scope

        type_: None | str | Unset
        if isinstance(self.type_, Unset):
            type_ = UNSET
        elif isinstance(self.type_, ArtifactType):
            type_ = self.type_.value
        else:
            type_ = self.type_

        visibility: None | str | Unset
        if isinstance(self.visibility, Unset):
            visibility = UNSET
        elif isinstance(self.visibility, ArtifactVisibility):
            visibility = self.visibility.value
        else:
            visibility = self.visibility

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update({})
        if content_type is not UNSET:
            field_dict["content_type"] = content_type
        if data is not UNSET:
            field_dict["data"] = data
        if description is not UNSET:
            field_dict["description"] = description
        if name is not UNSET:
            field_dict["name"] = name
        if owner is not UNSET:
            field_dict["owner"] = owner
        if retention_limit is not UNSET:
            field_dict["retention_limit"] = retention_limit
        if retention_policy is not UNSET:
            field_dict["retention_policy"] = retention_policy
        if scope is not UNSET:
            field_dict["scope"] = scope
        if type_ is not UNSET:
            field_dict["type"] = type_
        if visibility is not UNSET:
            field_dict["visibility"] = visibility

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        from ..models.artifact_json_patch_type_0 import (
            ArtifactJsonPatchType0,
        )
        from ..models.artifact_json_patch_type_1 import (
            ArtifactJsonPatchType1,
        )
        from ..models.artifact_string_patch_type_0 import (
            ArtifactStringPatchType0,
        )
        from ..models.artifact_string_patch_type_1 import (
            ArtifactStringPatchType1,
        )

        d = dict(src_dict)

        def _parse_content_type(
            data: object,
        ) -> ArtifactStringPatchType0 | ArtifactStringPatchType1 | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_artifact_string_patch_type_0 = (
                    ArtifactStringPatchType0.from_dict(data)
                )

                return componentsschemas_artifact_string_patch_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_artifact_string_patch_type_1 = (
                    ArtifactStringPatchType1.from_dict(data)
                )

                return componentsschemas_artifact_string_patch_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(
                ArtifactStringPatchType0 | ArtifactStringPatchType1 | None | Unset, data
            )

        content_type = _parse_content_type(d.pop("content_type", UNSET))

        def _parse_data(
            data: object,
        ) -> ArtifactJsonPatchType0 | ArtifactJsonPatchType1 | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_artifact_json_patch_type_0 = (
                    ArtifactJsonPatchType0.from_dict(data)
                )

                return componentsschemas_artifact_json_patch_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_artifact_json_patch_type_1 = (
                    ArtifactJsonPatchType1.from_dict(data)
                )

                return componentsschemas_artifact_json_patch_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(
                ArtifactJsonPatchType0 | ArtifactJsonPatchType1 | None | Unset, data
            )

        data = _parse_data(d.pop("data", UNSET))

        def _parse_description(
            data: object,
        ) -> ArtifactStringPatchType0 | ArtifactStringPatchType1 | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_artifact_string_patch_type_0 = (
                    ArtifactStringPatchType0.from_dict(data)
                )

                return componentsschemas_artifact_string_patch_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_artifact_string_patch_type_1 = (
                    ArtifactStringPatchType1.from_dict(data)
                )

                return componentsschemas_artifact_string_patch_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(
                ArtifactStringPatchType0 | ArtifactStringPatchType1 | None | Unset, data
            )

        description = _parse_description(d.pop("description", UNSET))

        def _parse_name(
            data: object,
        ) -> ArtifactStringPatchType0 | ArtifactStringPatchType1 | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_artifact_string_patch_type_0 = (
                    ArtifactStringPatchType0.from_dict(data)
                )

                return componentsschemas_artifact_string_patch_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_artifact_string_patch_type_1 = (
                    ArtifactStringPatchType1.from_dict(data)
                )

                return componentsschemas_artifact_string_patch_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(
                ArtifactStringPatchType0 | ArtifactStringPatchType1 | None | Unset, data
            )

        name = _parse_name(d.pop("name", UNSET))

        def _parse_owner(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        owner = _parse_owner(d.pop("owner", UNSET))

        def _parse_retention_limit(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        retention_limit = _parse_retention_limit(d.pop("retention_limit", UNSET))

        def _parse_retention_policy(data: object) -> None | RetentionPolicyType | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                retention_policy_type_1 = RetentionPolicyType(data)

                return retention_policy_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(None | RetentionPolicyType | Unset, data)

        retention_policy = _parse_retention_policy(d.pop("retention_policy", UNSET))

        def _parse_scope(data: object) -> None | OwnerType | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                scope_type_1 = OwnerType(data)

                return scope_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(None | OwnerType | Unset, data)

        scope = _parse_scope(d.pop("scope", UNSET))

        def _parse_type_(data: object) -> ArtifactType | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                type_type_1 = ArtifactType(data)

                return type_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(ArtifactType | None | Unset, data)

        type_ = _parse_type_(d.pop("type", UNSET))

        def _parse_visibility(data: object) -> ArtifactVisibility | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                visibility_type_1 = ArtifactVisibility(data)

                return visibility_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(ArtifactVisibility | None | Unset, data)

        visibility = _parse_visibility(d.pop("visibility", UNSET))

        update_artifact_request = cls(
            content_type=content_type,
            data=data,
            description=description,
            name=name,
            owner=owner,
            retention_limit=retention_limit,
            retention_policy=retention_policy,
            scope=scope,
            type_=type_,
            visibility=visibility,
        )

        update_artifact_request.additional_properties = d
        return update_artifact_request

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
