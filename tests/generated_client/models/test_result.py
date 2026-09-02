from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

T = TypeVar("T", bound="TestResult")


@_attrs_define
class TestResult:
    """Test result

    Attributes:
        failed (int): Number failed
        passed (int): Number passed
        status (str): Test status
        total_tests (int): Total number of tests
    """

    failed: int
    passed: int
    status: str
    total_tests: int
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        failed = self.failed

        passed = self.passed

        status = self.status

        total_tests = self.total_tests

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "failed": failed,
                "passed": passed,
                "status": status,
                "total_tests": total_tests,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        d = dict(src_dict)
        failed = d.pop("failed")

        passed = d.pop("passed")

        status = d.pop("status")

        total_tests = d.pop("total_tests")

        test_result = cls(
            failed=failed,
            passed=passed,
            status=status,
            total_tests=total_tests,
        )

        test_result.additional_properties = d
        return test_result

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
