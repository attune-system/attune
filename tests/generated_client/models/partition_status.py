from __future__ import annotations

import datetime
from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

from ..models.managed_table import ManagedTable
from ..types import UNSET, Unset

T = TypeVar("T", bound="PartitionStatus")


@_attrs_define
class PartitionStatus:
    """Catalog-verified status. DEFAULT counts stop at repair_cap + 1; they are
    lower bounds when default_count_exact is false, never full backlog scans.

        Attributes:
            default_count_exact (bool):
            default_rows_at_least (int):
            future_partitions (int):
            missing_future_partitions (int):
            parent (ManagedTable): Daily UTC RANGE parents managed by the supervisor.
            registered_partitions (int):
            oldest_default_day (datetime.datetime | None | Unset):
    """

    default_count_exact: bool
    default_rows_at_least: int
    future_partitions: int
    missing_future_partitions: int
    parent: ManagedTable
    registered_partitions: int
    oldest_default_day: datetime.datetime | None | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        default_count_exact = self.default_count_exact

        default_rows_at_least = self.default_rows_at_least

        future_partitions = self.future_partitions

        missing_future_partitions = self.missing_future_partitions

        parent = self.parent.value

        registered_partitions = self.registered_partitions

        oldest_default_day: None | str | Unset
        if isinstance(self.oldest_default_day, Unset):
            oldest_default_day = UNSET
        elif isinstance(self.oldest_default_day, datetime.datetime):
            oldest_default_day = self.oldest_default_day.isoformat()
        else:
            oldest_default_day = self.oldest_default_day

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "default_count_exact": default_count_exact,
                "default_rows_at_least": default_rows_at_least,
                "future_partitions": future_partitions,
                "missing_future_partitions": missing_future_partitions,
                "parent": parent,
                "registered_partitions": registered_partitions,
            }
        )
        if oldest_default_day is not UNSET:
            field_dict["oldest_default_day"] = oldest_default_day

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        d = dict(src_dict)
        default_count_exact = d.pop("default_count_exact")

        default_rows_at_least = d.pop("default_rows_at_least")

        future_partitions = d.pop("future_partitions")

        missing_future_partitions = d.pop("missing_future_partitions")

        parent = ManagedTable(d.pop("parent"))

        registered_partitions = d.pop("registered_partitions")

        def _parse_oldest_default_day(data: object) -> datetime.datetime | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                oldest_default_day_type_0 = datetime.datetime.fromisoformat(data)

                return oldest_default_day_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(datetime.datetime | None | Unset, data)

        oldest_default_day = _parse_oldest_default_day(
            d.pop("oldest_default_day", UNSET)
        )

        partition_status = cls(
            default_count_exact=default_count_exact,
            default_rows_at_least=default_rows_at_least,
            future_partitions=future_partitions,
            missing_future_partitions=missing_future_partitions,
            parent=parent,
            registered_partitions=registered_partitions,
            oldest_default_day=oldest_default_day,
        )

        partition_status.additional_properties = d
        return partition_status

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
