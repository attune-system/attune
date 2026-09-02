from __future__ import annotations

import datetime
from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.pack_response_conf_schema import PackResponseConfSchema
    from ..models.pack_response_config import PackResponseConfig
    from ..models.pack_response_meta import PackResponseMeta


T = TypeVar("T", bound="PackResponse")


@_attrs_define
class PackResponse:
    """Response DTO for pack information

    Attributes:
        conf_schema (PackResponseConfSchema): Configuration schema
        config (PackResponseConfig): Pack configuration
        created (datetime.datetime): Creation timestamp Example: 2024-01-13T10:30:00Z.
        dependencies (list[str]): Pack dependencies (refs of required packs) Example: ['core'].
        id (int): Pack ID Example: 1.
        is_standard (bool): Is standard pack Example: False.
        label (str): Human-readable label Example: Slack Integration.
        meta (PackResponseMeta): Pack metadata
        ref (str): Unique reference identifier Example: slack.
        runtime_deps (list[str]): Runtime dependencies (e.g., shell, python, nodejs) Example: ['shell', 'python'].
        tags (list[str]): Tags Example: ['messaging', 'collaboration'].
        updated (datetime.datetime): Last update timestamp Example: 2024-01-13T10:30:00Z.
        version (str): Pack version Example: 1.0.0.
        worker_affinity (Any):
        worker_selector (Any):
        worker_tolerations (Any):
        action_count (int | None | Unset): Number of actions registered for this pack Example: 12.
        description (None | str | Unset): Pack description Example: Integration with Slack for messaging and
            notifications.
        rule_count (int | None | Unset): Number of rules registered for this pack Example: 5.
        sensor_count (int | None | Unset): Number of sensors registered for this pack Example: 2.
        trigger_count (int | None | Unset): Number of triggers registered for this pack Example: 3.
    """

    conf_schema: PackResponseConfSchema
    config: PackResponseConfig
    created: datetime.datetime
    dependencies: list[str]
    id: int
    is_standard: bool
    label: str
    meta: PackResponseMeta
    ref: str
    runtime_deps: list[str]
    tags: list[str]
    updated: datetime.datetime
    version: str
    worker_affinity: Any
    worker_selector: Any
    worker_tolerations: Any
    action_count: int | None | Unset = UNSET
    description: None | str | Unset = UNSET
    rule_count: int | None | Unset = UNSET
    sensor_count: int | None | Unset = UNSET
    trigger_count: int | None | Unset = UNSET
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

        worker_affinity = self.worker_affinity

        worker_selector = self.worker_selector

        worker_tolerations = self.worker_tolerations

        action_count: int | None | Unset
        if isinstance(self.action_count, Unset):
            action_count = UNSET
        else:
            action_count = self.action_count

        description: None | str | Unset
        if isinstance(self.description, Unset):
            description = UNSET
        else:
            description = self.description

        rule_count: int | None | Unset
        if isinstance(self.rule_count, Unset):
            rule_count = UNSET
        else:
            rule_count = self.rule_count

        sensor_count: int | None | Unset
        if isinstance(self.sensor_count, Unset):
            sensor_count = UNSET
        else:
            sensor_count = self.sensor_count

        trigger_count: int | None | Unset
        if isinstance(self.trigger_count, Unset):
            trigger_count = UNSET
        else:
            trigger_count = self.trigger_count

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
                "worker_affinity": worker_affinity,
                "worker_selector": worker_selector,
                "worker_tolerations": worker_tolerations,
            }
        )
        if action_count is not UNSET:
            field_dict["action_count"] = action_count
        if description is not UNSET:
            field_dict["description"] = description
        if rule_count is not UNSET:
            field_dict["rule_count"] = rule_count
        if sensor_count is not UNSET:
            field_dict["sensor_count"] = sensor_count
        if trigger_count is not UNSET:
            field_dict["trigger_count"] = trigger_count

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        from ..models.pack_response_conf_schema import (
            PackResponseConfSchema,
        )
        from ..models.pack_response_config import PackResponseConfig
        from ..models.pack_response_meta import PackResponseMeta

        d = dict(src_dict)
        conf_schema = PackResponseConfSchema.from_dict(d.pop("conf_schema"))

        config = PackResponseConfig.from_dict(d.pop("config"))

        created = datetime.datetime.fromisoformat(d.pop("created"))

        dependencies = cast(list[str], d.pop("dependencies"))

        id = d.pop("id")

        is_standard = d.pop("is_standard")

        label = d.pop("label")

        meta = PackResponseMeta.from_dict(d.pop("meta"))

        ref = d.pop("ref")

        runtime_deps = cast(list[str], d.pop("runtime_deps"))

        tags = cast(list[str], d.pop("tags"))

        updated = datetime.datetime.fromisoformat(d.pop("updated"))

        version = d.pop("version")

        worker_affinity = d.pop("worker_affinity")

        worker_selector = d.pop("worker_selector")

        worker_tolerations = d.pop("worker_tolerations")

        def _parse_action_count(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        action_count = _parse_action_count(d.pop("action_count", UNSET))

        def _parse_description(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        description = _parse_description(d.pop("description", UNSET))

        def _parse_rule_count(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        rule_count = _parse_rule_count(d.pop("rule_count", UNSET))

        def _parse_sensor_count(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        sensor_count = _parse_sensor_count(d.pop("sensor_count", UNSET))

        def _parse_trigger_count(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        trigger_count = _parse_trigger_count(d.pop("trigger_count", UNSET))

        pack_response = cls(
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
            worker_affinity=worker_affinity,
            worker_selector=worker_selector,
            worker_tolerations=worker_tolerations,
            action_count=action_count,
            description=description,
            rule_count=rule_count,
            sensor_count=sensor_count,
            trigger_count=trigger_count,
        )

        pack_response.additional_properties = d
        return pack_response

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
