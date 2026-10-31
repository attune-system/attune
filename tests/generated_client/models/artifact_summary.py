from __future__ import annotations

import datetime
from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, Self, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.artifact_classification import ArtifactClassification
from ..models.artifact_type import ArtifactType
from ..models.artifact_visibility import ArtifactVisibility
from ..models.owner_type import OwnerType
from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.artifact_version_summary import ArtifactVersionSummary


T = TypeVar("T", bound="ArtifactSummary")


@_attrs_define
class ArtifactSummary:
    """Simplified artifact for list endpoints

    Attributes:
        classification (ArtifactClassification):
        created (datetime.datetime): Creation timestamp
        id (int): Artifact ID
        owner (str): Owner identifier
        ref (str): Artifact reference
        scope (OwnerType):
        type_ (ArtifactType):
        updated (datetime.datetime): Last update timestamp
        visibility (ArtifactVisibility): Visibility level for artifacts.
            - `Public`: viewable by all authenticated users on the platform.
            - `Private`: restricted based on the artifact's `scope` and `owner` fields.
              Full RBAC enforcement is deferred; for now the field enables filtering.
        content_type (None | str | Unset): MIME content type
        name (None | str | Unset): Human-readable name
        selected_version (ArtifactVersionSummary | None | Unset):
        size_bytes (int | None | Unset): Display size: execution-selected when scoped; omitted for global runtime logs.
    """

    classification: ArtifactClassification
    created: datetime.datetime
    id: int
    owner: str
    ref: str
    scope: OwnerType
    type_: ArtifactType
    updated: datetime.datetime
    visibility: ArtifactVisibility
    content_type: None | str | Unset = UNSET
    name: None | str | Unset = UNSET
    selected_version: ArtifactVersionSummary | None | Unset = UNSET
    size_bytes: int | None | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.artifact_version_summary import ArtifactVersionSummary

        classification = self.classification.value

        created = self.created.isoformat()

        id = self.id

        owner = self.owner

        ref = self.ref

        scope = self.scope.value

        type_ = self.type_.value

        updated = self.updated.isoformat()

        visibility = self.visibility.value

        content_type: None | str | Unset
        if isinstance(self.content_type, Unset):
            content_type = UNSET
        else:
            content_type = self.content_type

        name: None | str | Unset
        if isinstance(self.name, Unset):
            name = UNSET
        else:
            name = self.name

        selected_version: dict[str, Any] | None | Unset
        if isinstance(self.selected_version, Unset):
            selected_version = UNSET
        elif isinstance(self.selected_version, ArtifactVersionSummary):
            selected_version = self.selected_version.to_dict()
        else:
            selected_version = self.selected_version

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
                "scope": scope,
                "type": type_,
                "updated": updated,
                "visibility": visibility,
            }
        )
        if content_type is not UNSET:
            field_dict["content_type"] = content_type
        if name is not UNSET:
            field_dict["name"] = name
        if selected_version is not UNSET:
            field_dict["selected_version"] = selected_version
        if size_bytes is not UNSET:
            field_dict["size_bytes"] = size_bytes

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        from ..models.artifact_version_summary import ArtifactVersionSummary

        d = dict(src_dict)
        classification = ArtifactClassification(d.pop("classification"))

        created = datetime.datetime.fromisoformat(d.pop("created"))

        id = d.pop("id")

        owner = d.pop("owner")

        ref = d.pop("ref")

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

        def _parse_name(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        name = _parse_name(d.pop("name", UNSET))

        def _parse_selected_version(
            data: object,
        ) -> ArtifactVersionSummary | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                selected_version_type_1 = ArtifactVersionSummary.from_dict(data)

                return selected_version_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(ArtifactVersionSummary | None | Unset, data)

        selected_version = _parse_selected_version(d.pop("selected_version", UNSET))

        def _parse_size_bytes(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        size_bytes = _parse_size_bytes(d.pop("size_bytes", UNSET))

        artifact_summary = cls(
            classification=classification,
            created=created,
            id=id,
            owner=owner,
            ref=ref,
            scope=scope,
            type_=type_,
            updated=updated,
            visibility=visibility,
            content_type=content_type,
            name=name,
            selected_version=selected_version,
            size_bytes=size_bytes,
        )

        artifact_summary.additional_properties = d
        return artifact_summary

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
