from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from typing_extensions import Self

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.dashboard_data_request_filters import DashboardDataRequestFilters
    from ..models.dashboard_time_range_request import DashboardTimeRangeRequest


T = TypeVar("T", bound="DashboardDataRequest")


@_attrs_define
class DashboardDataRequest:
    """
    Attributes:
        card_ids (list[str] | None | Unset):  Example: ['overview_backlog', 'event_rate'].
        filters (DashboardDataRequestFilters | Unset):
        include_meta (bool | Unset):
        request_id (None | str | Unset):
        source_ids (list[str] | None | Unset): Optional source selector.

            Membership only: request order is ignored. The response emits `sources[]`
            in canonical `source_id` ascending order. Example: ['queue_backlog', 'event_count'].
        time_range (DashboardTimeRangeRequest | None | Unset):
        time_window (None | str | Unset):  Example: 24h.
        timezone (None | str | Unset):  Example: America/Chicago.
    """

    card_ids: list[str] | None | Unset = UNSET
    filters: DashboardDataRequestFilters | Unset = UNSET
    include_meta: bool | Unset = UNSET
    request_id: None | str | Unset = UNSET
    source_ids: list[str] | None | Unset = UNSET
    time_range: DashboardTimeRangeRequest | None | Unset = UNSET
    time_window: None | str | Unset = UNSET
    timezone: None | str | Unset = UNSET

    def to_dict(self) -> dict[str, Any]:
        from ..models.dashboard_time_range_request import DashboardTimeRangeRequest

        card_ids: list[str] | None | Unset
        if isinstance(self.card_ids, Unset):
            card_ids = UNSET
        elif isinstance(self.card_ids, list):
            card_ids = self.card_ids

        else:
            card_ids = self.card_ids

        filters: dict[str, Any] | Unset = UNSET
        if not isinstance(self.filters, Unset):
            filters = self.filters.to_dict()

        include_meta = self.include_meta

        request_id: None | str | Unset
        if isinstance(self.request_id, Unset):
            request_id = UNSET
        else:
            request_id = self.request_id

        source_ids: list[str] | None | Unset
        if isinstance(self.source_ids, Unset):
            source_ids = UNSET
        elif isinstance(self.source_ids, list):
            source_ids = self.source_ids

        else:
            source_ids = self.source_ids

        time_range: dict[str, Any] | None | Unset
        if isinstance(self.time_range, Unset):
            time_range = UNSET
        elif isinstance(self.time_range, DashboardTimeRangeRequest):
            time_range = self.time_range.to_dict()
        else:
            time_range = self.time_range

        time_window: None | str | Unset
        if isinstance(self.time_window, Unset):
            time_window = UNSET
        else:
            time_window = self.time_window

        timezone: None | str | Unset
        if isinstance(self.timezone, Unset):
            timezone = UNSET
        else:
            timezone = self.timezone

        field_dict: dict[str, Any] = {}

        field_dict.update({})
        if card_ids is not UNSET:
            field_dict["card_ids"] = card_ids
        if filters is not UNSET:
            field_dict["filters"] = filters
        if include_meta is not UNSET:
            field_dict["include_meta"] = include_meta
        if request_id is not UNSET:
            field_dict["request_id"] = request_id
        if source_ids is not UNSET:
            field_dict["source_ids"] = source_ids
        if time_range is not UNSET:
            field_dict["time_range"] = time_range
        if time_window is not UNSET:
            field_dict["time_window"] = time_window
        if timezone is not UNSET:
            field_dict["timezone"] = timezone

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        from ..models.dashboard_data_request_filters import (
            DashboardDataRequestFilters,
        )
        from ..models.dashboard_time_range_request import (
            DashboardTimeRangeRequest,
        )

        d = dict(src_dict)

        def _parse_card_ids(data: object) -> list[str] | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, list):
                    raise TypeError()
                card_ids_type_0 = cast(list[str], data)

                return card_ids_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(list[str] | None | Unset, data)

        card_ids = _parse_card_ids(d.pop("card_ids", UNSET))

        _filters = d.pop("filters", UNSET)
        filters: DashboardDataRequestFilters | Unset
        if isinstance(_filters, Unset):
            filters = UNSET
        else:
            filters = DashboardDataRequestFilters.from_dict(_filters)

        include_meta = d.pop("include_meta", UNSET)

        def _parse_request_id(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        request_id = _parse_request_id(d.pop("request_id", UNSET))

        def _parse_source_ids(data: object) -> list[str] | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, list):
                    raise TypeError()
                source_ids_type_0 = cast(list[str], data)

                return source_ids_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(list[str] | None | Unset, data)

        source_ids = _parse_source_ids(d.pop("source_ids", UNSET))

        def _parse_time_range(data: object) -> DashboardTimeRangeRequest | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                time_range_type_1 = DashboardTimeRangeRequest.from_dict(data)

                return time_range_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(DashboardTimeRangeRequest | None | Unset, data)

        time_range = _parse_time_range(d.pop("time_range", UNSET))

        def _parse_time_window(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        time_window = _parse_time_window(d.pop("time_window", UNSET))

        def _parse_timezone(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        timezone = _parse_timezone(d.pop("timezone", UNSET))

        dashboard_data_request = cls(
            card_ids=card_ids,
            filters=filters,
            include_meta=include_meta,
            request_id=request_id,
            source_ids=source_ids,
            time_range=time_range,
            time_window=time_window,
            timezone=timezone,
        )

        return dashboard_data_request
