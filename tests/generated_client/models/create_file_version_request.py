from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.create_file_version_request_meta_type_0 import (
        CreateFileVersionRequestMetaType0,
    )


T = TypeVar("T", bound="CreateFileVersionRequest")


@_attrs_define
class CreateFileVersionRequest:
    """Request DTO for creating a new file-backed artifact version.
    No file content is included — the caller writes the file directly to
    `$ATTUNE_ARTIFACTS_DIR/{file_path}` after receiving the response.

        Attributes:
            content_type (None | str | Unset): MIME content type (e.g. "text/plain", "application/octet-stream") Example:
                text/plain.
            created_by (None | str | Unset): Who created this version (e.g. action ref, identity, "system")
            execution (int | None | Unset): Execution that produced this version (optional) Example: 42.
            meta (CreateFileVersionRequestMetaType0 | None | Unset): Free-form metadata about this version
    """

    content_type: None | str | Unset = UNSET
    created_by: None | str | Unset = UNSET
    execution: int | None | Unset = UNSET
    meta: CreateFileVersionRequestMetaType0 | None | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.create_file_version_request_meta_type_0 import (
            CreateFileVersionRequestMetaType0,
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

        execution: int | None | Unset
        if isinstance(self.execution, Unset):
            execution = UNSET
        else:
            execution = self.execution

        meta: dict[str, Any] | None | Unset
        if isinstance(self.meta, Unset):
            meta = UNSET
        elif isinstance(self.meta, CreateFileVersionRequestMetaType0):
            meta = self.meta.to_dict()
        else:
            meta = self.meta

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update({})
        if content_type is not UNSET:
            field_dict["content_type"] = content_type
        if created_by is not UNSET:
            field_dict["created_by"] = created_by
        if execution is not UNSET:
            field_dict["execution"] = execution
        if meta is not UNSET:
            field_dict["meta"] = meta

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        from ..models.create_file_version_request_meta_type_0 import (
            CreateFileVersionRequestMetaType0,
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

        def _parse_execution(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        execution = _parse_execution(d.pop("execution", UNSET))

        def _parse_meta(
            data: object,
        ) -> CreateFileVersionRequestMetaType0 | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                meta_type_0 = CreateFileVersionRequestMetaType0.from_dict(data)

                return meta_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(CreateFileVersionRequestMetaType0 | None | Unset, data)

        meta = _parse_meta(d.pop("meta", UNSET))

        create_file_version_request = cls(
            content_type=content_type,
            created_by=created_by,
            execution=execution,
            meta=meta,
        )

        create_file_version_request.additional_properties = d
        return create_file_version_request

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
