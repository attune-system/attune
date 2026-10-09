from __future__ import annotations

import datetime
from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

from ..models.summary_kind import SummaryKind
from ..types import UNSET, Unset

T = TypeVar("T", bound="SummaryStatus")


@_attrs_define
class SummaryStatus:
    """
    Attributes:
        coverage_hours (int):
        dirty_hours (int):
        dirty_notifications (int):
        kind (SummaryKind): Independent hourly materializations. Enum order is also the state-lock order.
        covered_since (datetime.datetime | None | Unset): Extrema only. They do not assert continuous coverage.
        covered_until (datetime.datetime | None | Unset):
        latest_success (datetime.datetime | None | Unset):
        oldest_dirty_bucket (datetime.datetime | None | Unset):
        oldest_notification (datetime.datetime | None | Unset):
    """

    coverage_hours: int
    dirty_hours: int
    dirty_notifications: int
    kind: SummaryKind
    covered_since: datetime.datetime | None | Unset = UNSET
    covered_until: datetime.datetime | None | Unset = UNSET
    latest_success: datetime.datetime | None | Unset = UNSET
    oldest_dirty_bucket: datetime.datetime | None | Unset = UNSET
    oldest_notification: datetime.datetime | None | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        coverage_hours = self.coverage_hours

        dirty_hours = self.dirty_hours

        dirty_notifications = self.dirty_notifications

        kind = self.kind.value

        covered_since: None | str | Unset
        if isinstance(self.covered_since, Unset):
            covered_since = UNSET
        elif isinstance(self.covered_since, datetime.datetime):
            covered_since = self.covered_since.isoformat()
        else:
            covered_since = self.covered_since

        covered_until: None | str | Unset
        if isinstance(self.covered_until, Unset):
            covered_until = UNSET
        elif isinstance(self.covered_until, datetime.datetime):
            covered_until = self.covered_until.isoformat()
        else:
            covered_until = self.covered_until

        latest_success: None | str | Unset
        if isinstance(self.latest_success, Unset):
            latest_success = UNSET
        elif isinstance(self.latest_success, datetime.datetime):
            latest_success = self.latest_success.isoformat()
        else:
            latest_success = self.latest_success

        oldest_dirty_bucket: None | str | Unset
        if isinstance(self.oldest_dirty_bucket, Unset):
            oldest_dirty_bucket = UNSET
        elif isinstance(self.oldest_dirty_bucket, datetime.datetime):
            oldest_dirty_bucket = self.oldest_dirty_bucket.isoformat()
        else:
            oldest_dirty_bucket = self.oldest_dirty_bucket

        oldest_notification: None | str | Unset
        if isinstance(self.oldest_notification, Unset):
            oldest_notification = UNSET
        elif isinstance(self.oldest_notification, datetime.datetime):
            oldest_notification = self.oldest_notification.isoformat()
        else:
            oldest_notification = self.oldest_notification

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "coverage_hours": coverage_hours,
                "dirty_hours": dirty_hours,
                "dirty_notifications": dirty_notifications,
                "kind": kind,
            }
        )
        if covered_since is not UNSET:
            field_dict["covered_since"] = covered_since
        if covered_until is not UNSET:
            field_dict["covered_until"] = covered_until
        if latest_success is not UNSET:
            field_dict["latest_success"] = latest_success
        if oldest_dirty_bucket is not UNSET:
            field_dict["oldest_dirty_bucket"] = oldest_dirty_bucket
        if oldest_notification is not UNSET:
            field_dict["oldest_notification"] = oldest_notification

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        d = dict(src_dict)
        coverage_hours = d.pop("coverage_hours")

        dirty_hours = d.pop("dirty_hours")

        dirty_notifications = d.pop("dirty_notifications")

        kind = SummaryKind(d.pop("kind"))

        def _parse_covered_since(data: object) -> datetime.datetime | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                covered_since_type_0 = datetime.datetime.fromisoformat(data)

                return covered_since_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(datetime.datetime | None | Unset, data)

        covered_since = _parse_covered_since(d.pop("covered_since", UNSET))

        def _parse_covered_until(data: object) -> datetime.datetime | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                covered_until_type_0 = datetime.datetime.fromisoformat(data)

                return covered_until_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(datetime.datetime | None | Unset, data)

        covered_until = _parse_covered_until(d.pop("covered_until", UNSET))

        def _parse_latest_success(data: object) -> datetime.datetime | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                latest_success_type_0 = datetime.datetime.fromisoformat(data)

                return latest_success_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(datetime.datetime | None | Unset, data)

        latest_success = _parse_latest_success(d.pop("latest_success", UNSET))

        def _parse_oldest_dirty_bucket(
            data: object,
        ) -> datetime.datetime | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                oldest_dirty_bucket_type_0 = datetime.datetime.fromisoformat(data)

                return oldest_dirty_bucket_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(datetime.datetime | None | Unset, data)

        oldest_dirty_bucket = _parse_oldest_dirty_bucket(
            d.pop("oldest_dirty_bucket", UNSET)
        )

        def _parse_oldest_notification(
            data: object,
        ) -> datetime.datetime | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                oldest_notification_type_0 = datetime.datetime.fromisoformat(data)

                return oldest_notification_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(datetime.datetime | None | Unset, data)

        oldest_notification = _parse_oldest_notification(
            d.pop("oldest_notification", UNSET)
        )

        summary_status = cls(
            coverage_hours=coverage_hours,
            dirty_hours=dirty_hours,
            dirty_notifications=dirty_notifications,
            kind=kind,
            covered_since=covered_since,
            covered_until=covered_until,
            latest_success=latest_success,
            oldest_dirty_bucket=oldest_dirty_bucket,
            oldest_notification=oldest_notification,
        )

        summary_status.additional_properties = d
        return summary_status

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
