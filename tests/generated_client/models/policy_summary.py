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
  from ..models.concurrency_policy_response import ConcurrencyPolicyResponse
  from ..models.policy_scope_response import PolicyScopeResponse
  from ..models.quota_policy_response import QuotaPolicyResponse
  from ..models.rate_limit_policy_response import RateLimitPolicyResponse





T = TypeVar("T", bound="PolicySummary")



@_attrs_define
class PolicySummary:
    """ 
        Attributes:
            created (datetime.datetime):
            enabled (bool):
            id (int):
            name (str):
            priority (int):
            quotas (list[QuotaPolicyResponse]):
            ref (str):
            scope (PolicyScopeResponse):
            tags (list[str]):
            updated (datetime.datetime):
            concurrency (ConcurrencyPolicyResponse | None | Unset):
            description (None | str | Unset):
            rate_limit (None | RateLimitPolicyResponse | Unset):
     """

    created: datetime.datetime
    enabled: bool
    id: int
    name: str
    priority: int
    quotas: list[QuotaPolicyResponse]
    ref: str
    scope: PolicyScopeResponse
    tags: list[str]
    updated: datetime.datetime
    concurrency: ConcurrencyPolicyResponse | None | Unset = UNSET
    description: None | str | Unset = UNSET
    rate_limit: None | RateLimitPolicyResponse | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)





    def to_dict(self) -> dict[str, Any]:
        from ..models.concurrency_policy_response import ConcurrencyPolicyResponse
        from ..models.policy_scope_response import PolicyScopeResponse
        from ..models.quota_policy_response import QuotaPolicyResponse
        from ..models.rate_limit_policy_response import RateLimitPolicyResponse
        created = self.created.isoformat()

        enabled = self.enabled

        id = self.id

        name = self.name

        priority = self.priority

        quotas = []
        for quotas_item_data in self.quotas:
            quotas_item = quotas_item_data.to_dict()
            quotas.append(quotas_item)



        ref = self.ref

        scope = self.scope.to_dict()

        tags = self.tags



        updated = self.updated.isoformat()

        concurrency: dict[str, Any] | None | Unset
        if isinstance(self.concurrency, Unset):
            concurrency = UNSET
        elif isinstance(self.concurrency, ConcurrencyPolicyResponse):
            concurrency = self.concurrency.to_dict()
        else:
            concurrency = self.concurrency

        description: None | str | Unset
        if isinstance(self.description, Unset):
            description = UNSET
        else:
            description = self.description

        rate_limit: dict[str, Any] | None | Unset
        if isinstance(self.rate_limit, Unset):
            rate_limit = UNSET
        elif isinstance(self.rate_limit, RateLimitPolicyResponse):
            rate_limit = self.rate_limit.to_dict()
        else:
            rate_limit = self.rate_limit


        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update({
            "created": created,
            "enabled": enabled,
            "id": id,
            "name": name,
            "priority": priority,
            "quotas": quotas,
            "ref": ref,
            "scope": scope,
            "tags": tags,
            "updated": updated,
        })
        if concurrency is not UNSET:
            field_dict["concurrency"] = concurrency
        if description is not UNSET:
            field_dict["description"] = description
        if rate_limit is not UNSET:
            field_dict["rate_limit"] = rate_limit

        return field_dict



    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.concurrency_policy_response import ConcurrencyPolicyResponse
        from ..models.policy_scope_response import PolicyScopeResponse
        from ..models.quota_policy_response import QuotaPolicyResponse
        from ..models.rate_limit_policy_response import RateLimitPolicyResponse
        d = dict(src_dict)
        created = datetime.datetime.fromisoformat(d.pop("created"))




        enabled = d.pop("enabled")

        id = d.pop("id")

        name = d.pop("name")

        priority = d.pop("priority")

        quotas = []
        _quotas = d.pop("quotas")
        for quotas_item_data in (_quotas):
            quotas_item = QuotaPolicyResponse.from_dict(quotas_item_data)



            quotas.append(quotas_item)


        ref = d.pop("ref")

        scope = PolicyScopeResponse.from_dict(d.pop("scope"))




        tags = cast(list[str], d.pop("tags"))


        updated = datetime.datetime.fromisoformat(d.pop("updated"))




        def _parse_concurrency(data: object) -> ConcurrencyPolicyResponse | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                concurrency_type_1 = ConcurrencyPolicyResponse.from_dict(data)



                return concurrency_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(ConcurrencyPolicyResponse | None | Unset, data)

        concurrency = _parse_concurrency(d.pop("concurrency", UNSET))


        def _parse_description(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        description = _parse_description(d.pop("description", UNSET))


        def _parse_rate_limit(data: object) -> None | RateLimitPolicyResponse | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                rate_limit_type_1 = RateLimitPolicyResponse.from_dict(data)



                return rate_limit_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(None | RateLimitPolicyResponse | Unset, data)

        rate_limit = _parse_rate_limit(d.pop("rate_limit", UNSET))


        policy_summary = cls(
            created=created,
            enabled=enabled,
            id=id,
            name=name,
            priority=priority,
            quotas=quotas,
            ref=ref,
            scope=scope,
            tags=tags,
            updated=updated,
            concurrency=concurrency,
            description=description,
            rate_limit=rate_limit,
        )


        policy_summary.additional_properties = d
        return policy_summary

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
