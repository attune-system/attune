from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.component_counts import ComponentCounts
    from ..models.test_result import TestResult
    from ..models.validation_results import ValidationResults


T = TypeVar("T", bound="RegisteredPack")


@_attrs_define
class RegisteredPack:
    """Information about a registered pack

    Attributes:
        components_registered (ComponentCounts): Component counts
        pack_id (int): Pack database ID
        pack_ref (str): Pack reference
        pack_version (str): Pack version
        storage_path (str): Permanent storage path
        validation_results (ValidationResults): Validation results
        install_id (int | None | Unset): Pack install tracking ID when tests were dispatched or failed to dispatch
        install_status (None | str | Unset): Current pack install status
        test_result (None | TestResult | Unset):
    """

    components_registered: ComponentCounts
    pack_id: int
    pack_ref: str
    pack_version: str
    storage_path: str
    validation_results: ValidationResults
    install_id: int | None | Unset = UNSET
    install_status: None | str | Unset = UNSET
    test_result: None | TestResult | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.test_result import TestResult

        components_registered = self.components_registered.to_dict()

        pack_id = self.pack_id

        pack_ref = self.pack_ref

        pack_version = self.pack_version

        storage_path = self.storage_path

        validation_results = self.validation_results.to_dict()

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

        test_result: dict[str, Any] | None | Unset
        if isinstance(self.test_result, Unset):
            test_result = UNSET
        elif isinstance(self.test_result, TestResult):
            test_result = self.test_result.to_dict()
        else:
            test_result = self.test_result

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "components_registered": components_registered,
                "pack_id": pack_id,
                "pack_ref": pack_ref,
                "pack_version": pack_version,
                "storage_path": storage_path,
                "validation_results": validation_results,
            }
        )
        if install_id is not UNSET:
            field_dict["install_id"] = install_id
        if install_status is not UNSET:
            field_dict["install_status"] = install_status
        if test_result is not UNSET:
            field_dict["test_result"] = test_result

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        from ..models.component_counts import ComponentCounts
        from ..models.test_result import TestResult
        from ..models.validation_results import ValidationResults

        d = dict(src_dict)
        components_registered = ComponentCounts.from_dict(
            d.pop("components_registered")
        )

        pack_id = d.pop("pack_id")

        pack_ref = d.pop("pack_ref")

        pack_version = d.pop("pack_version")

        storage_path = d.pop("storage_path")

        validation_results = ValidationResults.from_dict(d.pop("validation_results"))

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

        def _parse_test_result(data: object) -> None | TestResult | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                test_result_type_1 = TestResult.from_dict(data)

                return test_result_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(None | TestResult | Unset, data)

        test_result = _parse_test_result(d.pop("test_result", UNSET))

        registered_pack = cls(
            components_registered=components_registered,
            pack_id=pack_id,
            pack_ref=pack_ref,
            pack_version=pack_version,
            storage_path=storage_path,
            validation_results=validation_results,
            install_id=install_id,
            install_status=install_status,
            test_result=test_result,
        )

        registered_pack.additional_properties = d
        return registered_pack

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
