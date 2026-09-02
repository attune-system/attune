from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

T = TypeVar("T", bound="CacheNamespaceDeletionResponse")


@_attrs_define
class CacheNamespaceDeletionResponse:
    """Tombstone/queued-cleanup status returned by namespace deletion.

    Attributes:
        cleanup_pending (bool): Cleanup is asynchronous; entries are reclaimed in bounded batches.
        id (int):
        namespace (str):
        status (str):
        tombstoned (bool):
    """

    cleanup_pending: bool
    id: int
    namespace: str
    status: str
    tombstoned: bool
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        cleanup_pending = self.cleanup_pending

        id = self.id

        namespace = self.namespace

        status = self.status

        tombstoned = self.tombstoned

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "cleanup_pending": cleanup_pending,
                "id": id,
                "namespace": namespace,
                "status": status,
                "tombstoned": tombstoned,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        d = dict(src_dict)
        cleanup_pending = d.pop("cleanup_pending")

        id = d.pop("id")

        namespace = d.pop("namespace")

        status = d.pop("status")

        tombstoned = d.pop("tombstoned")

        cache_namespace_deletion_response = cls(
            cleanup_pending=cleanup_pending,
            id=id,
            namespace=namespace,
            status=status,
            tombstoned=tombstoned,
        )

        cache_namespace_deletion_response.additional_properties = d
        return cache_namespace_deletion_response

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
