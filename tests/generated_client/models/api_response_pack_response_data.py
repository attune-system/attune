from __future__ import annotations

import datetime
from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.api_response_pack_response_data_conf_schema import (
        ApiResponsePackResponseDataConfSchema,
    )
    from ..models.api_response_pack_response_data_config import (
        ApiResponsePackResponseDataConfig,
    )
    from ..models.api_response_pack_response_data_meta import (
        ApiResponsePackResponseDataMeta,
    )


T = TypeVar("T", bound="ApiResponsePackResponseData")


@_attrs_define
class ApiResponsePackResponseData:
    """Response DTO for pack information

    Attributes:
        conf_schema (ApiResponsePackResponseDataConfSchema): Configuration schema
        config (ApiResponsePackResponseDataConfig): Pack configuration
        created (datetime.datetime): Creation timestamp Example: 2024-01-13T10:30:00Z.
        dependencies (list[str]): Pack dependencies (refs of required packs) Example: ['core'].
        id (int): Pack ID Example: 1.
        is_standard (bool): Is standard pack
        label (str): Human-readable label Example: Slack Integration.
        meta (ApiResponsePackResponseDataMeta): Pack metadata
        ref (str): Unique reference identifier Example: slack.
        runtime_deps (list[str]): Runtime dependencies (e.g., shell, python, nodejs) Example: ['shell', 'python'].
        tags (list[str]): Tags Example: ['messaging', 'collaboration'].
        updated (datetime.datetime): Last update timestamp Example: 2024-01-13T10:30:00Z.
        version (str): Pack version Example: 1.0.0.
        description (None | str | Unset): Pack description Example: Integration with Slack for messaging and
            notifications.
    """

    conf_schema: ApiResponsePackResponseDataConfSchema
    config: ApiResponsePackResponseDataConfig
    created: datetime.datetime
    dependencies: list[str]
    id: int
    is_standard: bool
    label: str
    meta: ApiResponsePackResponseDataMeta
    ref: str
    runtime_deps: list[str]
    tags: list[str]
    updated: datetime.datetime
    version: str
    description: None | str | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        conf_schema = self.conf_schema.to_dict()

        config = self.config.to_dict()

        created = self.created.isoformat()

        dependencies = self.dependencies

        id = self.id

        is_standard = self.is_standard

        label = self.label

        meta = self.meta.to_dict()

        ref = self.ref

        runtime_deps = self.runtime_deps

        tags = self.tags

        updated = self.updated.isoformat()

        version = self.version

        description: None | str | Unset
        if isinstance(self.description, Unset):
            description = UNSET
        else:
            description = self.description

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "conf_schema": conf_schema,
                "config": config,
                "created": created,
                "dependencies": dependencies,
                "id": id,
                "is_standard": is_standard,
                "label": label,
                "meta": meta,
                "ref": ref,
                "runtime_deps": runtime_deps,
                "tags": tags,
                "updated": updated,
                "version": version,
            }
        )
        if description is not UNSET:
            field_dict["description"] = description

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.api_response_pack_response_data_conf_schema import (
            ApiResponsePackResponseDataConfSchema,
        )
        from ..models.api_response_pack_response_data_config import (
            ApiResponsePackResponseDataConfig,
        )
        from ..models.api_response_pack_response_data_meta import (
            ApiResponsePackResponseDataMeta,
        )

        d = dict(src_dict)
        conf_schema = ApiResponsePackResponseDataConfSchema.from_dict(
            d.pop("conf_schema")
        )

        config = ApiResponsePackResponseDataConfig.from_dict(d.pop("config"))

        created = datetime.datetime.fromisoformat(d.pop("created"))

        dependencies = cast(list[str], d.pop("dependencies"))

        id = d.pop("id")

        is_standard = d.pop("is_standard")

        label = d.pop("label")

        meta = ApiResponsePackResponseDataMeta.from_dict(d.pop("meta"))

        ref = d.pop("ref")

        runtime_deps = cast(list[str], d.pop("runtime_deps"))

        tags = cast(list[str], d.pop("tags"))

        updated = datetime.datetime.fromisoformat(d.pop("updated"))

        version = d.pop("version")

        def _parse_description(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        description = _parse_description(d.pop("description", UNSET))

        api_response_pack_response_data = cls(
            conf_schema=conf_schema,
            config=config,
            created=created,
            dependencies=dependencies,
            id=id,
            is_standard=is_standard,
            label=label,
            meta=meta,
            ref=ref,
            runtime_deps=runtime_deps,
            tags=tags,
            updated=updated,
            version=version,
            description=description,
        )

        api_response_pack_response_data.additional_properties = d
        return api_response_pack_response_data

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
