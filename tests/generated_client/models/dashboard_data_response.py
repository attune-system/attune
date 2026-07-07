from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..types import UNSET, Unset
from typing import cast
import datetime

if TYPE_CHECKING:
  from ..models.dashboard_effective_time_range import DashboardEffectiveTimeRange
  from ..models.dashboard_source_result import DashboardSourceResult





T = TypeVar("T", bound="DashboardDataResponse")



@_attrs_define
class DashboardDataResponse:
    """ 
        Attributes:
            contract_version (int):
            dashboard_ref (str):
            dashboard_revision (int):
            effective_time_range (DashboardEffectiveTimeRange):
            partial (bool):
            resolved_at (datetime.datetime):
            sources (list[DashboardSourceResult]): Source results in canonical `source_id` ascending order.
            spec_version (int):
            request_id (None | str | Unset):
     """

    contract_version: int
    dashboard_ref: str
    dashboard_revision: int
    effective_time_range: DashboardEffectiveTimeRange
    partial: bool
    resolved_at: datetime.datetime
    sources: list[DashboardSourceResult]
    spec_version: int
    request_id: None | str | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)





    def to_dict(self) -> dict[str, Any]:
        from ..models.dashboard_effective_time_range import DashboardEffectiveTimeRange
        from ..models.dashboard_source_result import DashboardSourceResult
        contract_version = self.contract_version

        dashboard_ref = self.dashboard_ref

        dashboard_revision = self.dashboard_revision

        effective_time_range = self.effective_time_range.to_dict()

        partial = self.partial

        resolved_at = self.resolved_at.isoformat()

        sources = []
        for sources_item_data in self.sources:
            sources_item = sources_item_data.to_dict()
            sources.append(sources_item)



        spec_version = self.spec_version

        request_id: None | str | Unset
        if isinstance(self.request_id, Unset):
            request_id = UNSET
        else:
            request_id = self.request_id


        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update({
            "contract_version": contract_version,
            "dashboard_ref": dashboard_ref,
            "dashboard_revision": dashboard_revision,
            "effective_time_range": effective_time_range,
            "partial": partial,
            "resolved_at": resolved_at,
            "sources": sources,
            "spec_version": spec_version,
        })
        if request_id is not UNSET:
            field_dict["request_id"] = request_id

        return field_dict



    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.dashboard_effective_time_range import DashboardEffectiveTimeRange
        from ..models.dashboard_source_result import DashboardSourceResult
        d = dict(src_dict)
        contract_version = d.pop("contract_version")

        dashboard_ref = d.pop("dashboard_ref")

        dashboard_revision = d.pop("dashboard_revision")

        effective_time_range = DashboardEffectiveTimeRange.from_dict(d.pop("effective_time_range"))




        partial = d.pop("partial")

        resolved_at = datetime.datetime.fromisoformat(d.pop("resolved_at"))




        sources = []
        _sources = d.pop("sources")
        for sources_item_data in (_sources):
            sources_item = DashboardSourceResult.from_dict(sources_item_data)



            sources.append(sources_item)


        spec_version = d.pop("spec_version")

        def _parse_request_id(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        request_id = _parse_request_id(d.pop("request_id", UNSET))


        dashboard_data_response = cls(
            contract_version=contract_version,
            dashboard_ref=dashboard_ref,
            dashboard_revision=dashboard_revision,
            effective_time_range=effective_time_range,
            partial=partial,
            resolved_at=resolved_at,
            sources=sources,
            spec_version=spec_version,
            request_id=request_id,
        )


        dashboard_data_response.additional_properties = d
        return dashboard_data_response

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
