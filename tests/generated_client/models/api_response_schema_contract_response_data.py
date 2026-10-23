from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

T = TypeVar("T", bound="ApiResponseSchemaContractResponseData")


@_attrs_define
class ApiResponseSchemaContractResponseData:
    """
    Attributes:
        contract (Any): Self-contained rendering graph. Preserve authored references in editors.
        semantic_digest (str):
    """

    contract: Any
    semantic_digest: str
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        contract = self.contract

        semantic_digest = self.semantic_digest

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "contract": contract,
                "semantic_digest": semantic_digest,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        d = dict(src_dict)
        contract = d.pop("contract")

        semantic_digest = d.pop("semantic_digest")

        api_response_schema_contract_response_data = cls(
            contract=contract,
            semantic_digest=semantic_digest,
        )

        api_response_schema_contract_response_data.additional_properties = d
        return api_response_schema_contract_response_data

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
