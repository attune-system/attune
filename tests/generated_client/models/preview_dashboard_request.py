from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar

from attrs import define as _attrs_define
from typing_extensions import Self

if TYPE_CHECKING:
    from ..models.create_dashboard_request import CreateDashboardRequest
    from ..models.dashboard_data_request import DashboardDataRequest


T = TypeVar("T", bound="PreviewDashboardRequest")


@_attrs_define
class PreviewDashboardRequest:
    """
    Attributes:
        dashboard (CreateDashboardRequest):
        data_request (DashboardDataRequest):
    """

    dashboard: CreateDashboardRequest
    data_request: DashboardDataRequest

    def to_dict(self) -> dict[str, Any]:
        dashboard = self.dashboard.to_dict()

        data_request = self.data_request.to_dict()

        field_dict: dict[str, Any] = {}

        field_dict.update(
            {
                "dashboard": dashboard,
                "data_request": data_request,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        from ..models.create_dashboard_request import (
            CreateDashboardRequest,
        )
        from ..models.dashboard_data_request import (
            DashboardDataRequest,
        )

        d = dict(src_dict)
        dashboard = CreateDashboardRequest.from_dict(d.pop("dashboard"))

        data_request = DashboardDataRequest.from_dict(d.pop("data_request"))

        preview_dashboard_request = cls(
            dashboard=dashboard,
            data_request=data_request,
        )

        return preview_dashboard_request
