from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..models.authorization_basis import AuthorizationBasis
from ..models.freshness_mode import FreshnessMode
from ..models.source_availability import SourceAvailability
from ..models.source_type import SourceType
from ..types import UNSET, Unset
from typing import cast

if TYPE_CHECKING:
  from ..models.dashboard_source_param_schema_response import DashboardSourceParamSchemaResponse





T = TypeVar("T", bound="DashboardSourceContractResponse")



@_attrs_define
class DashboardSourceContractResponse:
    """ 
        Attributes:
            authorization_basis (AuthorizationBasis):
            availability (SourceAvailability):
            default_freshness_mode (FreshnessMode):
            ordering (list[str]):
            param_schema (DashboardSourceParamSchemaResponse):
            response_shape (str):
            source_type (SourceType):
            notes (None | str | Unset):
     """

    authorization_basis: AuthorizationBasis
    availability: SourceAvailability
    default_freshness_mode: FreshnessMode
    ordering: list[str]
    param_schema: DashboardSourceParamSchemaResponse
    response_shape: str
    source_type: SourceType
    notes: None | str | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)





    def to_dict(self) -> dict[str, Any]:
        from ..models.dashboard_source_param_schema_response import DashboardSourceParamSchemaResponse
        authorization_basis = self.authorization_basis.value

        availability = self.availability.value

        default_freshness_mode = self.default_freshness_mode.value

        ordering = self.ordering



        param_schema = self.param_schema.to_dict()

        response_shape = self.response_shape

        source_type = self.source_type.value

        notes: None | str | Unset
        if isinstance(self.notes, Unset):
            notes = UNSET
        else:
            notes = self.notes


        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update({
            "authorization_basis": authorization_basis,
            "availability": availability,
            "default_freshness_mode": default_freshness_mode,
            "ordering": ordering,
            "param_schema": param_schema,
            "response_shape": response_shape,
            "source_type": source_type,
        })
        if notes is not UNSET:
            field_dict["notes"] = notes

        return field_dict



    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.dashboard_source_param_schema_response import DashboardSourceParamSchemaResponse
        d = dict(src_dict)
        authorization_basis = AuthorizationBasis(d.pop("authorization_basis"))




        availability = SourceAvailability(d.pop("availability"))




        default_freshness_mode = FreshnessMode(d.pop("default_freshness_mode"))




        ordering = cast(list[str], d.pop("ordering"))


        param_schema = DashboardSourceParamSchemaResponse.from_dict(d.pop("param_schema"))




        response_shape = d.pop("response_shape")

        source_type = SourceType(d.pop("source_type"))




        def _parse_notes(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        notes = _parse_notes(d.pop("notes", UNSET))


        dashboard_source_contract_response = cls(
            authorization_basis=authorization_basis,
            availability=availability,
            default_freshness_mode=default_freshness_mode,
            ordering=ordering,
            param_schema=param_schema,
            response_shape=response_shape,
            source_type=source_type,
            notes=notes,
        )


        dashboard_source_contract_response.additional_properties = d
        return dashboard_source_contract_response

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
