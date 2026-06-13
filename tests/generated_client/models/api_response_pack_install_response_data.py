from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.pack_response import PackResponse
    from ..models.pack_test_result import PackTestResult


T = TypeVar("T", bound="ApiResponsePackInstallResponseData")


@_attrs_define
class ApiResponsePackInstallResponseData:
    """Response for pack install/register operations with test results

    Attributes:
        pack (PackResponse): Response DTO for pack information
        tests_skipped (bool): Whether tests were skipped
        test_result (None | PackTestResult | Unset):
    """

    pack: PackResponse
    tests_skipped: bool
    test_result: None | PackTestResult | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.pack_test_result import PackTestResult

        pack = self.pack.to_dict()

        tests_skipped = self.tests_skipped

        test_result: dict[str, Any] | None | Unset
        if isinstance(self.test_result, Unset):
            test_result = UNSET
        elif isinstance(self.test_result, PackTestResult):
            test_result = self.test_result.to_dict()
        else:
            test_result = self.test_result

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "pack": pack,
                "tests_skipped": tests_skipped,
            }
        )
        if test_result is not UNSET:
            field_dict["test_result"] = test_result

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.pack_response import PackResponse
        from ..models.pack_test_result import PackTestResult

        d = dict(src_dict)
        pack = PackResponse.from_dict(d.pop("pack"))

        tests_skipped = d.pop("tests_skipped")

        def _parse_test_result(data: object) -> None | PackTestResult | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                test_result_type_1 = PackTestResult.from_dict(data)

                return test_result_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(None | PackTestResult | Unset, data)

        test_result = _parse_test_result(d.pop("test_result", UNSET))

        api_response_pack_install_response_data = cls(
            pack=pack,
            tests_skipped=tests_skipped,
            test_result=test_result,
        )

        api_response_pack_install_response_data.additional_properties = d
        return api_response_pack_install_response_data

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
