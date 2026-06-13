from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.log_retention_policy_patch_type_0_op import LogRetentionPolicyPatchType0Op
from ..models.retention_policy_type import RetentionPolicyType

T = TypeVar("T", bound="LogRetentionPolicyPatchType0")


@_attrs_define
class LogRetentionPolicyPatchType0:
    """
    Attributes:
        op (LogRetentionPolicyPatchType0Op):
        value (RetentionPolicyType):
    """

    op: LogRetentionPolicyPatchType0Op
    value: RetentionPolicyType
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        op = self.op.value

        value = self.value.value

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "op": op,
                "value": value,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        op = LogRetentionPolicyPatchType0Op(d.pop("op"))

        value = RetentionPolicyType(d.pop("value"))

        log_retention_policy_patch_type_0 = cls(
            op=op,
            value=value,
        )

        log_retention_policy_patch_type_0.additional_properties = d
        return log_retention_policy_patch_type_0

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
