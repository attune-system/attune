from __future__ import annotations

import datetime
from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

from ..types import UNSET, Unset

T = TypeVar("T", bound="CreateVersionFileResponse201Data")


@_attrs_define
class CreateVersionFileResponse201Data:
    """Response DTO for an artifact version (without binary content)

    Attributes:
        artifact (int): Parent artifact ID
        created (datetime.datetime): Creation timestamp
        id (int): Version ID
        version (int): Version number (1-based)
        content_json (Any | None | Unset):
        content_type (None | str | Unset): MIME content type
        created_by (None | str | Unset): Who created this version
        execution (int | None | Unset): Execution that produced this version (e.g., the execution that wrote
            this log version). Per-version association — the parent artifact may
            be linked to many executions across versions.
        file_path (None | str | Unset): Relative file path for disk-backed versions (from artifacts_dir root).
            When present, the file content lives on the shared volume, not in the DB.
        meta (Any | None | Unset):
        size_bytes (int | None | Unset): Size of content in bytes
    """

    artifact: int
    created: datetime.datetime
    id: int
    version: int
    content_json: Any | None | Unset = UNSET
    content_type: None | str | Unset = UNSET
    created_by: None | str | Unset = UNSET
    execution: int | None | Unset = UNSET
    file_path: None | str | Unset = UNSET
    meta: Any | None | Unset = UNSET
    size_bytes: int | None | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        artifact = self.artifact

        created = self.created.isoformat()

        id = self.id

        version = self.version

        content_json: Any | None | Unset
        if isinstance(self.content_json, Unset):
            content_json = UNSET
        else:
            content_json = self.content_json

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

        meta: Any | None | Unset
        if isinstance(self.meta, Unset):
            meta = UNSET
        else:
            meta = self.meta

        size_bytes: int | None | Unset
        if isinstance(self.size_bytes, Unset):
            size_bytes = UNSET
        else:
            size_bytes = self.size_bytes

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "artifact": artifact,
                "created": created,
                "id": id,
                "version": version,
            }
        )
        if content_json is not UNSET:
            field_dict["content_json"] = content_json
        if content_type is not UNSET:
            field_dict["content_type"] = content_type
        if created_by is not UNSET:
            field_dict["created_by"] = created_by
        if execution is not UNSET:
            field_dict["execution"] = execution
        if file_path is not UNSET:
            field_dict["file_path"] = file_path
        if meta is not UNSET:
            field_dict["meta"] = meta
        if size_bytes is not UNSET:
            field_dict["size_bytes"] = size_bytes

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        d = dict(src_dict)
        artifact = d.pop("artifact")

        created = datetime.datetime.fromisoformat(d.pop("created"))

        id = d.pop("id")

        version = d.pop("version")

        def _parse_content_json(data: object) -> Any | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Any | None | Unset, data)

        content_json = _parse_content_json(d.pop("content_json", UNSET))

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

        def _parse_meta(data: object) -> Any | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Any | None | Unset, data)

        meta = _parse_meta(d.pop("meta", UNSET))

        def _parse_size_bytes(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        size_bytes = _parse_size_bytes(d.pop("size_bytes", UNSET))

        create_version_file_response_201_data = cls(
            artifact=artifact,
            created=created,
            id=id,
            version=version,
            content_json=content_json,
            content_type=content_type,
            created_by=created_by,
            execution=execution,
            file_path=file_path,
            meta=meta,
            size_bytes=size_bytes,
        )

        create_version_file_response_201_data.additional_properties = d
        return create_version_file_response_201_data

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
