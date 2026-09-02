from __future__ import annotations

import datetime
from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

from ..types import UNSET, Unset

T = TypeVar("T", bound="ListVersionsResponse200DataItem")


@_attrs_define
class ListVersionsResponse200DataItem:
    """Simplified version for list endpoints

    Attributes:
        created (datetime.datetime): Creation timestamp
        id (int): Version ID
        version (int): Version number
        content_type (None | str | Unset): MIME content type
        created_by (None | str | Unset): Who created this version
        execution (int | None | Unset): Execution that produced this version
        file_path (None | str | Unset): Relative file path for disk-backed versions
        size_bytes (int | None | Unset): Size of content in bytes
    """

    created: datetime.datetime
    id: int
    version: int
    content_type: None | str | Unset = UNSET
    created_by: None | str | Unset = UNSET
    execution: int | None | Unset = UNSET
    file_path: None | str | Unset = UNSET
    size_bytes: int | None | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        created = self.created.isoformat()

        id = self.id

        version = self.version

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
        if content_type is not UNSET:
            field_dict["content_type"] = content_type
        if created_by is not UNSET:
            field_dict["created_by"] = created_by
        if execution is not UNSET:
            field_dict["execution"] = execution
        if file_path is not UNSET:
            field_dict["file_path"] = file_path
        if size_bytes is not UNSET:
            field_dict["size_bytes"] = size_bytes

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        d = dict(src_dict)
        created = datetime.datetime.fromisoformat(d.pop("created"))

        id = d.pop("id")

        version = d.pop("version")

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
            content_type=content_type,
            created_by=created_by,
            execution=execution,
            file_path=file_path,
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
