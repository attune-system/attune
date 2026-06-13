from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.trigger_string_patch_type_1_op import TriggerStringPatchType1Op

T = TypeVar("T", bound="TriggerStringPatchType1")


@_attrs_define
class TriggerStringPatchType1:
    """
    Attributes:
        op (TriggerStringPatchType1Op):
    """

    op: TriggerStringPatchType1Op
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        op = self.op.value

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "op": op,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        op = TriggerStringPatchType1Op(d.pop("op"))

        trigger_string_patch_type_1 = cls(
            op=op,
        )

        trigger_string_patch_type_1.additional_properties = d
        return trigger_string_patch_type_1

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
