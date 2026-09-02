from __future__ import annotations

import datetime
from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

from ..models.artifact_classification import ArtifactClassification
from ..models.artifact_type import ArtifactType
from ..models.artifact_visibility import ArtifactVisibility
from ..models.owner_type import OwnerType
from ..models.retention_policy_type import RetentionPolicyType
from ..types import UNSET, Unset

T = TypeVar("T", bound="CreateArtifactResponse201Data")


@_attrs_define
class CreateArtifactResponse201Data:
    """Response DTO for artifact information

    Attributes:
        classification (ArtifactClassification):
        created (datetime.datetime): Creation timestamp
        id (int): Artifact ID Example: 1.
        owner (str): Owner identifier Example: mypack.deploy.
        ref (str): Artifact reference Example: mypack.build_log.
        retention_limit (int): Retention limit Example: 5.
        retention_policy (RetentionPolicyType):
        scope (OwnerType):
        type_ (ArtifactType):
        updated (datetime.datetime): Last update timestamp
        visibility (ArtifactVisibility): Visibility level for artifacts.
            - `Public`: viewable by all authenticated users on the platform.
            - `Private`: restricted based on the artifact's `scope` and `owner` fields.
              Full RBAC enforcement is deferred; for now the field enables filtering.
        content_type (None | str | Unset): MIME content type Example: text/plain.
        data (Any | None | Unset):
        description (None | str | Unset): Description
        name (None | str | Unset): Human-readable name Example: Build Log.
        size_bytes (int | None | Unset): Size of the latest version in bytes
    """

    classification: ArtifactClassification
    created: datetime.datetime
    id: int
    owner: str
    ref: str
    retention_limit: int
    retention_policy: RetentionPolicyType
    scope: OwnerType
    type_: ArtifactType
    updated: datetime.datetime
    visibility: ArtifactVisibility
    content_type: None | str | Unset = UNSET
    data: Any | None | Unset = UNSET
    description: None | str | Unset = UNSET
    name: None | str | Unset = UNSET
    size_bytes: int | None | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        classification = self.classification.value

        created = self.created.isoformat()

        id = self.id

        owner = self.owner

        ref = self.ref

        retention_limit = self.retention_limit

        retention_policy = self.retention_policy.value

        scope = self.scope.value

        type_ = self.type_.value

        updated = self.updated.isoformat()

        visibility = self.visibility.value

        content_type: None | str | Unset
        if isinstance(self.content_type, Unset):
            content_type = UNSET
        else:
            content_type = self.content_type

        data: Any | None | Unset
        if isinstance(self.data, Unset):
            data = UNSET
        else:
            data = self.data

        description: None | str | Unset
        if isinstance(self.description, Unset):
            description = UNSET
        else:
            description = self.description

        name: None | str | Unset
        if isinstance(self.name, Unset):
            name = UNSET
        else:
            name = self.name

        size_bytes: int | None | Unset
        if isinstance(self.size_bytes, Unset):
            size_bytes = UNSET
        else:
            size_bytes = self.size_bytes

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "classification": classification,
                "created": created,
                "id": id,
                "owner": owner,
                "ref": ref,
                "retention_limit": retention_limit,
                "retention_policy": retention_policy,
                "scope": scope,
                "type": type_,
                "updated": updated,
                "visibility": visibility,
            }
        )
        if content_type is not UNSET:
            field_dict["content_type"] = content_type
        if data is not UNSET:
            field_dict["data"] = data
        if description is not UNSET:
            field_dict["description"] = description
        if name is not UNSET:
            field_dict["name"] = name
        if size_bytes is not UNSET:
            field_dict["size_bytes"] = size_bytes

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        d = dict(src_dict)
        classification = ArtifactClassification(d.pop("classification"))

        created = datetime.datetime.fromisoformat(d.pop("created"))

        id = d.pop("id")

        owner = d.pop("owner")

        ref = d.pop("ref")

        retention_limit = d.pop("retention_limit")

        retention_policy = RetentionPolicyType(d.pop("retention_policy"))

        scope = OwnerType(d.pop("scope"))

        type_ = ArtifactType(d.pop("type"))

        updated = datetime.datetime.fromisoformat(d.pop("updated"))

        visibility = ArtifactVisibility(d.pop("visibility"))

        def _parse_content_type(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        content_type = _parse_content_type(d.pop("content_type", UNSET))

        def _parse_data(data: object) -> Any | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Any | None | Unset, data)

        data = _parse_data(d.pop("data", UNSET))

        def _parse_description(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        description = _parse_description(d.pop("description", UNSET))

        def _parse_name(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        name = _parse_name(d.pop("name", UNSET))

        def _parse_size_bytes(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        size_bytes = _parse_size_bytes(d.pop("size_bytes", UNSET))

        create_artifact_response_201_data = cls(
            classification=classification,
            created=created,
            id=id,
            owner=owner,
            ref=ref,
            retention_limit=retention_limit,
            retention_policy=retention_policy,
            scope=scope,
            type_=type_,
            updated=updated,
            visibility=visibility,
            content_type=content_type,
            data=data,
            description=description,
            name=name,
            size_bytes=size_bytes,
        )

        create_artifact_response_201_data.additional_properties = d
        return create_artifact_response_201_data

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
