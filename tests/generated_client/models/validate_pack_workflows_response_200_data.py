from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

if TYPE_CHECKING:
    from ..models.validate_pack_workflows_response_200_data_errors import (
        ValidatePackWorkflowsResponse200DataErrors,
    )


T = TypeVar("T", bound="ValidatePackWorkflowsResponse200Data")


@_attrs_define
class ValidatePackWorkflowsResponse200Data:
    """Response for pack workflow validation operation

    Attributes:
        error_count (int): Number of workflows with errors
        errors (ValidatePackWorkflowsResponse200DataErrors): Validation errors by workflow reference
        pack_ref (str): Pack reference
        validated_count (int): Number of workflows validated
    """

    error_count: int
    errors: ValidatePackWorkflowsResponse200DataErrors
    pack_ref: str
    validated_count: int
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        error_count = self.error_count

        errors = self.errors.to_dict()

        pack_ref = self.pack_ref

        validated_count = self.validated_count

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "error_count": error_count,
                "errors": errors,
                "pack_ref": pack_ref,
                "validated_count": validated_count,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.validate_pack_workflows_response_200_data_errors import (
            ValidatePackWorkflowsResponse200DataErrors,
        )

        d = dict(src_dict)
        error_count = d.pop("error_count")

        errors = ValidatePackWorkflowsResponse200DataErrors.from_dict(d.pop("errors"))

        pack_ref = d.pop("pack_ref")

        validated_count = d.pop("validated_count")

        validate_pack_workflows_response_200_data = cls(
            error_count=error_count,
            errors=errors,
            pack_ref=pack_ref,
            validated_count=validated_count,
        )

        validate_pack_workflows_response_200_data.additional_properties = d
        return validate_pack_workflows_response_200_data

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
