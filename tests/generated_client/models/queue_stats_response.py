from __future__ import annotations

import datetime
from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="QueueStatsResponse")


@_attrs_define
class QueueStatsResponse:
    """Response DTO for queue statistics

    Attributes:
        action_id (int): Action ID Example: 1.
        action_ref (str): Action reference Example: slack.post_message.
        active_count (int): Number of currently running executions Example: 2.
        last_updated (datetime.datetime): Timestamp of last statistics update Example: 2024-01-13T10:30:00Z.
        max_concurrent (int): Maximum concurrent executions allowed Example: 3.
        queue_length (int): Number of executions waiting in queue Example: 5.
        total_completed (int): Total executions completed since queue creation Example: 95.
        total_enqueued (int): Total executions enqueued since queue creation Example: 100.
        oldest_enqueued_at (datetime.datetime | None | Unset): Timestamp of oldest queued execution (if any) Example:
            2024-01-13T10:30:00Z.
    """

    action_id: int
    action_ref: str
    active_count: int
    last_updated: datetime.datetime
    max_concurrent: int
    queue_length: int
    total_completed: int
    total_enqueued: int
    oldest_enqueued_at: datetime.datetime | None | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        action_id = self.action_id

        action_ref = self.action_ref

        active_count = self.active_count

        last_updated = self.last_updated.isoformat()

        max_concurrent = self.max_concurrent

        queue_length = self.queue_length

        total_completed = self.total_completed

        total_enqueued = self.total_enqueued

        oldest_enqueued_at: None | str | Unset
        if isinstance(self.oldest_enqueued_at, Unset):
            oldest_enqueued_at = UNSET
        elif isinstance(self.oldest_enqueued_at, datetime.datetime):
            oldest_enqueued_at = self.oldest_enqueued_at.isoformat()
        else:
            oldest_enqueued_at = self.oldest_enqueued_at

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "action_id": action_id,
                "action_ref": action_ref,
                "active_count": active_count,
                "last_updated": last_updated,
                "max_concurrent": max_concurrent,
                "queue_length": queue_length,
                "total_completed": total_completed,
                "total_enqueued": total_enqueued,
            }
        )
        if oldest_enqueued_at is not UNSET:
            field_dict["oldest_enqueued_at"] = oldest_enqueued_at

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        action_id = d.pop("action_id")

        action_ref = d.pop("action_ref")

        active_count = d.pop("active_count")

        last_updated = datetime.datetime.fromisoformat(d.pop("last_updated"))

        max_concurrent = d.pop("max_concurrent")

        queue_length = d.pop("queue_length")

        total_completed = d.pop("total_completed")

        total_enqueued = d.pop("total_enqueued")

        def _parse_oldest_enqueued_at(data: object) -> datetime.datetime | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                oldest_enqueued_at_type_0 = datetime.datetime.fromisoformat(data)

                return oldest_enqueued_at_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(datetime.datetime | None | Unset, data)

        oldest_enqueued_at = _parse_oldest_enqueued_at(
            d.pop("oldest_enqueued_at", UNSET)
        )

        queue_stats_response = cls(
            action_id=action_id,
            action_ref=action_ref,
            active_count=active_count,
            last_updated=last_updated,
            max_concurrent=max_concurrent,
            queue_length=queue_length,
            total_completed=total_completed,
            total_enqueued=total_enqueued,
            oldest_enqueued_at=oldest_enqueued_at,
        )

        queue_stats_response.additional_properties = d
        return queue_stats_response

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
