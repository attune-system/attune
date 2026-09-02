from __future__ import annotations

import datetime
from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

T = TypeVar("T", bound="GetFailureRateAnalyticsResponse200Data")


@_attrs_define
class GetFailureRateAnalyticsResponse200Data:
    """Response for the execution failure rate summary.

    Attributes:
        completed_count (int): Number of completed executions Example: 85.
        failed_count (int): Number of failed executions Example: 12.
        failure_rate_pct (float): Failure rate as a percentage (0.0 – 100.0) Example: 15.0.
        since (datetime.datetime): Time range start
        timeout_count (int): Number of timed-out executions Example: 3.
        total_terminal (int): Total executions reaching a terminal state in the window Example: 100.
        until (datetime.datetime): Time range end
    """

    completed_count: int
    failed_count: int
    failure_rate_pct: float
    since: datetime.datetime
    timeout_count: int
    total_terminal: int
    until: datetime.datetime
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        completed_count = self.completed_count

        failed_count = self.failed_count

        failure_rate_pct = self.failure_rate_pct

        since = self.since.isoformat()

        timeout_count = self.timeout_count

        total_terminal = self.total_terminal

        until = self.until.isoformat()

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "completed_count": completed_count,
                "failed_count": failed_count,
                "failure_rate_pct": failure_rate_pct,
                "since": since,
                "timeout_count": timeout_count,
                "total_terminal": total_terminal,
                "until": until,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        d = dict(src_dict)
        completed_count = d.pop("completed_count")

        failed_count = d.pop("failed_count")

        failure_rate_pct = d.pop("failure_rate_pct")

        since = datetime.datetime.fromisoformat(d.pop("since"))

        timeout_count = d.pop("timeout_count")

        total_terminal = d.pop("total_terminal")

        until = datetime.datetime.fromisoformat(d.pop("until"))

        get_failure_rate_analytics_response_200_data = cls(
            completed_count=completed_count,
            failed_count=failed_count,
            failure_rate_pct=failure_rate_pct,
            since=since,
            timeout_count=timeout_count,
            total_terminal=total_terminal,
            until=until,
        )

        get_failure_rate_analytics_response_200_data.additional_properties = d
        return get_failure_rate_analytics_response_200_data

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
