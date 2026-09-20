from __future__ import annotations

import datetime
from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

from ..models.artifact_body_state import ArtifactBodyState
from ..types import UNSET, Unset

T = TypeVar("T", bound="ListVersionsResponse200DataItem")


@_attrs_define
class ListVersionsResponse200DataItem:
    """Simplified version for list endpoints

    Attributes:
        created (datetime.datetime): Creation timestamp
        id (int): Version ID
        version (int): Version number
        body_state (ArtifactBodyState | None | Unset):
        content_type (None | str | Unset): MIME content type
        created_by (None | str | Unset): Who created this version
        execution (int | None | Unset): Execution that produced this version
        file_path (None | str | Unset): Relative file path for disk-backed versions
        object_key (None | str | Unset):
        provider_version (None | str | Unset):
        sha256 (None | str | Unset):
        size_bytes (int | None | Unset): Size of content in bytes
    """

    created: datetime.datetime
    id: int
    version: int
    body_state: ArtifactBodyState | None | Unset = UNSET
    content_type: None | str | Unset = UNSET
    created_by: None | str | Unset = UNSET
    execution: int | None | Unset = UNSET
    file_path: None | str | Unset = UNSET
    object_key: None | str | Unset = UNSET
    provider_version: None | str | Unset = UNSET
    sha256: None | str | Unset = UNSET
    size_bytes: int | None | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        created = self.created.isoformat()

        id = self.id

        version = self.version

        body_state: None | str | Unset
        if isinstance(self.body_state, Unset):
            body_state = UNSET
        elif isinstance(self.body_state, ArtifactBodyState):
            body_state = self.body_state.value
        else:
            body_state = self.body_state

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

        execution: int | None | Unset
        if isinstance(self.execution, Unset):
            execution = UNSET
        else:
            execution = self.execution

        file_path: None | str | Unset
        if isinstance(self.file_path, Unset):
            file_path = UNSET
        else:
            file_path = self.file_path

        object_key: None | str | Unset
        if isinstance(self.object_key, Unset):
            object_key = UNSET
        else:
            object_key = self.object_key

        provider_version: None | str | Unset
        if isinstance(self.provider_version, Unset):
            provider_version = UNSET
        else:
            provider_version = self.provider_version

        sha256: None | str | Unset
        if isinstance(self.sha256, Unset):
            sha256 = UNSET
        else:
            sha256 = self.sha256

        size_bytes: int | None | Unset
        if isinstance(self.size_bytes, Unset):
            size_bytes = UNSET
        else:
            size_bytes = self.size_bytes

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "created": created,
                "id": id,
                "version": version,
            }
        )
        if body_state is not UNSET:
            field_dict["body_state"] = body_state
        if content_type is not UNSET:
            field_dict["content_type"] = content_type
        if created_by is not UNSET:
            field_dict["created_by"] = created_by
        if execution is not UNSET:
            field_dict["execution"] = execution
        if file_path is not UNSET:
            field_dict["file_path"] = file_path
        if object_key is not UNSET:
            field_dict["object_key"] = object_key
        if provider_version is not UNSET:
            field_dict["provider_version"] = provider_version
        if sha256 is not UNSET:
            field_dict["sha256"] = sha256
        if size_bytes is not UNSET:
            field_dict["size_bytes"] = size_bytes

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        d = dict(src_dict)
        created = datetime.datetime.fromisoformat(d.pop("created"))

        id = d.pop("id")

        version = d.pop("version")

        def _parse_body_state(data: object) -> ArtifactBodyState | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                body_state_type_1 = ArtifactBodyState(data)

                return body_state_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(ArtifactBodyState | None | Unset, data)

        body_state = _parse_body_state(d.pop("body_state", UNSET))

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

        def _parse_execution(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        execution = _parse_execution(d.pop("execution", UNSET))

        def _parse_file_path(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        file_path = _parse_file_path(d.pop("file_path", UNSET))

        def _parse_object_key(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        object_key = _parse_object_key(d.pop("object_key", UNSET))

        def _parse_provider_version(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        provider_version = _parse_provider_version(d.pop("provider_version", UNSET))

        def _parse_sha256(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        sha256 = _parse_sha256(d.pop("sha256", UNSET))

        def _parse_size_bytes(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        size_bytes = _parse_size_bytes(d.pop("size_bytes", UNSET))

        list_versions_response_200_data_item = cls(
            created=created,
            id=id,
            version=version,
            body_state=body_state,
            content_type=content_type,
            created_by=created_by,
            execution=execution,
            file_path=file_path,
            object_key=object_key,
            provider_version=provider_version,
            sha256=sha256,
            size_bytes=size_bytes,
        )

        list_versions_response_200_data_item.additional_properties = d
        return list_versions_response_200_data_item

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
