from __future__ import annotations

import datetime
from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

from ..types import UNSET, Unset

T = TypeVar("T", bound="RetiredPackComponentResponse")


@_attrs_define
class RetiredPackComponentResponse:
    """A component removed from the active projection of an installed pack.

    Attributes:
        id (int):
        kind (str):
        retired_at (datetime.datetime):
        component_ref (None | str | Unset):
        managed_release (int | None | Unset):
    """

    id: int
    kind: str
    retired_at: datetime.datetime
    component_ref: None | str | Unset = UNSET
    managed_release: int | None | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        id = self.id

        kind = self.kind

        retired_at = self.retired_at.isoformat()

        component_ref: None | str | Unset
        if isinstance(self.component_ref, Unset):
            component_ref = UNSET
        else:
            component_ref = self.component_ref

        managed_release: int | None | Unset
        if isinstance(self.managed_release, Unset):
            managed_release = UNSET
        else:
            managed_release = self.managed_release

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "id": id,
                "kind": kind,
                "retired_at": retired_at,
            }
        )
        if component_ref is not UNSET:
            field_dict["component_ref"] = component_ref
        if managed_release is not UNSET:
            field_dict["managed_release"] = managed_release

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        d = dict(src_dict)
        id = d.pop("id")

        kind = d.pop("kind")

        retired_at = datetime.datetime.fromisoformat(d.pop("retired_at"))

        def _parse_component_ref(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        component_ref = _parse_component_ref(d.pop("component_ref", UNSET))

        def _parse_managed_release(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        managed_release = _parse_managed_release(d.pop("managed_release", UNSET))

        retired_pack_component_response = cls(
            id=id,
            kind=kind,
            retired_at=retired_at,
            component_ref=component_ref,
            managed_release=managed_release,
        )

        retired_pack_component_response.additional_properties = d
        return retired_pack_component_response

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
