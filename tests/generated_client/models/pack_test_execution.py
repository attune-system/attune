from __future__ import annotations

import datetime
from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

T = TypeVar("T", bound="PackTestExecution")


@_attrs_define
class PackTestExecution:
    """Pack test execution record

    Attributes:
        created (datetime.datetime):
        duration_ms (int):
        execution_time (datetime.datetime):
        failed (int):
        id (int):
        pack_id (int):
        pack_version (str):
        pass_rate (float):
        passed (int):
        result (Any):
        skipped (int):
        total_tests (int):
        trigger_reason (str):
    """

    created: datetime.datetime
    duration_ms: int
    execution_time: datetime.datetime
    failed: int
    id: int
    pack_id: int
    pack_version: str
    pass_rate: float
    passed: int
    result: Any
    skipped: int
    total_tests: int
    trigger_reason: str
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        created = self.created.isoformat()

        duration_ms = self.duration_ms

        execution_time = self.execution_time.isoformat()

        failed = self.failed

        id = self.id

        pack_id = self.pack_id

        pack_version = self.pack_version

        pass_rate = self.pass_rate

        passed = self.passed

        result = self.result

        skipped = self.skipped

        total_tests = self.total_tests

        trigger_reason = self.trigger_reason

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "created": created,
                "durationMs": duration_ms,
                "executionTime": execution_time,
                "failed": failed,
                "id": id,
                "packId": pack_id,
                "packVersion": pack_version,
                "passRate": pass_rate,
                "passed": passed,
                "result": result,
                "skipped": skipped,
                "totalTests": total_tests,
                "triggerReason": trigger_reason,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        created = datetime.datetime.fromisoformat(d.pop("created"))

        duration_ms = d.pop("durationMs")

        execution_time = datetime.datetime.fromisoformat(d.pop("executionTime"))

        failed = d.pop("failed")

        id = d.pop("id")

        pack_id = d.pop("packId")

        pack_version = d.pop("packVersion")

        pass_rate = d.pop("passRate")

        passed = d.pop("passed")

        result = d.pop("result")

        skipped = d.pop("skipped")

        total_tests = d.pop("totalTests")

        trigger_reason = d.pop("triggerReason")

        pack_test_execution = cls(
            created=created,
            duration_ms=duration_ms,
            execution_time=execution_time,
            failed=failed,
            id=id,
            pack_id=pack_id,
            pack_version=pack_version,
            pass_rate=pass_rate,
            passed=passed,
            result=result,
            skipped=skipped,
            total_tests=total_tests,
            trigger_reason=trigger_reason,
        )

        pack_test_execution.additional_properties = d
        return pack_test_execution

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
