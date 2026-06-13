from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.test_status import TestStatus
from ..types import UNSET, Unset

T = TypeVar("T", bound="TestCaseResult")


@_attrs_define
class TestCaseResult:
    """Individual test case result

    Attributes:
        duration_ms (int):
        name (str):
        status (TestStatus): Test status enum
        error_message (None | str | Unset):
        stderr (None | str | Unset):
        stdout (None | str | Unset):
    """

    duration_ms: int
    name: str
    status: TestStatus
    error_message: None | str | Unset = UNSET
    stderr: None | str | Unset = UNSET
    stdout: None | str | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        duration_ms = self.duration_ms

        name = self.name

        status = self.status.value

        error_message: None | str | Unset
        if isinstance(self.error_message, Unset):
            error_message = UNSET
        else:
            error_message = self.error_message

        stderr: None | str | Unset
        if isinstance(self.stderr, Unset):
            stderr = UNSET
        else:
            stderr = self.stderr

        stdout: None | str | Unset
        if isinstance(self.stdout, Unset):
            stdout = UNSET
        else:
            stdout = self.stdout

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "durationMs": duration_ms,
                "name": name,
                "status": status,
            }
        )
        if error_message is not UNSET:
            field_dict["errorMessage"] = error_message
        if stderr is not UNSET:
            field_dict["stderr"] = stderr
        if stdout is not UNSET:
            field_dict["stdout"] = stdout

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        duration_ms = d.pop("durationMs")

        name = d.pop("name")

        status = TestStatus(d.pop("status"))

        def _parse_error_message(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        error_message = _parse_error_message(d.pop("errorMessage", UNSET))

        def _parse_stderr(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        stderr = _parse_stderr(d.pop("stderr", UNSET))

        def _parse_stdout(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        stdout = _parse_stdout(d.pop("stdout", UNSET))

        test_case_result = cls(
            duration_ms=duration_ms,
            name=name,
            status=status,
            error_message=error_message,
            stderr=stderr,
            stdout=stdout,
        )

        test_case_result.additional_properties = d
        return test_case_result

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
