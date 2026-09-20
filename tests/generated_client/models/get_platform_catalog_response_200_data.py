from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

from ..models.platform_catalog_status import PlatformCatalogStatus

T = TypeVar("T", bound="GetPlatformCatalogResponse200Data")


@_attrs_define
class GetPlatformCatalogResponse200Data:
    """
    Attributes:
        compatibility_epoch (int):
        expected_compatibility_epoch (int):
        expected_revision (int):
        revision (int):
        status (PlatformCatalogStatus): Compatibility between the database catalog and this API build.
    """

    compatibility_epoch: int
    expected_compatibility_epoch: int
    expected_revision: int
    revision: int
    status: PlatformCatalogStatus
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        compatibility_epoch = self.compatibility_epoch

        expected_compatibility_epoch = self.expected_compatibility_epoch

        expected_revision = self.expected_revision

        revision = self.revision

        status = self.status.value

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "compatibility_epoch": compatibility_epoch,
                "expected_compatibility_epoch": expected_compatibility_epoch,
                "expected_revision": expected_revision,
                "revision": revision,
                "status": status,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        d = dict(src_dict)
        compatibility_epoch = d.pop("compatibility_epoch")

        expected_compatibility_epoch = d.pop("expected_compatibility_epoch")

        expected_revision = d.pop("expected_revision")

        revision = d.pop("revision")

        status = PlatformCatalogStatus(d.pop("status"))

        get_platform_catalog_response_200_data = cls(
            compatibility_epoch=compatibility_epoch,
            expected_compatibility_epoch=expected_compatibility_epoch,
            expected_revision=expected_revision,
            revision=revision,
            status=status,
        )

        get_platform_catalog_response_200_data.additional_properties = d
        return get_platform_catalog_response_200_data

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
