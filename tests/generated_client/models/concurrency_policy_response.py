from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

from ..models.policy_method import PolicyMethod

T = TypeVar("T", bound="ConcurrencyPolicyResponse")


@_attrs_define
class ConcurrencyPolicyResponse:
    """
    Attributes:
        limit (int):  Example: 5.
        method (PolicyMethod):
        parameters (list[str]):  Example: ['customer_id'].
    """

    limit: int
    method: PolicyMethod
    parameters: list[str]
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        limit = self.limit

        method = self.method.value

        parameters = self.parameters

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "limit": limit,
                "method": method,
                "parameters": parameters,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        d = dict(src_dict)
        limit = d.pop("limit")

        method = PolicyMethod(d.pop("method"))

        parameters = cast(list[str], d.pop("parameters"))

        concurrency_policy_response = cls(
            limit=limit,
            method=method,
            parameters=parameters,
        )

        concurrency_policy_response.additional_properties = d
        return concurrency_policy_response

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
