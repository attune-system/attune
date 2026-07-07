from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..models.dashboard_source_status import DashboardSourceStatus
from ..types import UNSET, Unset
from typing import cast

if TYPE_CHECKING:
  from ..models.dashboard_source_error import DashboardSourceError
  from ..models.dashboard_source_meta import DashboardSourceMeta
  from ..models.dashboard_source_result_data_type_0 import DashboardSourceResultDataType0





T = TypeVar("T", bound="DashboardSourceResult")



@_attrs_define
class DashboardSourceResult:
    """ 
        Attributes:
            data (DashboardSourceResultDataType0 | None):
            meta (DashboardSourceMeta):
            source_id (str):
            source_type (str):
            status (DashboardSourceStatus):
            error (DashboardSourceError | None | Unset):
     """

    data: DashboardSourceResultDataType0 | None
    meta: DashboardSourceMeta
    source_id: str
    source_type: str
    status: DashboardSourceStatus
    error: DashboardSourceError | None | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)





    def to_dict(self) -> dict[str, Any]:
        from ..models.dashboard_source_error import DashboardSourceError
        from ..models.dashboard_source_meta import DashboardSourceMeta
        from ..models.dashboard_source_result_data_type_0 import DashboardSourceResultDataType0
        data: dict[str, Any] | None
        if isinstance(self.data, DashboardSourceResultDataType0):
            data = self.data.to_dict()
        else:
            data = self.data

        meta = self.meta.to_dict()

        source_id = self.source_id

        source_type = self.source_type

        status = self.status.value

        error: dict[str, Any] | None | Unset
        if isinstance(self.error, Unset):
            error = UNSET
        elif isinstance(self.error, DashboardSourceError):
            error = self.error.to_dict()
        else:
            error = self.error


        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update({
            "data": data,
            "meta": meta,
            "source_id": source_id,
            "source_type": source_type,
            "status": status,
        })
        if error is not UNSET:
            field_dict["error"] = error

        return field_dict



    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.dashboard_source_error import DashboardSourceError
        from ..models.dashboard_source_meta import DashboardSourceMeta
        from ..models.dashboard_source_result_data_type_0 import DashboardSourceResultDataType0
        d = dict(src_dict)
        def _parse_data(data: object) -> DashboardSourceResultDataType0 | None:
            if data is None:
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                data_type_0 = DashboardSourceResultDataType0.from_dict(data)



                return data_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(DashboardSourceResultDataType0 | None, data)

        data = _parse_data(d.pop("data"))


        meta = DashboardSourceMeta.from_dict(d.pop("meta"))




        source_id = d.pop("source_id")

        source_type = d.pop("source_type")

        status = DashboardSourceStatus(d.pop("status"))




        def _parse_error(data: object) -> DashboardSourceError | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                error_type_1 = DashboardSourceError.from_dict(data)



                return error_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(DashboardSourceError | None | Unset, data)

        error = _parse_error(d.pop("error", UNSET))


        dashboard_source_result = cls(
            data=data,
            meta=meta,
            source_id=source_id,
            source_type=source_type,
            status=status,
            error=error,
        )


        dashboard_source_result.additional_properties = d
        return dashboard_source_result

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
