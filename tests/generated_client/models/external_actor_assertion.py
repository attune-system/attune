from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define
from typing_extensions import Self

T = TypeVar("T", bound="ExternalActorAssertion")


@_attrs_define
class ExternalActorAssertion:
    """External actor asserted by an authenticated integration adapter.

    Attributes:
        external_subject (str):
        provider (str):
        tenant (str):
    """

    external_subject: str
    provider: str
    tenant: str

    def to_dict(self) -> dict[str, Any]:
        external_subject = self.external_subject

        provider = self.provider

        tenant = self.tenant

        field_dict: dict[str, Any] = {}

        field_dict.update(
            {
                "external_subject": external_subject,
                "provider": provider,
                "tenant": tenant,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        d = dict(src_dict)
        external_subject = d.pop("external_subject")

        provider = d.pop("provider")

        tenant = d.pop("tenant")

        external_actor_assertion = cls(
            external_subject=external_subject,
            provider=provider,
            tenant=tenant,
        )

        return external_actor_assertion
