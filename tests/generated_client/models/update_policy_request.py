from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..types import UNSET, Unset
from typing import cast

if TYPE_CHECKING:
  from ..models.concurrency_policy_request import ConcurrencyPolicyRequest
  from ..models.quota_policy_request import QuotaPolicyRequest
  from ..models.rate_limit_policy_request import RateLimitPolicyRequest





T = TypeVar("T", bound="UpdatePolicyRequest")



@_attrs_define
class UpdatePolicyRequest:
    """ 
        Attributes:
            concurrency (ConcurrencyPolicyRequest | None | Unset):
            description (None | str | Unset):  Example: Limit concurrent echo executions by customer..
            enabled (bool | None | Unset):  Example: True.
            name (None | str | Unset):  Example: Limit echo executions.
            priority (int | None | Unset):  Example: 10.
            quotas (list[QuotaPolicyRequest] | None | Unset):
            rate_limit (None | RateLimitPolicyRequest | Unset):
            tags (list[str] | None | Unset):
     """

    concurrency: ConcurrencyPolicyRequest | None | Unset = UNSET
    description: None | str | Unset = UNSET
    enabled: bool | None | Unset = UNSET
    name: None | str | Unset = UNSET
    priority: int | None | Unset = UNSET
    quotas: list[QuotaPolicyRequest] | None | Unset = UNSET
    rate_limit: None | RateLimitPolicyRequest | Unset = UNSET
    tags: list[str] | None | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)





    def to_dict(self) -> dict[str, Any]:
        from ..models.concurrency_policy_request import ConcurrencyPolicyRequest
        from ..models.quota_policy_request import QuotaPolicyRequest
        from ..models.rate_limit_policy_request import RateLimitPolicyRequest
        concurrency: dict[str, Any] | None | Unset
        if isinstance(self.concurrency, Unset):
            concurrency = UNSET
        elif isinstance(self.concurrency, ConcurrencyPolicyRequest):
            concurrency = self.concurrency.to_dict()
        else:
            concurrency = self.concurrency

        description: None | str | Unset
        if isinstance(self.description, Unset):
            description = UNSET
        else:
            description = self.description

        enabled: bool | None | Unset
        if isinstance(self.enabled, Unset):
            enabled = UNSET
        else:
            enabled = self.enabled

        name: None | str | Unset
        if isinstance(self.name, Unset):
            name = UNSET
        else:
            name = self.name

        priority: int | None | Unset
        if isinstance(self.priority, Unset):
            priority = UNSET
        else:
            priority = self.priority

        quotas: list[dict[str, Any]] | None | Unset
        if isinstance(self.quotas, Unset):
            quotas = UNSET
        elif isinstance(self.quotas, list):
            quotas = []
            for quotas_type_0_item_data in self.quotas:
                quotas_type_0_item = quotas_type_0_item_data.to_dict()
                quotas.append(quotas_type_0_item)


        else:
            quotas = self.quotas

        rate_limit: dict[str, Any] | None | Unset
        if isinstance(self.rate_limit, Unset):
            rate_limit = UNSET
        elif isinstance(self.rate_limit, RateLimitPolicyRequest):
            rate_limit = self.rate_limit.to_dict()
        else:
            rate_limit = self.rate_limit

        tags: list[str] | None | Unset
        if isinstance(self.tags, Unset):
            tags = UNSET
        elif isinstance(self.tags, list):
            tags = self.tags


        else:
            tags = self.tags


        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update({
        })
        if concurrency is not UNSET:
            field_dict["concurrency"] = concurrency
        if description is not UNSET:
            field_dict["description"] = description
        if enabled is not UNSET:
            field_dict["enabled"] = enabled
        if name is not UNSET:
            field_dict["name"] = name
        if priority is not UNSET:
            field_dict["priority"] = priority
        if quotas is not UNSET:
            field_dict["quotas"] = quotas
        if rate_limit is not UNSET:
            field_dict["rate_limit"] = rate_limit
        if tags is not UNSET:
            field_dict["tags"] = tags

        return field_dict



    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.concurrency_policy_request import ConcurrencyPolicyRequest
        from ..models.quota_policy_request import QuotaPolicyRequest
        from ..models.rate_limit_policy_request import RateLimitPolicyRequest
        d = dict(src_dict)
        def _parse_concurrency(data: object) -> ConcurrencyPolicyRequest | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                concurrency_type_1 = ConcurrencyPolicyRequest.from_dict(data)



                return concurrency_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(ConcurrencyPolicyRequest | None | Unset, data)

        concurrency = _parse_concurrency(d.pop("concurrency", UNSET))


        def _parse_description(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        description = _parse_description(d.pop("description", UNSET))


        def _parse_enabled(data: object) -> bool | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(bool | None | Unset, data)

        enabled = _parse_enabled(d.pop("enabled", UNSET))


        def _parse_name(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        name = _parse_name(d.pop("name", UNSET))


        def _parse_priority(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        priority = _parse_priority(d.pop("priority", UNSET))


        def _parse_quotas(data: object) -> list[QuotaPolicyRequest] | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, list):
                    raise TypeError()
                quotas_type_0 = []
                _quotas_type_0 = data
                for quotas_type_0_item_data in (_quotas_type_0):
                    quotas_type_0_item = QuotaPolicyRequest.from_dict(quotas_type_0_item_data)



                    quotas_type_0.append(quotas_type_0_item)

                return quotas_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(list[QuotaPolicyRequest] | None | Unset, data)

        quotas = _parse_quotas(d.pop("quotas", UNSET))


        def _parse_rate_limit(data: object) -> None | RateLimitPolicyRequest | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                rate_limit_type_1 = RateLimitPolicyRequest.from_dict(data)



                return rate_limit_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(None | RateLimitPolicyRequest | Unset, data)

        rate_limit = _parse_rate_limit(d.pop("rate_limit", UNSET))


        def _parse_tags(data: object) -> list[str] | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, list):
                    raise TypeError()
                tags_type_0 = cast(list[str], data)

                return tags_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(list[str] | None | Unset, data)

        tags = _parse_tags(d.pop("tags", UNSET))


        update_policy_request = cls(
            concurrency=concurrency,
            description=description,
            enabled=enabled,
            name=name,
            priority=priority,
            quotas=quotas,
            rate_limit=rate_limit,
            tags=tags,
        )


        update_policy_request.additional_properties = d
        return update_policy_request

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
