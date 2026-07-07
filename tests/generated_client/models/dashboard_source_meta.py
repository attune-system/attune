from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..models.dashboard_authorization_mode import DashboardAuthorizationMode
from ..models.dashboard_freshness_mode import DashboardFreshnessMode
from ..types import UNSET, Unset
from typing import cast
import datetime

if TYPE_CHECKING:
  from ..models.dashboard_source_meta_authorized_refs_type_0 import DashboardSourceMetaAuthorizedRefsType0
  from ..models.dashboard_source_meta_unit_hints import DashboardSourceMetaUnitHints





T = TypeVar("T", bound="DashboardSourceMeta")



@_attrs_define
class DashboardSourceMeta:
    """ 
        Attributes:
            authorization_mode (DashboardAuthorizationMode):
            authorized_refs (DashboardSourceMetaAuthorizedRefsType0 | None):
            cache_hit (bool):
            freshness_mode (DashboardFreshnessMode):
            ordering (list[str]):
            truncated (bool):
            unit_hints (DashboardSourceMetaUnitHints):
            aggregate_watermark (datetime.datetime | None | Unset):
            bucket_size (None | str | Unset):
     """

    authorization_mode: DashboardAuthorizationMode
    authorized_refs: DashboardSourceMetaAuthorizedRefsType0 | None
    cache_hit: bool
    freshness_mode: DashboardFreshnessMode
    ordering: list[str]
    truncated: bool
    unit_hints: DashboardSourceMetaUnitHints
    aggregate_watermark: datetime.datetime | None | Unset = UNSET
    bucket_size: None | str | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)





    def to_dict(self) -> dict[str, Any]:
        from ..models.dashboard_source_meta_authorized_refs_type_0 import DashboardSourceMetaAuthorizedRefsType0
        from ..models.dashboard_source_meta_unit_hints import DashboardSourceMetaUnitHints
        authorization_mode = self.authorization_mode.value

        authorized_refs: dict[str, Any] | None
        if isinstance(self.authorized_refs, DashboardSourceMetaAuthorizedRefsType0):
            authorized_refs = self.authorized_refs.to_dict()
        else:
            authorized_refs = self.authorized_refs

        cache_hit = self.cache_hit

        freshness_mode = self.freshness_mode.value

        ordering = self.ordering



        truncated = self.truncated

        unit_hints = self.unit_hints.to_dict()

        aggregate_watermark: None | str | Unset
        if isinstance(self.aggregate_watermark, Unset):
            aggregate_watermark = UNSET
        elif isinstance(self.aggregate_watermark, datetime.datetime):
            aggregate_watermark = self.aggregate_watermark.isoformat()
        else:
            aggregate_watermark = self.aggregate_watermark

        bucket_size: None | str | Unset
        if isinstance(self.bucket_size, Unset):
            bucket_size = UNSET
        else:
            bucket_size = self.bucket_size


        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update({
            "authorization_mode": authorization_mode,
            "authorized_refs": authorized_refs,
            "cache_hit": cache_hit,
            "freshness_mode": freshness_mode,
            "ordering": ordering,
            "truncated": truncated,
            "unit_hints": unit_hints,
        })
        if aggregate_watermark is not UNSET:
            field_dict["aggregate_watermark"] = aggregate_watermark
        if bucket_size is not UNSET:
            field_dict["bucket_size"] = bucket_size

        return field_dict



    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.dashboard_source_meta_authorized_refs_type_0 import DashboardSourceMetaAuthorizedRefsType0
        from ..models.dashboard_source_meta_unit_hints import DashboardSourceMetaUnitHints
        d = dict(src_dict)
        authorization_mode = DashboardAuthorizationMode(d.pop("authorization_mode"))




        def _parse_authorized_refs(data: object) -> DashboardSourceMetaAuthorizedRefsType0 | None:
            if data is None:
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                authorized_refs_type_0 = DashboardSourceMetaAuthorizedRefsType0.from_dict(data)



                return authorized_refs_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(DashboardSourceMetaAuthorizedRefsType0 | None, data)

        authorized_refs = _parse_authorized_refs(d.pop("authorized_refs"))


        cache_hit = d.pop("cache_hit")

        freshness_mode = DashboardFreshnessMode(d.pop("freshness_mode"))




        ordering = cast(list[str], d.pop("ordering"))


        truncated = d.pop("truncated")

        unit_hints = DashboardSourceMetaUnitHints.from_dict(d.pop("unit_hints"))




        def _parse_aggregate_watermark(data: object) -> datetime.datetime | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                aggregate_watermark_type_0 = datetime.datetime.fromisoformat(data)



                return aggregate_watermark_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(datetime.datetime | None | Unset, data)

        aggregate_watermark = _parse_aggregate_watermark(d.pop("aggregate_watermark", UNSET))


        def _parse_bucket_size(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        bucket_size = _parse_bucket_size(d.pop("bucket_size", UNSET))


        dashboard_source_meta = cls(
            authorization_mode=authorization_mode,
            authorized_refs=authorized_refs,
            cache_hit=cache_hit,
            freshness_mode=freshness_mode,
            ordering=ordering,
            truncated=truncated,
            unit_hints=unit_hints,
            aggregate_watermark=aggregate_watermark,
            bucket_size=bucket_size,
        )


        dashboard_source_meta.additional_properties = d
        return dashboard_source_meta

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
