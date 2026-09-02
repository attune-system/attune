from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

if TYPE_CHECKING:
    from ..models.dashboard_source_contract_response import (
        DashboardSourceContractResponse,
    )


T = TypeVar("T", bound="DashboardSourceCatalogResponse")


@_attrs_define
class DashboardSourceCatalogResponse:
    """
    Attributes:
        contracts (list[DashboardSourceContractResponse]):
        source (str):
    """

    contracts: list[DashboardSourceContractResponse]
    source: str
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        contracts = []
        for contracts_item_data in self.contracts:
            contracts_item = contracts_item_data.to_dict()
            contracts.append(contracts_item)

        source = self.source

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "contracts": contracts,
                "source": source,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        from ..models.dashboard_source_contract_response import (
            DashboardSourceContractResponse,
        )

        d = dict(src_dict)
        contracts = []
        _contracts = d.pop("contracts")
        for contracts_item_data in _contracts:
            contracts_item = DashboardSourceContractResponse.from_dict(
                contracts_item_data
            )

            contracts.append(contracts_item)

        source = d.pop("source")

        dashboard_source_catalog_response = cls(
            contracts=contracts,
            source=source,
        )

        dashboard_source_catalog_response.additional_properties = d
        return dashboard_source_catalog_response

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
