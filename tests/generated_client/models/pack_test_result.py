from __future__ import annotations

import datetime
from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

if TYPE_CHECKING:
    from ..models.test_suite_result import TestSuiteResult


T = TypeVar("T", bound="PackTestResult")


@_attrs_define
class PackTestResult:
    """Pack test result structure (not from DB, used for test execution)

    Attributes:
        duration_ms (int):
        execution_time (datetime.datetime):
        failed (int):
        pack_ref (str):
        pack_version (str):
        pass_rate (float):
        passed (int):
        skipped (int):
        status (str):
        test_suites (list[TestSuiteResult]):
        total_tests (int):
    """

    duration_ms: int
    execution_time: datetime.datetime
    failed: int
    pack_ref: str
    pack_version: str
    pass_rate: float
    passed: int
    skipped: int
    status: str
    test_suites: list[TestSuiteResult]
    total_tests: int
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        duration_ms = self.duration_ms

        execution_time = self.execution_time.isoformat()

        failed = self.failed

        pack_ref = self.pack_ref

        pack_version = self.pack_version

        pass_rate = self.pass_rate

        passed = self.passed

        skipped = self.skipped

        status = self.status

        test_suites = []
        for test_suites_item_data in self.test_suites:
            test_suites_item = test_suites_item_data.to_dict()
            test_suites.append(test_suites_item)

        total_tests = self.total_tests

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "durationMs": duration_ms,
                "executionTime": execution_time,
                "failed": failed,
                "packRef": pack_ref,
                "packVersion": pack_version,
                "passRate": pass_rate,
                "passed": passed,
                "skipped": skipped,
                "status": status,
                "testSuites": test_suites,
                "totalTests": total_tests,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.test_suite_result import TestSuiteResult

        d = dict(src_dict)
        duration_ms = d.pop("durationMs")

        execution_time = datetime.datetime.fromisoformat(d.pop("executionTime"))

        failed = d.pop("failed")

        pack_ref = d.pop("packRef")

        pack_version = d.pop("packVersion")

        pass_rate = d.pop("passRate")

        passed = d.pop("passed")

        skipped = d.pop("skipped")

        status = d.pop("status")

        test_suites = []
        _test_suites = d.pop("testSuites")
        for test_suites_item_data in _test_suites:
            test_suites_item = TestSuiteResult.from_dict(test_suites_item_data)

            test_suites.append(test_suites_item)

        total_tests = d.pop("totalTests")

        pack_test_result = cls(
            duration_ms=duration_ms,
            execution_time=execution_time,
            failed=failed,
            pack_ref=pack_ref,
            pack_version=pack_version,
            pass_rate=pass_rate,
            passed=passed,
            skipped=skipped,
            status=status,
            test_suites=test_suites,
            total_tests=total_tests,
        )

        pack_test_result.additional_properties = d
        return pack_test_result

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
