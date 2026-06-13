from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

if TYPE_CHECKING:
    from ..models.test_case_result import TestCaseResult


T = TypeVar("T", bound="TestSuiteResult")


@_attrs_define
class TestSuiteResult:
    """Test suite result (collection of test cases)

    Attributes:
        duration_ms (int):
        failed (int):
        name (str):
        passed (int):
        runner_type (str):
        skipped (int):
        test_cases (list[TestCaseResult]):
        total (int):
    """

    duration_ms: int
    failed: int
    name: str
    passed: int
    runner_type: str
    skipped: int
    test_cases: list[TestCaseResult]
    total: int
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        duration_ms = self.duration_ms

        failed = self.failed

        name = self.name

        passed = self.passed

        runner_type = self.runner_type

        skipped = self.skipped

        test_cases = []
        for test_cases_item_data in self.test_cases:
            test_cases_item = test_cases_item_data.to_dict()
            test_cases.append(test_cases_item)

        total = self.total

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "durationMs": duration_ms,
                "failed": failed,
                "name": name,
                "passed": passed,
                "runnerType": runner_type,
                "skipped": skipped,
                "testCases": test_cases,
                "total": total,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.test_case_result import TestCaseResult

        d = dict(src_dict)
        duration_ms = d.pop("durationMs")

        failed = d.pop("failed")

        name = d.pop("name")

        passed = d.pop("passed")

        runner_type = d.pop("runnerType")

        skipped = d.pop("skipped")

        test_cases = []
        _test_cases = d.pop("testCases")
        for test_cases_item_data in _test_cases:
            test_cases_item = TestCaseResult.from_dict(test_cases_item_data)

            test_cases.append(test_cases_item)

        total = d.pop("total")

        test_suite_result = cls(
            duration_ms=duration_ms,
            failed=failed,
            name=name,
            passed=passed,
            runner_type=runner_type,
            skipped=skipped,
            test_cases=test_cases,
            total=total,
        )

        test_suite_result.additional_properties = d
        return test_suite_result

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
