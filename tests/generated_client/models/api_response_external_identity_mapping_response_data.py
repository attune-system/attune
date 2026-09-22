from __future__ import annotations

import datetime
from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

from ..types import UNSET, Unset

T = TypeVar("T", bound="ApiResponseExternalIdentityMappingResponseData")


@_attrs_define
class ApiResponseExternalIdentityMappingResponseData:
    """
    Attributes:
        created (datetime.datetime):
        external_subject (str):
        id (int):
        integration_identity (int):
        mapped_identity (int):
        provider (str):
        subject_kind (str):
        tenant (str):
        updated (datetime.datetime):
        created_by (int | None | Unset):
    """

    created: datetime.datetime
    external_subject: str
    id: int
    integration_identity: int
    mapped_identity: int
    provider: str
    subject_kind: str
    tenant: str
    updated: datetime.datetime
    created_by: int | None | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        created = self.created.isoformat()

        external_subject = self.external_subject

        id = self.id

        integration_identity = self.integration_identity

        mapped_identity = self.mapped_identity

        provider = self.provider

        subject_kind = self.subject_kind

        tenant = self.tenant

        updated = self.updated.isoformat()

        created_by: int | None | Unset
        if isinstance(self.created_by, Unset):
            created_by = UNSET
        else:
            created_by = self.created_by

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "created": created,
                "external_subject": external_subject,
                "id": id,
                "integration_identity": integration_identity,
                "mapped_identity": mapped_identity,
                "provider": provider,
                "subject_kind": subject_kind,
                "tenant": tenant,
                "updated": updated,
            }
        )
        if created_by is not UNSET:
            field_dict["created_by"] = created_by

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        d = dict(src_dict)
        created = datetime.datetime.fromisoformat(d.pop("created"))

        external_subject = d.pop("external_subject")

        id = d.pop("id")

        integration_identity = d.pop("integration_identity")

        mapped_identity = d.pop("mapped_identity")

        provider = d.pop("provider")

        subject_kind = d.pop("subject_kind")

        tenant = d.pop("tenant")

        updated = datetime.datetime.fromisoformat(d.pop("updated"))

        def _parse_created_by(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        created_by = _parse_created_by(d.pop("created_by", UNSET))

        api_response_external_identity_mapping_response_data = cls(
            created=created,
            external_subject=external_subject,
            id=id,
            integration_identity=integration_identity,
            mapped_identity=mapped_identity,
            provider=provider,
            subject_kind=subject_kind,
            tenant=tenant,
            updated=updated,
            created_by=created_by,
        )

        api_response_external_identity_mapping_response_data.additional_properties = d
        return api_response_external_identity_mapping_response_data

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
