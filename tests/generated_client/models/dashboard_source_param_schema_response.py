from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

T = TypeVar("T", bound="DashboardSourceParamSchemaResponse")


@_attrs_define
class DashboardSourceParamSchemaResponse:
    """
    Attributes:
        optional (list[str]):
        required (list[str]):
    """

    optional: list[str]
    required: list[str]
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        optional = self.optional

        required = self.required

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "optional": optional,
                "required": required,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        d = dict(src_dict)
        optional = cast(list[str], d.pop("optional"))

        required = cast(list[str], d.pop("required"))

        dashboard_source_param_schema_response = cls(
            optional=optional,
            required=required,
        )

        dashboard_source_param_schema_response.additional_properties = d
        return dashboard_source_param_schema_response

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
