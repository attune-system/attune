from __future__ import annotations

from collections.abc import Mapping
from io import BytesIO
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

from .. import types
from ..types import UNSET, File, Unset

T = TypeVar("T", bound="ArtifactVersionUploadForm")


@_attrs_define
class ArtifactVersionUploadForm:
    """
    Attributes:
        file (File):
        content_type (None | str | Unset):
        created_by (None | str | Unset):
        meta (None | str | Unset):
    """

    file: File
    content_type: None | str | Unset = UNSET
    created_by: None | str | Unset = UNSET
    meta: None | str | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        file = self.file.to_tuple()

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

        meta: None | str | Unset
        if isinstance(self.meta, Unset):
            meta = UNSET
        else:
            meta = self.meta

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "file": file,
            }
        )
        if content_type is not UNSET:
            field_dict["content_type"] = content_type
        if created_by is not UNSET:
            field_dict["created_by"] = created_by
        if meta is not UNSET:
            field_dict["meta"] = meta

        return field_dict

    def to_multipart(self) -> types.RequestFiles:
        files: types.RequestFiles = []

        files.append(("file", self.file.to_tuple()))

        if not isinstance(self.content_type, Unset):
            if isinstance(self.content_type, str):
                files.append(
                    (
                        "content_type",
                        (None, str(self.content_type).encode(), "text/plain"),
                    )
                )
            else:
                files.append(
                    (
                        "content_type",
                        (None, str(self.content_type).encode(), "text/plain"),
                    )
                )

        if not isinstance(self.created_by, Unset):
            if isinstance(self.created_by, str):
                files.append(
                    ("created_by", (None, str(self.created_by).encode(), "text/plain"))
                )
            else:
                files.append(
                    ("created_by", (None, str(self.created_by).encode(), "text/plain"))
                )

        if not isinstance(self.meta, Unset):
            if isinstance(self.meta, str):
                files.append(("meta", (None, str(self.meta).encode(), "text/plain")))
            else:
                files.append(("meta", (None, str(self.meta).encode(), "text/plain")))

        for prop_name, prop in self.additional_properties.items():
            files.append((prop_name, (None, str(prop).encode(), "text/plain")))

        return files

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        d = dict(src_dict)
        file = File(payload=BytesIO(d.pop("file")))

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

        def _parse_meta(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        meta = _parse_meta(d.pop("meta", UNSET))

        artifact_version_upload_form = cls(
            file=file,
            content_type=content_type,
            created_by=created_by,
            meta=meta,
        )

        artifact_version_upload_form.additional_properties = d
        return artifact_version_upload_form

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
