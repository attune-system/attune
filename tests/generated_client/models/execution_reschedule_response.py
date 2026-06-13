from __future__ import annotations

import datetime
from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

if TYPE_CHECKING:
    from ..models.execution_response import ExecutionResponse


T = TypeVar("T", bound="ExecutionRescheduleResponse")


@_attrs_define
class ExecutionRescheduleResponse:
    """Response DTO for manual execution reschedule requests.

    Attributes:
        attempt_count (int): Number of reschedule attempts recorded for this execution. Example: 1.
        execution (ExecutionResponse): Response DTO for execution information
        last_attempt_at (datetime.datetime): Timestamp for the recorded reschedule attempt. Example:
            2024-01-13T10:35:00Z.
        message (str): Human-readable status of the republish request. Example: Execution request republished; pending
            scheduling.
    """

    attempt_count: int
    execution: ExecutionResponse
    last_attempt_at: datetime.datetime
    message: str
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        attempt_count = self.attempt_count

        execution = self.execution.to_dict()

        last_attempt_at = self.last_attempt_at.isoformat()

        message = self.message

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "attempt_count": attempt_count,
                "execution": execution,
                "last_attempt_at": last_attempt_at,
                "message": message,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.execution_response import ExecutionResponse

        d = dict(src_dict)
        attempt_count = d.pop("attempt_count")

        execution = ExecutionResponse.from_dict(d.pop("execution"))

        last_attempt_at = datetime.datetime.fromisoformat(d.pop("last_attempt_at"))

        message = d.pop("message")

        execution_reschedule_response = cls(
            attempt_count=attempt_count,
            execution=execution,
            last_attempt_at=last_attempt_at,
            message=message,
        )

        execution_reschedule_response.additional_properties = d
        return execution_reschedule_response

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
