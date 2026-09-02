from __future__ import annotations

from collections.abc import Mapping
from io import BytesIO
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

from .. import types
from ..types import UNSET, File, Unset

T = TypeVar("T", bound="ArtifactVersionByRefUploadForm")


@_attrs_define
class ArtifactVersionByRefUploadForm:
    """
    Attributes:
        file (File):
        content_type (None | str | Unset):
        created_by (None | str | Unset):
        description (None | str | Unset):
        execution (None | str | Unset):
        meta (None | str | Unset):
        name (None | str | Unset):
        owner (None | str | Unset):
        retention_limit (None | str | Unset):
        retention_policy (None | str | Unset):
        scope (None | str | Unset):
        type_ (None | str | Unset):
        visibility (None | str | Unset):
    """

    file: File
    content_type: None | str | Unset = UNSET
    created_by: None | str | Unset = UNSET
    description: None | str | Unset = UNSET
    execution: None | str | Unset = UNSET
    meta: None | str | Unset = UNSET
    name: None | str | Unset = UNSET
    owner: None | str | Unset = UNSET
    retention_limit: None | str | Unset = UNSET
    retention_policy: None | str | Unset = UNSET
    scope: None | str | Unset = UNSET
    type_: None | str | Unset = UNSET
    visibility: None | str | Unset = UNSET
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

        description: None | str | Unset
        if isinstance(self.description, Unset):
            description = UNSET
        else:
            description = self.description

        execution: None | str | Unset
        if isinstance(self.execution, Unset):
            execution = UNSET
        else:
            execution = self.execution

        meta: None | str | Unset
        if isinstance(self.meta, Unset):
            meta = UNSET
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

        retention_limit: None | str | Unset
        if isinstance(self.retention_limit, Unset):
            retention_limit = UNSET
        else:
            retention_limit = self.retention_limit

        retention_policy: None | str | Unset
        if isinstance(self.retention_policy, Unset):
            retention_policy = UNSET
        else:
            retention_policy = self.retention_policy

        scope: None | str | Unset
        if isinstance(self.scope, Unset):
            scope = UNSET
        else:
            scope = self.scope

        type_: None | str | Unset
        if isinstance(self.type_, Unset):
            type_ = UNSET
        else:
            type_ = self.type_

        visibility: None | str | Unset
        if isinstance(self.visibility, Unset):
            visibility = UNSET
        else:
            visibility = self.visibility

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

        if not isinstance(self.description, Unset):
            if isinstance(self.description, str):
                files.append(
                    (
                        "description",
                        (None, str(self.description).encode(), "text/plain"),
                    )
                )
            else:
                files.append(
                    (
                        "description",
                        (None, str(self.description).encode(), "text/plain"),
                    )
                )

        if not isinstance(self.execution, Unset):
            if isinstance(self.execution, str):
                files.append(
                    ("execution", (None, str(self.execution).encode(), "text/plain"))
                )
            else:
                files.append(
                    ("execution", (None, str(self.execution).encode(), "text/plain"))
                )

        if not isinstance(self.meta, Unset):
            if isinstance(self.meta, str):
                files.append(("meta", (None, str(self.meta).encode(), "text/plain")))
            else:
                files.append(("meta", (None, str(self.meta).encode(), "text/plain")))

        if not isinstance(self.name, Unset):
            if isinstance(self.name, str):
                files.append(("name", (None, str(self.name).encode(), "text/plain")))
            else:
                files.append(("name", (None, str(self.name).encode(), "text/plain")))

        if not isinstance(self.owner, Unset):
            if isinstance(self.owner, str):
                files.append(("owner", (None, str(self.owner).encode(), "text/plain")))
            else:
                files.append(("owner", (None, str(self.owner).encode(), "text/plain")))

        if not isinstance(self.retention_limit, Unset):
            if isinstance(self.retention_limit, str):
                files.append(
                    (
                        "retention_limit",
                        (None, str(self.retention_limit).encode(), "text/plain"),
                    )
                )
            else:
                files.append(
                    (
                        "retention_limit",
                        (None, str(self.retention_limit).encode(), "text/plain"),
                    )
                )

        if not isinstance(self.retention_policy, Unset):
            if isinstance(self.retention_policy, str):
                files.append(
                    (
                        "retention_policy",
                        (None, str(self.retention_policy).encode(), "text/plain"),
                    )
                )
            else:
                files.append(
                    (
                        "retention_policy",
                        (None, str(self.retention_policy).encode(), "text/plain"),
                    )
                )

        if not isinstance(self.scope, Unset):
            if isinstance(self.scope, str):
                files.append(("scope", (None, str(self.scope).encode(), "text/plain")))
            else:
                files.append(("scope", (None, str(self.scope).encode(), "text/plain")))

        if not isinstance(self.type_, Unset):
            if isinstance(self.type_, str):
                files.append(("type", (None, str(self.type_).encode(), "text/plain")))
            else:
                files.append(("type", (None, str(self.type_).encode(), "text/plain")))

        if not isinstance(self.visibility, Unset):
            if isinstance(self.visibility, str):
                files.append(
                    ("visibility", (None, str(self.visibility).encode(), "text/plain"))
                )
            else:
                files.append(
                    ("visibility", (None, str(self.visibility).encode(), "text/plain"))
                )

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

        def _parse_description(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        description = _parse_description(d.pop("description", UNSET))

        def _parse_execution(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        execution = _parse_execution(d.pop("execution", UNSET))

        def _parse_meta(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

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

        def _parse_retention_limit(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        retention_limit = _parse_retention_limit(d.pop("retention_limit", UNSET))

        def _parse_retention_policy(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        retention_policy = _parse_retention_policy(d.pop("retention_policy", UNSET))

        def _parse_scope(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        scope = _parse_scope(d.pop("scope", UNSET))

        def _parse_type_(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        type_ = _parse_type_(d.pop("type", UNSET))

        def _parse_visibility(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        visibility = _parse_visibility(d.pop("visibility", UNSET))

        artifact_version_by_ref_upload_form = cls(
            file=file,
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

        artifact_version_by_ref_upload_form.additional_properties = d
        return artifact_version_by_ref_upload_form

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
