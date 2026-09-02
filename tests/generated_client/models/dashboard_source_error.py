from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

if TYPE_CHECKING:
    from ..models.dashboard_source_error_details_type_0 import (
        DashboardSourceErrorDetailsType0,
    )


T = TypeVar("T", bound="DashboardSourceError")


@_attrs_define
class DashboardSourceError:
    """
    Attributes:
        code (str):
        details (DashboardSourceErrorDetailsType0 | None):
        message (str):
        retryable (bool):
    """

    code: str
    details: DashboardSourceErrorDetailsType0 | None
    message: str
    retryable: bool
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.dashboard_source_error_details_type_0 import (
            DashboardSourceErrorDetailsType0,
        )

        code = self.code

        details: dict[str, Any] | None
        if isinstance(self.details, DashboardSourceErrorDetailsType0):
            details = self.details.to_dict()
        else:
            details = self.details

        message = self.message

        retryable = self.retryable

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "code": code,
                "details": details,
                "message": message,
                "retryable": retryable,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        from ..models.dashboard_source_error_details_type_0 import (
            DashboardSourceErrorDetailsType0,
        )

        d = dict(src_dict)
        code = d.pop("code")

        def _parse_details(data: object) -> DashboardSourceErrorDetailsType0 | None:
            if data is None:
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                details_type_0 = DashboardSourceErrorDetailsType0.from_dict(data)

                return details_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(DashboardSourceErrorDetailsType0 | None, data)

        details = _parse_details(d.pop("details"))

        message = d.pop("message")

        retryable = d.pop("retryable")

        dashboard_source_error = cls(
            code=code,
            details=details,
            message=message,
            retryable=retryable,
        )

        dashboard_source_error.additional_properties = d
        return dashboard_source_error

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
