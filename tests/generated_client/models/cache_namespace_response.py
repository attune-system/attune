from __future__ import annotations

import datetime
from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

from ..models.owner_type import OwnerType

T = TypeVar("T", bound="CacheNamespaceResponse")


@_attrs_define
class CacheNamespaceResponse:
    """Namespace metadata and freshness/health summary. Never includes entries.

    Attributes:
        active_generation (int | None):
        cache_not_populated (bool): True when there is no active generation (uninitialized dataset).
        created (datetime.datetime):
        definition_ref (None | str): Stable declarative component ref for a pack-managed namespace.
        freshness_target_seconds (int):
        id (int):
        last_refreshed_at (datetime.datetime | None): When the active generation was published.
        managed (bool): Whether this namespace is declaratively managed by a pack definition.
        managing_pack_ref (None | str): Durable ref of the pack that manages this namespace.
        max_generation_bytes (int):
        max_records_per_generation (int):
        max_retained_bytes (int):
        max_retained_generations (int):
        max_staging_generations (int):
        namespace (str):
        owner (str): Canonical owner key (`system` or a numeric owner id as text).
        owner_ref (None | str): Owner reference for display, when known.
        owner_type (OwnerType):
        record_count (int | None): Active generation record count, when populated.
        size_bytes (int | None): Active generation size in bytes, when populated.
        source_revision (None | str): Active generation source revision, when populated.
        stale (bool): True when the active generation's age exceeds the freshness target.
        tombstoned (bool): Whether the namespace is tombstoned and pending bounded cleanup.
        updated (datetime.datetime):
    """

    active_generation: int | None
    cache_not_populated: bool
    created: datetime.datetime
    definition_ref: None | str
    freshness_target_seconds: int
    id: int
    last_refreshed_at: datetime.datetime | None
    managed: bool
    managing_pack_ref: None | str
    max_generation_bytes: int
    max_records_per_generation: int
    max_retained_bytes: int
    max_retained_generations: int
    max_staging_generations: int
    namespace: str
    owner: str
    owner_ref: None | str
    owner_type: OwnerType
    record_count: int | None
    size_bytes: int | None
    source_revision: None | str
    stale: bool
    tombstoned: bool
    updated: datetime.datetime
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        active_generation: int | None
        active_generation = self.active_generation

        cache_not_populated = self.cache_not_populated

        created = self.created.isoformat()

        definition_ref: None | str
        definition_ref = self.definition_ref

        freshness_target_seconds = self.freshness_target_seconds

        id = self.id

        last_refreshed_at: None | str
        if isinstance(self.last_refreshed_at, datetime.datetime):
            last_refreshed_at = self.last_refreshed_at.isoformat()
        else:
            last_refreshed_at = self.last_refreshed_at

        managed = self.managed

        managing_pack_ref: None | str
        managing_pack_ref = self.managing_pack_ref

        max_generation_bytes = self.max_generation_bytes

        max_records_per_generation = self.max_records_per_generation

        max_retained_bytes = self.max_retained_bytes

        max_retained_generations = self.max_retained_generations

        max_staging_generations = self.max_staging_generations

        namespace = self.namespace

        owner = self.owner

        owner_ref: None | str
        owner_ref = self.owner_ref

        owner_type = self.owner_type.value

        record_count: int | None
        record_count = self.record_count

        size_bytes: int | None
        size_bytes = self.size_bytes

        source_revision: None | str
        source_revision = self.source_revision

        stale = self.stale

        tombstoned = self.tombstoned

        updated = self.updated.isoformat()

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "active_generation": active_generation,
                "cache_not_populated": cache_not_populated,
                "created": created,
                "definition_ref": definition_ref,
                "freshness_target_seconds": freshness_target_seconds,
                "id": id,
                "last_refreshed_at": last_refreshed_at,
                "managed": managed,
                "managing_pack_ref": managing_pack_ref,
                "max_generation_bytes": max_generation_bytes,
                "max_records_per_generation": max_records_per_generation,
                "max_retained_bytes": max_retained_bytes,
                "max_retained_generations": max_retained_generations,
                "max_staging_generations": max_staging_generations,
                "namespace": namespace,
                "owner": owner,
                "owner_ref": owner_ref,
                "owner_type": owner_type,
                "record_count": record_count,
                "size_bytes": size_bytes,
                "source_revision": source_revision,
                "stale": stale,
                "tombstoned": tombstoned,
                "updated": updated,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        d = dict(src_dict)

        def _parse_active_generation(data: object) -> int | None:
            if data is None:
                return data
            return cast(int | None, data)

        active_generation = _parse_active_generation(d.pop("active_generation"))

        cache_not_populated = d.pop("cache_not_populated")

        created = datetime.datetime.fromisoformat(d.pop("created"))

        def _parse_definition_ref(data: object) -> None | str:
            if data is None:
                return data
            return cast(None | str, data)

        definition_ref = _parse_definition_ref(d.pop("definition_ref"))

        freshness_target_seconds = d.pop("freshness_target_seconds")

        id = d.pop("id")

        def _parse_last_refreshed_at(data: object) -> datetime.datetime | None:
            if data is None:
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                last_refreshed_at_type_0 = datetime.datetime.fromisoformat(data)

                return last_refreshed_at_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(datetime.datetime | None, data)

        last_refreshed_at = _parse_last_refreshed_at(d.pop("last_refreshed_at"))

        managed = d.pop("managed")

        def _parse_managing_pack_ref(data: object) -> None | str:
            if data is None:
                return data
            return cast(None | str, data)

        managing_pack_ref = _parse_managing_pack_ref(d.pop("managing_pack_ref"))

        max_generation_bytes = d.pop("max_generation_bytes")

        max_records_per_generation = d.pop("max_records_per_generation")

        max_retained_bytes = d.pop("max_retained_bytes")

        max_retained_generations = d.pop("max_retained_generations")

        max_staging_generations = d.pop("max_staging_generations")

        namespace = d.pop("namespace")

        owner = d.pop("owner")

        def _parse_owner_ref(data: object) -> None | str:
            if data is None:
                return data
            return cast(None | str, data)

        owner_ref = _parse_owner_ref(d.pop("owner_ref"))

        owner_type = OwnerType(d.pop("owner_type"))

        def _parse_record_count(data: object) -> int | None:
            if data is None:
                return data
            return cast(int | None, data)

        record_count = _parse_record_count(d.pop("record_count"))

        def _parse_size_bytes(data: object) -> int | None:
            if data is None:
                return data
            return cast(int | None, data)

        size_bytes = _parse_size_bytes(d.pop("size_bytes"))

        def _parse_source_revision(data: object) -> None | str:
            if data is None:
                return data
            return cast(None | str, data)

        source_revision = _parse_source_revision(d.pop("source_revision"))

        stale = d.pop("stale")

        tombstoned = d.pop("tombstoned")

        updated = datetime.datetime.fromisoformat(d.pop("updated"))

        cache_namespace_response = cls(
            active_generation=active_generation,
            cache_not_populated=cache_not_populated,
            created=created,
            definition_ref=definition_ref,
            freshness_target_seconds=freshness_target_seconds,
            id=id,
            last_refreshed_at=last_refreshed_at,
            managed=managed,
            managing_pack_ref=managing_pack_ref,
            max_generation_bytes=max_generation_bytes,
            max_records_per_generation=max_records_per_generation,
            max_retained_bytes=max_retained_bytes,
            max_retained_generations=max_retained_generations,
            max_staging_generations=max_staging_generations,
            namespace=namespace,
            owner=owner,
            owner_ref=owner_ref,
            owner_type=owner_type,
            record_count=record_count,
            size_bytes=size_bytes,
            source_revision=source_revision,
            stale=stale,
            tombstoned=tombstoned,
            updated=updated,
        )

        cache_namespace_response.additional_properties = d
        return cache_namespace_response

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
