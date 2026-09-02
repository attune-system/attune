from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.concurrency_policy_request import ConcurrencyPolicyRequest
    from ..models.policy_scope_request import PolicyScopeRequest
    from ..models.quota_policy_request import QuotaPolicyRequest
    from ..models.rate_limit_policy_request import RateLimitPolicyRequest


T = TypeVar("T", bound="CreatePolicyRequest")


@_attrs_define
class CreatePolicyRequest:
    """
    Attributes:
        name (str):  Example: Limit echo executions.
        ref (str):  Example: core.limit_echo.
        scope (PolicyScopeRequest):
        concurrency (ConcurrencyPolicyRequest | None | Unset):
        description (None | str | Unset):  Example: Limit concurrent echo executions by customer..
        enabled (bool | Unset):  Default: True. Example: True.
        priority (int | Unset):  Default: 0. Example: 10.
        quotas (list[QuotaPolicyRequest] | Unset):
        rate_limit (None | RateLimitPolicyRequest | Unset):
        tags (list[str] | Unset):  Example: ['production'].
    """

    name: str
    ref: str
    scope: PolicyScopeRequest
    concurrency: ConcurrencyPolicyRequest | None | Unset = UNSET
    description: None | str | Unset = UNSET
    enabled: bool | Unset = True
    priority: int | Unset = 0
    quotas: list[QuotaPolicyRequest] | Unset = UNSET
    rate_limit: None | RateLimitPolicyRequest | Unset = UNSET
    tags: list[str] | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.concurrency_policy_request import (
            ConcurrencyPolicyRequest,
        )
        from ..models.rate_limit_policy_request import (
            RateLimitPolicyRequest,
        )

        name = self.name

        ref = self.ref

        scope = self.scope.to_dict()

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

        enabled = self.enabled

        priority = self.priority

        quotas: list[dict[str, Any]] | Unset = UNSET
        if not isinstance(self.quotas, Unset):
            quotas = []
            for quotas_item_data in self.quotas:
                quotas_item = quotas_item_data.to_dict()
                quotas.append(quotas_item)

        rate_limit: dict[str, Any] | None | Unset
        if isinstance(self.rate_limit, Unset):
            rate_limit = UNSET
        elif isinstance(self.rate_limit, RateLimitPolicyRequest):
            rate_limit = self.rate_limit.to_dict()
        else:
            rate_limit = self.rate_limit

        tags: list[str] | Unset = UNSET
        if not isinstance(self.tags, Unset):
            tags = self.tags

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "name": name,
                "ref": ref,
                "scope": scope,
            }
        )
        if concurrency is not UNSET:
            field_dict["concurrency"] = concurrency
        if description is not UNSET:
            field_dict["description"] = description
        if enabled is not UNSET:
            field_dict["enabled"] = enabled
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
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        from ..models.concurrency_policy_request import (
            ConcurrencyPolicyRequest,
        )
        from ..models.policy_scope_request import PolicyScopeRequest
        from ..models.quota_policy_request import QuotaPolicyRequest
        from ..models.rate_limit_policy_request import (
            RateLimitPolicyRequest,
        )

        d = dict(src_dict)
        name = d.pop("name")

        ref = d.pop("ref")

        scope = PolicyScopeRequest.from_dict(d.pop("scope"))

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

        enabled = d.pop("enabled", UNSET)

        priority = d.pop("priority", UNSET)

        _quotas = d.pop("quotas", UNSET)
        quotas: list[QuotaPolicyRequest] | Unset = UNSET
        if _quotas is not UNSET:
            quotas = []
            for quotas_item_data in _quotas:
                quotas_item = QuotaPolicyRequest.from_dict(quotas_item_data)

                quotas.append(quotas_item)

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

        tags = cast(list[str], d.pop("tags", UNSET))

        create_policy_request = cls(
            name=name,
            ref=ref,
            scope=scope,
            concurrency=concurrency,
            description=description,
            enabled=enabled,
            priority=priority,
            quotas=quotas,
            rate_limit=rate_limit,
            tags=tags,
        )

        create_policy_request.additional_properties = d
        return create_policy_request

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
