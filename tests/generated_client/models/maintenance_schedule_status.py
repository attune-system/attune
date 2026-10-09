from __future__ import annotations

import datetime
from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

from ..models.maintenance_job import MaintenanceJob
from ..types import UNSET, Unset

T = TypeVar("T", bound="MaintenanceScheduleStatus")


@_attrs_define
class MaintenanceScheduleStatus:
    """
    Attributes:
        job (MaintenanceJob):
        next_due (datetime.datetime):
        last_success (datetime.datetime | None | Unset):
    """

    job: MaintenanceJob
    next_due: datetime.datetime
    last_success: datetime.datetime | None | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        job = self.job.value

        next_due = self.next_due.isoformat()

        last_success: None | str | Unset
        if isinstance(self.last_success, Unset):
            last_success = UNSET
        elif isinstance(self.last_success, datetime.datetime):
            last_success = self.last_success.isoformat()
        else:
            last_success = self.last_success

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "job": job,
                "next_due": next_due,
            }
        )
        if last_success is not UNSET:
            field_dict["last_success"] = last_success

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        d = dict(src_dict)
        job = MaintenanceJob(d.pop("job"))

        next_due = datetime.datetime.fromisoformat(d.pop("next_due"))

        def _parse_last_success(data: object) -> datetime.datetime | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                last_success_type_0 = datetime.datetime.fromisoformat(data)

                return last_success_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(datetime.datetime | None | Unset, data)

        last_success = _parse_last_success(d.pop("last_success", UNSET))

        maintenance_schedule_status = cls(
            job=job,
            next_due=next_due,
            last_success=last_success,
        )

        maintenance_schedule_status.additional_properties = d
        return maintenance_schedule_status

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
