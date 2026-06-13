from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.create_pack_request_conf_schema import CreatePackRequestConfSchema
    from ..models.create_pack_request_config import CreatePackRequestConfig
    from ..models.create_pack_request_meta import CreatePackRequestMeta


T = TypeVar("T", bound="CreatePackRequest")


@_attrs_define
class CreatePackRequest:
    """Request DTO for creating a new pack

    Attributes:
        label (str): Human-readable label Example: Slack Integration.
        ref (str): Unique reference identifier (e.g., "core", "aws", "slack") Example: slack.
        version (str): Pack version (semver format recommended) Example: 1.0.0.
        conf_schema (CreatePackRequestConfSchema | Unset): Configuration schema (flat format with inline required/secret
            per parameter)
        config (CreatePackRequestConfig | Unset): Pack configuration values
        dependencies (list[str] | Unset): Pack dependencies (refs of required packs) Example: ['core'].
        description (None | str | Unset): Pack description Example: Integration with Slack for messaging and
            notifications.
        is_standard (bool | Unset): Whether this is a standard/built-in pack
        meta (CreatePackRequestMeta | Unset): Pack metadata
        runtime_deps (list[str] | Unset): Runtime dependencies (e.g., shell, python, nodejs) Example: ['shell',
            'python'].
        tags (list[str] | Unset): Tags for categorization Example: ['messaging', 'collaboration'].
    """

    label: str
    ref: str
    version: str
    conf_schema: CreatePackRequestConfSchema | Unset = UNSET
    config: CreatePackRequestConfig | Unset = UNSET
    dependencies: list[str] | Unset = UNSET
    description: None | str | Unset = UNSET
    is_standard: bool | Unset = UNSET
    meta: CreatePackRequestMeta | Unset = UNSET
    runtime_deps: list[str] | Unset = UNSET
    tags: list[str] | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        label = self.label

        ref = self.ref

        version = self.version

        conf_schema: dict[str, Any] | Unset = UNSET
        if not isinstance(self.conf_schema, Unset):
            conf_schema = self.conf_schema.to_dict()

        config: dict[str, Any] | Unset = UNSET
        if not isinstance(self.config, Unset):
            config = self.config.to_dict()

        dependencies: list[str] | Unset = UNSET
        if not isinstance(self.dependencies, Unset):
            dependencies = self.dependencies

        description: None | str | Unset
        if isinstance(self.description, Unset):
            description = UNSET
        else:
            description = self.description

        is_standard = self.is_standard

        meta: dict[str, Any] | Unset = UNSET
        if not isinstance(self.meta, Unset):
            meta = self.meta.to_dict()

        runtime_deps: list[str] | Unset = UNSET
        if not isinstance(self.runtime_deps, Unset):
            runtime_deps = self.runtime_deps

        tags: list[str] | Unset = UNSET
        if not isinstance(self.tags, Unset):
            tags = self.tags

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "label": label,
                "ref": ref,
                "version": version,
            }
        )
        if conf_schema is not UNSET:
            field_dict["conf_schema"] = conf_schema
        if config is not UNSET:
            field_dict["config"] = config
        if dependencies is not UNSET:
            field_dict["dependencies"] = dependencies
        if description is not UNSET:
            field_dict["description"] = description
        if is_standard is not UNSET:
            field_dict["is_standard"] = is_standard
        if meta is not UNSET:
            field_dict["meta"] = meta
        if runtime_deps is not UNSET:
            field_dict["runtime_deps"] = runtime_deps
        if tags is not UNSET:
            field_dict["tags"] = tags

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.create_pack_request_conf_schema import CreatePackRequestConfSchema
        from ..models.create_pack_request_config import CreatePackRequestConfig
        from ..models.create_pack_request_meta import CreatePackRequestMeta

        d = dict(src_dict)
        label = d.pop("label")

        ref = d.pop("ref")

        version = d.pop("version")

        _conf_schema = d.pop("conf_schema", UNSET)
        conf_schema: CreatePackRequestConfSchema | Unset
        if isinstance(_conf_schema, Unset):
            conf_schema = UNSET
        else:
            conf_schema = CreatePackRequestConfSchema.from_dict(_conf_schema)

        _config = d.pop("config", UNSET)
        config: CreatePackRequestConfig | Unset
        if isinstance(_config, Unset):
            config = UNSET
        else:
            config = CreatePackRequestConfig.from_dict(_config)

        dependencies = cast(list[str], d.pop("dependencies", UNSET))

        def _parse_description(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        description = _parse_description(d.pop("description", UNSET))

        is_standard = d.pop("is_standard", UNSET)

        _meta = d.pop("meta", UNSET)
        meta: CreatePackRequestMeta | Unset
        if isinstance(_meta, Unset):
            meta = UNSET
        else:
            meta = CreatePackRequestMeta.from_dict(_meta)

        runtime_deps = cast(list[str], d.pop("runtime_deps", UNSET))

        tags = cast(list[str], d.pop("tags", UNSET))

        create_pack_request = cls(
            label=label,
            ref=ref,
            version=version,
            conf_schema=conf_schema,
            config=config,
            dependencies=dependencies,
            description=description,
            is_standard=is_standard,
            meta=meta,
            runtime_deps=runtime_deps,
            tags=tags,
        )

        create_pack_request.additional_properties = d
        return create_pack_request

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
