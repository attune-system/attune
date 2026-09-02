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
    from ..models.allocate_file_version_by_ref_request_meta_type_0 import (
        AllocateFileVersionByRefRequestMetaType0,
    )


T = TypeVar("T", bound="AllocateFileVersionByRefRequest")


@_attrs_define
class AllocateFileVersionByRefRequest:
    """Request DTO for the upsert-and-allocate endpoint.

    Looks up an artifact by ref (creating it if it doesn't exist), then
    allocates a new file-backed version and returns the `file_path` where
    the caller should write the file on the shared artifact volume.

    This replaces the multi-step create → 409-handling → allocate dance
    with a single API call.

        Attributes:
            content_type (None | str | Unset): MIME content type for this version (e.g. "text/plain") Example: text/plain.
            created_by (None | str | Unset): Who created this version (e.g. action ref, identity, "system")
            description (None | str | Unset): Optional description
            execution (int | None | Unset): Execution ID to link this artifact to Example: 42.
            meta (AllocateFileVersionByRefRequestMetaType0 | None | Unset): Free-form metadata about this version
            name (None | str | Unset): Human-readable name Example: Demo Log.
            owner (None | str | Unset): Owner identifier (ref string of the owning entity) Example:
                python_example.artifact_demo.
            retention_limit (int | None | Unset): Retention limit (default: 10)
            retention_policy (None | RetentionPolicyType | Unset):
            scope (None | OwnerType | Unset):
            type_ (ArtifactType | None | Unset):
            visibility (ArtifactVisibility | None | Unset):
    """

    content_type: None | str | Unset = UNSET
    created_by: None | str | Unset = UNSET
    description: None | str | Unset = UNSET
    execution: int | None | Unset = UNSET
    meta: AllocateFileVersionByRefRequestMetaType0 | None | Unset = UNSET
    name: None | str | Unset = UNSET
    owner: None | str | Unset = UNSET
    retention_limit: int | None | Unset = UNSET
    retention_policy: None | RetentionPolicyType | Unset = UNSET
    scope: None | OwnerType | Unset = UNSET
    type_: ArtifactType | None | Unset = UNSET
    visibility: ArtifactVisibility | None | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.allocate_file_version_by_ref_request_meta_type_0 import (
            AllocateFileVersionByRefRequestMetaType0,
        )

        content_type: None | str | Unset
        if isinstance(self.content_type, Unset):
            content_type = UNSET
        else:
            content_type = self.content_type

        created_by: None | str | Unset
        if isinstance(self.created_by, Unset):
            created_by = UNSET
        else:
            created_by = self.created_by

        description: None | str | Unset
        if isinstance(self.description, Unset):
            description = UNSET
        else:
            description = self.description

        execution: int | None | Unset
        if isinstance(self.execution, Unset):
            execution = UNSET
        else:
            execution = self.execution

        meta: dict[str, Any] | None | Unset
        if isinstance(self.meta, Unset):
            meta = UNSET
        elif isinstance(self.meta, AllocateFileVersionByRefRequestMetaType0):
            meta = self.meta.to_dict()
        else:
            meta = self.meta

        name: None | str | Unset
        if isinstance(self.name, Unset):
            name = UNSET
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
        if created_by is not UNSET:
            field_dict["created_by"] = created_by
        if description is not UNSET:
            field_dict["description"] = description
        if execution is not UNSET:
            field_dict["execution"] = execution
        if meta is not UNSET:
            field_dict["meta"] = meta
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
        from ..models.allocate_file_version_by_ref_request_meta_type_0 import (
            AllocateFileVersionByRefRequestMetaType0,
        )

        d = dict(src_dict)

        def _parse_content_type(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        content_type = _parse_content_type(d.pop("content_type", UNSET))

        def _parse_created_by(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        created_by = _parse_created_by(d.pop("created_by", UNSET))

        def _parse_description(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        description = _parse_description(d.pop("description", UNSET))

        def _parse_execution(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        execution = _parse_execution(d.pop("execution", UNSET))

        def _parse_meta(
            data: object,
        ) -> AllocateFileVersionByRefRequestMetaType0 | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                meta_type_0 = AllocateFileVersionByRefRequestMetaType0.from_dict(data)

                return meta_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(AllocateFileVersionByRefRequestMetaType0 | None | Unset, data)

        meta = _parse_meta(d.pop("meta", UNSET))

        def _parse_name(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

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

        allocate_file_version_by_ref_request = cls(
            content_type=content_type,
            created_by=created_by,
            description=description,
            execution=execution,
            meta=meta,
            name=name,
            owner=owner,
            retention_limit=retention_limit,
            retention_policy=retention_policy,
            scope=scope,
            type_=type_,
            visibility=visibility,
        )

        allocate_file_version_by_ref_request.additional_properties = d
        return allocate_file_version_by_ref_request

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
