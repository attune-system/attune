from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

from ..types import UNSET, Unset

T = TypeVar("T", bound="SchemaTypeResponse")


@_attrs_define
class SchemaTypeResponse:
    """
    Attributes:
        id (int):
        label (str):
        pack_ref (str):
        ref (str):
        reference_allowed_pack_refs (list[str]):
        reference_visibility (str): Pack-reference policy, not a restriction on authenticated inspection or
            Key schema assignment.
        schema (Any):
        description (None | str | Unset):
        managed_release (int | None | Unset):
    """

    id: int
    label: str
    pack_ref: str
    ref: str
    reference_allowed_pack_refs: list[str]
    reference_visibility: str
    schema: Any
    description: None | str | Unset = UNSET
    managed_release: int | None | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        id = self.id

        label = self.label

        pack_ref = self.pack_ref

        ref = self.ref

        reference_allowed_pack_refs = self.reference_allowed_pack_refs

        reference_visibility = self.reference_visibility

        schema = self.schema

        description: None | str | Unset
        if isinstance(self.description, Unset):
            description = UNSET
        else:
            description = self.description

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
                "label": label,
                "pack_ref": pack_ref,
                "ref": ref,
                "reference_allowed_pack_refs": reference_allowed_pack_refs,
                "reference_visibility": reference_visibility,
                "schema": schema,
            }
        )
        if description is not UNSET:
            field_dict["description"] = description
        if managed_release is not UNSET:
            field_dict["managed_release"] = managed_release

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        d = dict(src_dict)
        id = d.pop("id")

        label = d.pop("label")

        pack_ref = d.pop("pack_ref")

        ref = d.pop("ref")

        reference_allowed_pack_refs = cast(
            list[str], d.pop("reference_allowed_pack_refs")
        )

        reference_visibility = d.pop("reference_visibility")

        schema = d.pop("schema")

        def _parse_description(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        description = _parse_description(d.pop("description", UNSET))

        def _parse_managed_release(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        managed_release = _parse_managed_release(d.pop("managed_release", UNSET))

        schema_type_response = cls(
            id=id,
            label=label,
            pack_ref=pack_ref,
            ref=ref,
            reference_allowed_pack_refs=reference_allowed_pack_refs,
            reference_visibility=reference_visibility,
            schema=schema,
            description=description,
            managed_release=managed_release,
        )

        schema_type_response.additional_properties = d
        return schema_type_response

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
