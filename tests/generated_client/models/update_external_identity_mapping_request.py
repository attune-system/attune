from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define
from typing_extensions import Self

T = TypeVar("T", bound="UpdateExternalIdentityMappingRequest")


@_attrs_define
class UpdateExternalIdentityMappingRequest:
    """
    Attributes:
        external_subject (str):
        mapped_identity (int):
        provider (str):
        tenant (str):
    """

    external_subject: str
    mapped_identity: int
    provider: str
    tenant: str

    def to_dict(self) -> dict[str, Any]:
        external_subject = self.external_subject

        mapped_identity = self.mapped_identity

        provider = self.provider

        tenant = self.tenant

        field_dict: dict[str, Any] = {}

        field_dict.update(
            {
                "external_subject": external_subject,
                "mapped_identity": mapped_identity,
                "provider": provider,
                "tenant": tenant,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        d = dict(src_dict)
        external_subject = d.pop("external_subject")

        mapped_identity = d.pop("mapped_identity")

        provider = d.pop("provider")

        tenant = d.pop("tenant")

        update_external_identity_mapping_request = cls(
            external_subject=external_subject,
            mapped_identity=mapped_identity,
            provider=provider,
            tenant=tenant,
        )

        return update_external_identity_mapping_request
