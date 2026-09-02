from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.pack_install_provenance import PackInstallProvenance
    from ..models.pack_response import PackResponse
    from ..models.pack_test_result import PackTestResult


T = TypeVar("T", bound="TestPackResponse202Data")


@_attrs_define
class TestPackResponse202Data:
    """Response for pack install/register operations with test results

    Attributes:
        pack (PackResponse): Response DTO for pack information
        tests_skipped (bool): Whether tests were skipped
        install_id (int | None | Unset): ID of the pack install tracking record, present when tests were dispatched.
        install_status (None | str | Unset): Current install status: pending, running, activating, succeeded, failed, or
            rolled_back.
        provenance (None | PackInstallProvenance | Unset):
        test_result (None | PackTestResult | Unset):
    """

    pack: PackResponse
    tests_skipped: bool
    install_id: int | None | Unset = UNSET
    install_status: None | str | Unset = UNSET
    provenance: None | PackInstallProvenance | Unset = UNSET
    test_result: None | PackTestResult | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.pack_install_provenance import (
            PackInstallProvenance,
        )
        from ..models.pack_test_result import PackTestResult

        pack = self.pack.to_dict()

        tests_skipped = self.tests_skipped

        install_id: int | None | Unset
        if isinstance(self.install_id, Unset):
            install_id = UNSET
        else:
            install_id = self.install_id

        install_status: None | str | Unset
        if isinstance(self.install_status, Unset):
            install_status = UNSET
        else:
            install_status = self.install_status

        provenance: dict[str, Any] | None | Unset
        if isinstance(self.provenance, Unset):
            provenance = UNSET
        elif isinstance(self.provenance, PackInstallProvenance):
            provenance = self.provenance.to_dict()
        else:
            provenance = self.provenance

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
        if install_id is not UNSET:
            field_dict["install_id"] = install_id
        if install_status is not UNSET:
            field_dict["install_status"] = install_status
        if provenance is not UNSET:
            field_dict["provenance"] = provenance
        if test_result is not UNSET:
            field_dict["test_result"] = test_result

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        from ..models.pack_install_provenance import (
            PackInstallProvenance,
        )
        from ..models.pack_response import PackResponse
        from ..models.pack_test_result import PackTestResult

        d = dict(src_dict)
        pack = PackResponse.from_dict(d.pop("pack"))

        tests_skipped = d.pop("tests_skipped")

        def _parse_install_id(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        install_id = _parse_install_id(d.pop("install_id", UNSET))

        def _parse_install_status(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        install_status = _parse_install_status(d.pop("install_status", UNSET))

        def _parse_provenance(data: object) -> None | PackInstallProvenance | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                provenance_type_1 = PackInstallProvenance.from_dict(data)

                return provenance_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(None | PackInstallProvenance | Unset, data)

        provenance = _parse_provenance(d.pop("provenance", UNSET))

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

        test_pack_response_202_data = cls(
            pack=pack,
            tests_skipped=tests_skipped,
            install_id=install_id,
            install_status=install_status,
            provenance=provenance,
            test_result=test_result,
        )

        test_pack_response_202_data.additional_properties = d
        return test_pack_response_202_data

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
