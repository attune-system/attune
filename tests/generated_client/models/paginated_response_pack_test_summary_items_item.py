from __future__ import annotations

import datetime
from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

T = TypeVar("T", bound="PaginatedResponsePackTestSummaryItemsItem")


@_attrs_define
class PaginatedResponsePackTestSummaryItemsItem:
    """Pack test summary view

    Attributes:
        duration_ms (int):
        failed (int):
        pack_id (int):
        pack_label (str):
        pack_ref (str):
        pack_version (str):
        pass_rate (float):
        passed (int):
        skipped (int):
        test_execution_id (int):
        test_time (datetime.datetime):
        total_tests (int):
        trigger_reason (str):
    """

    duration_ms: int
    failed: int
    pack_id: int
    pack_label: str
    pack_ref: str
    pack_version: str
    pass_rate: float
    passed: int
    skipped: int
    test_execution_id: int
    test_time: datetime.datetime
    total_tests: int
    trigger_reason: str
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        duration_ms = self.duration_ms

        failed = self.failed

        pack_id = self.pack_id

        pack_label = self.pack_label

        pack_ref = self.pack_ref

        pack_version = self.pack_version

        pass_rate = self.pass_rate

        passed = self.passed

        skipped = self.skipped

        test_execution_id = self.test_execution_id

        test_time = self.test_time.isoformat()

        total_tests = self.total_tests

        trigger_reason = self.trigger_reason

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "durationMs": duration_ms,
                "failed": failed,
                "packId": pack_id,
                "packLabel": pack_label,
                "packRef": pack_ref,
                "packVersion": pack_version,
                "passRate": pass_rate,
                "passed": passed,
                "skipped": skipped,
                "testExecutionId": test_execution_id,
                "testTime": test_time,
                "totalTests": total_tests,
                "triggerReason": trigger_reason,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        duration_ms = d.pop("durationMs")

        failed = d.pop("failed")

        pack_id = d.pop("packId")

        pack_label = d.pop("packLabel")

        pack_ref = d.pop("packRef")

        pack_version = d.pop("packVersion")

        pass_rate = d.pop("passRate")

        passed = d.pop("passed")

        skipped = d.pop("skipped")

        test_execution_id = d.pop("testExecutionId")

        test_time = datetime.datetime.fromisoformat(d.pop("testTime"))

        total_tests = d.pop("totalTests")

        trigger_reason = d.pop("triggerReason")

        paginated_response_pack_test_summary_items_item = cls(
            duration_ms=duration_ms,
            failed=failed,
            pack_id=pack_id,
            pack_label=pack_label,
            pack_ref=pack_ref,
            pack_version=pack_version,
            pass_rate=pass_rate,
            passed=passed,
            skipped=skipped,
            test_execution_id=test_execution_id,
            test_time=test_time,
            total_tests=total_tests,
            trigger_reason=trigger_reason,
        )

        paginated_response_pack_test_summary_items_item.additional_properties = d
        return paginated_response_pack_test_summary_items_item

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
