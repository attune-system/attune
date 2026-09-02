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
    from ..models.create_artifact_request_data_type_0 import (
        CreateArtifactRequestDataType0,
    )


T = TypeVar("T", bound="CreateArtifactRequest")


@_attrs_define
class CreateArtifactRequest:
    """Request DTO for creating a new artifact

    Attributes:
        owner (str): Owner identifier (ref string of the owning entity) Example: mypack.deploy.
        ref (str): Artifact reference (unique identifier, e.g. "build.log", "test.results") Example: mypack.build_log.
        scope (OwnerType):
        type_ (ArtifactType):
        content_type (None | str | Unset): MIME content type (e.g. "text/plain", "application/json") Example:
            text/plain.
        data (CreateArtifactRequestDataType0 | None | Unset): Initial structured data (for progress-type artifacts or
            metadata)
        description (None | str | Unset): Optional description Example: Output log from the build action.
        name (None | str | Unset): Human-readable name Example: Build Log.
        retention_limit (int | None | Unset): Retention limit (number of versions, days, hours, or minutes depending on
            policy).
            If omitted, execution/action/sensor defaults may apply. Example: 5.
        retention_policy (None | RetentionPolicyType | Unset):
        visibility (ArtifactVisibility | None | Unset):
    """

    owner: str
    ref: str
    scope: OwnerType
    type_: ArtifactType
    content_type: None | str | Unset = UNSET
    data: CreateArtifactRequestDataType0 | None | Unset = UNSET
    description: None | str | Unset = UNSET
    name: None | str | Unset = UNSET
    retention_limit: int | None | Unset = UNSET
    retention_policy: None | RetentionPolicyType | Unset = UNSET
    visibility: ArtifactVisibility | None | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.create_artifact_request_data_type_0 import (
            CreateArtifactRequestDataType0,
        )

        owner = self.owner

        ref = self.ref

        scope = self.scope.value

        type_ = self.type_.value

        content_type: None | str | Unset
        if isinstance(self.content_type, Unset):
            content_type = UNSET
        else:
            content_type = self.content_type

        data: dict[str, Any] | None | Unset
        if isinstance(self.data, Unset):
            data = UNSET
        elif isinstance(self.data, CreateArtifactRequestDataType0):
            data = self.data.to_dict()
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

        visibility: None | str | Unset
        if isinstance(self.visibility, Unset):
            visibility = UNSET
        elif isinstance(self.visibility, ArtifactVisibility):
            visibility = self.visibility.value
        else:
            visibility = self.visibility

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "owner": owner,
                "ref": ref,
                "scope": scope,
                "type": type_,
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
        if retention_limit is not UNSET:
            field_dict["retention_limit"] = retention_limit
        if retention_policy is not UNSET:
            field_dict["retention_policy"] = retention_policy
        if visibility is not UNSET:
            field_dict["visibility"] = visibility

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        from ..models.create_artifact_request_data_type_0 import (
            CreateArtifactRequestDataType0,
        )

        d = dict(src_dict)
        owner = d.pop("owner")

        ref = d.pop("ref")

        scope = OwnerType(d.pop("scope"))

        type_ = ArtifactType(d.pop("type"))

        def _parse_content_type(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        content_type = _parse_content_type(d.pop("content_type", UNSET))

        def _parse_data(data: object) -> CreateArtifactRequestDataType0 | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                data_type_0 = CreateArtifactRequestDataType0.from_dict(data)

                return data_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(CreateArtifactRequestDataType0 | None | Unset, data)

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

        create_artifact_request = cls(
            owner=owner,
            ref=ref,
            scope=scope,
            type_=type_,
            content_type=content_type,
            data=data,
            description=description,
            name=name,
            retention_limit=retention_limit,
            retention_policy=retention_policy,
            visibility=visibility,
        )

        create_artifact_request.additional_properties = d
        return create_artifact_request

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
