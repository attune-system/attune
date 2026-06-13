from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.pack_description_patch_type_0 import PackDescriptionPatchType0
    from ..models.pack_description_patch_type_1 import PackDescriptionPatchType1
    from ..models.update_pack_request_conf_schema_type_0 import (
        UpdatePackRequestConfSchemaType0,
    )
    from ..models.update_pack_request_config_type_0 import UpdatePackRequestConfigType0
    from ..models.update_pack_request_meta_type_0 import UpdatePackRequestMetaType0


T = TypeVar("T", bound="UpdatePackRequest")


@_attrs_define
class UpdatePackRequest:
    """Request DTO for updating a pack

    Attributes:
        conf_schema (None | UpdatePackRequestConfSchemaType0): Configuration schema
        config (None | UpdatePackRequestConfigType0): Pack configuration values
        meta (None | UpdatePackRequestMetaType0): Pack metadata
        dependencies (list[str] | None | Unset): Pack dependencies (refs of required packs) Example: ['core', 'http'].
        description (None | PackDescriptionPatchType0 | PackDescriptionPatchType1 | Unset):
        is_standard (bool | None | Unset): Whether this is a standard pack
        label (None | str | Unset): Human-readable label Example: Slack Integration v2.
        runtime_deps (list[str] | None | Unset): Runtime dependencies (e.g., shell, python, nodejs) Example: ['shell',
            'python'].
        tags (list[str] | None | Unset): Tags for categorization Example: ['messaging', 'collaboration', 'webhooks'].
        version (None | str | Unset): Pack version Example: 2.0.0.
    """

    conf_schema: None | UpdatePackRequestConfSchemaType0
    config: None | UpdatePackRequestConfigType0
    meta: None | UpdatePackRequestMetaType0
    dependencies: list[str] | None | Unset = UNSET
    description: (
        None | PackDescriptionPatchType0 | PackDescriptionPatchType1 | Unset
    ) = UNSET
    is_standard: bool | None | Unset = UNSET
    label: None | str | Unset = UNSET
    runtime_deps: list[str] | None | Unset = UNSET
    tags: list[str] | None | Unset = UNSET
    version: None | str | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.pack_description_patch_type_0 import PackDescriptionPatchType0
        from ..models.pack_description_patch_type_1 import PackDescriptionPatchType1
        from ..models.update_pack_request_conf_schema_type_0 import (
            UpdatePackRequestConfSchemaType0,
        )
        from ..models.update_pack_request_config_type_0 import (
            UpdatePackRequestConfigType0,
        )
        from ..models.update_pack_request_meta_type_0 import UpdatePackRequestMetaType0

        conf_schema: dict[str, Any] | None
        if isinstance(self.conf_schema, UpdatePackRequestConfSchemaType0):
            conf_schema = self.conf_schema.to_dict()
        else:
            conf_schema = self.conf_schema

        config: dict[str, Any] | None
        if isinstance(self.config, UpdatePackRequestConfigType0):
            config = self.config.to_dict()
        else:
            config = self.config

        meta: dict[str, Any] | None
        if isinstance(self.meta, UpdatePackRequestMetaType0):
            meta = self.meta.to_dict()
        else:
            meta = self.meta

        dependencies: list[str] | None | Unset
        if isinstance(self.dependencies, Unset):
            dependencies = UNSET
        elif isinstance(self.dependencies, list):
            dependencies = self.dependencies

        else:
            dependencies = self.dependencies

        description: dict[str, Any] | None | Unset
        if isinstance(self.description, Unset):
            description = UNSET
        elif isinstance(self.description, PackDescriptionPatchType0):
            description = self.description.to_dict()
        elif isinstance(self.description, PackDescriptionPatchType1):
            description = self.description.to_dict()
        else:
            description = self.description

        is_standard: bool | None | Unset
        if isinstance(self.is_standard, Unset):
            is_standard = UNSET
        else:
            is_standard = self.is_standard

        label: None | str | Unset
        if isinstance(self.label, Unset):
            label = UNSET
        else:
            label = self.label

        runtime_deps: list[str] | None | Unset
        if isinstance(self.runtime_deps, Unset):
            runtime_deps = UNSET
        elif isinstance(self.runtime_deps, list):
            runtime_deps = self.runtime_deps

        else:
            runtime_deps = self.runtime_deps

        tags: list[str] | None | Unset
        if isinstance(self.tags, Unset):
            tags = UNSET
        elif isinstance(self.tags, list):
            tags = self.tags

        else:
            tags = self.tags

        version: None | str | Unset
        if isinstance(self.version, Unset):
            version = UNSET
        else:
            version = self.version

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "conf_schema": conf_schema,
                "config": config,
                "meta": meta,
            }
        )
        if dependencies is not UNSET:
            field_dict["dependencies"] = dependencies
        if description is not UNSET:
            field_dict["description"] = description
        if is_standard is not UNSET:
            field_dict["is_standard"] = is_standard
        if label is not UNSET:
            field_dict["label"] = label
        if runtime_deps is not UNSET:
            field_dict["runtime_deps"] = runtime_deps
        if tags is not UNSET:
            field_dict["tags"] = tags
        if version is not UNSET:
            field_dict["version"] = version

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.pack_description_patch_type_0 import PackDescriptionPatchType0
        from ..models.pack_description_patch_type_1 import PackDescriptionPatchType1
        from ..models.update_pack_request_conf_schema_type_0 import (
            UpdatePackRequestConfSchemaType0,
        )
        from ..models.update_pack_request_config_type_0 import (
            UpdatePackRequestConfigType0,
        )
        from ..models.update_pack_request_meta_type_0 import UpdatePackRequestMetaType0

        d = dict(src_dict)

        def _parse_conf_schema(data: object) -> None | UpdatePackRequestConfSchemaType0:
            if data is None:
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                conf_schema_type_0 = UpdatePackRequestConfSchemaType0.from_dict(data)

                return conf_schema_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(None | UpdatePackRequestConfSchemaType0, data)

        conf_schema = _parse_conf_schema(d.pop("conf_schema"))

        def _parse_config(data: object) -> None | UpdatePackRequestConfigType0:
            if data is None:
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                config_type_0 = UpdatePackRequestConfigType0.from_dict(data)

                return config_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(None | UpdatePackRequestConfigType0, data)

        config = _parse_config(d.pop("config"))

        def _parse_meta(data: object) -> None | UpdatePackRequestMetaType0:
            if data is None:
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                meta_type_0 = UpdatePackRequestMetaType0.from_dict(data)

                return meta_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(None | UpdatePackRequestMetaType0, data)

        meta = _parse_meta(d.pop("meta"))

        def _parse_dependencies(data: object) -> list[str] | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, list):
                    raise TypeError()
                dependencies_type_0 = cast(list[str], data)

                return dependencies_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(list[str] | None | Unset, data)

        dependencies = _parse_dependencies(d.pop("dependencies", UNSET))

        def _parse_description(
            data: object,
        ) -> None | PackDescriptionPatchType0 | PackDescriptionPatchType1 | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_pack_description_patch_type_0 = (
                    PackDescriptionPatchType0.from_dict(data)
                )

                return componentsschemas_pack_description_patch_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_pack_description_patch_type_1 = (
                    PackDescriptionPatchType1.from_dict(data)
                )

                return componentsschemas_pack_description_patch_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(
                None | PackDescriptionPatchType0 | PackDescriptionPatchType1 | Unset,
                data,
            )

        description = _parse_description(d.pop("description", UNSET))

        def _parse_is_standard(data: object) -> bool | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(bool | None | Unset, data)

        is_standard = _parse_is_standard(d.pop("is_standard", UNSET))

        def _parse_label(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        label = _parse_label(d.pop("label", UNSET))

        def _parse_runtime_deps(data: object) -> list[str] | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, list):
                    raise TypeError()
                runtime_deps_type_0 = cast(list[str], data)

                return runtime_deps_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(list[str] | None | Unset, data)

        runtime_deps = _parse_runtime_deps(d.pop("runtime_deps", UNSET))

        def _parse_tags(data: object) -> list[str] | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, list):
                    raise TypeError()
                tags_type_0 = cast(list[str], data)

                return tags_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(list[str] | None | Unset, data)

        tags = _parse_tags(d.pop("tags", UNSET))

        def _parse_version(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        version = _parse_version(d.pop("version", UNSET))

        update_pack_request = cls(
            conf_schema=conf_schema,
            config=config,
            meta=meta,
            dependencies=dependencies,
            description=description,
            is_standard=is_standard,
            label=label,
            runtime_deps=runtime_deps,
            tags=tags,
            version=version,
        )

        update_pack_request.additional_properties = d
        return update_pack_request

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
