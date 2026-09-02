from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

if TYPE_CHECKING:
    from ..models.failed_pack_registration import FailedPackRegistration
    from ..models.registered_pack import RegisteredPack
    from ..models.registration_summary import RegistrationSummary


T = TypeVar("T", bound="RegisterPacksResponse")


@_attrs_define
class RegisterPacksResponse:
    """Response DTO for register packs operation

    Attributes:
        failed_packs (list[FailedPackRegistration]): Failed pack registrations
        registered_packs (list[RegisteredPack]): Successfully registered packs
        summary (RegistrationSummary): Registration summary
    """

    failed_packs: list[FailedPackRegistration]
    registered_packs: list[RegisteredPack]
    summary: RegistrationSummary
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        failed_packs = []
        for failed_packs_item_data in self.failed_packs:
            failed_packs_item = failed_packs_item_data.to_dict()
            failed_packs.append(failed_packs_item)

        registered_packs = []
        for registered_packs_item_data in self.registered_packs:
            registered_packs_item = registered_packs_item_data.to_dict()
            registered_packs.append(registered_packs_item)

        summary = self.summary.to_dict()

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "failed_packs": failed_packs,
                "registered_packs": registered_packs,
                "summary": summary,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        from ..models.failed_pack_registration import (
            FailedPackRegistration,
        )
        from ..models.registered_pack import RegisteredPack
        from ..models.registration_summary import RegistrationSummary

        d = dict(src_dict)
        failed_packs = []
        _failed_packs = d.pop("failed_packs")
        for failed_packs_item_data in _failed_packs:
            failed_packs_item = FailedPackRegistration.from_dict(failed_packs_item_data)

            failed_packs.append(failed_packs_item)

        registered_packs = []
        _registered_packs = d.pop("registered_packs")
        for registered_packs_item_data in _registered_packs:
            registered_packs_item = RegisteredPack.from_dict(registered_packs_item_data)

            registered_packs.append(registered_packs_item)

        summary = RegistrationSummary.from_dict(d.pop("summary"))

        register_packs_response = cls(
            failed_packs=failed_packs,
            registered_packs=registered_packs,
            summary=summary,
        )

        register_packs_response.additional_properties = d
        return register_packs_response

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
