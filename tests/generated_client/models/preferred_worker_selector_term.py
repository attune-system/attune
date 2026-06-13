from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.worker_selector_term import WorkerSelectorTerm


T = TypeVar("T", bound="PreferredWorkerSelectorTerm")


@_attrs_define
class PreferredWorkerSelectorTerm:
    """
    Attributes:
        preference (WorkerSelectorTerm):
        weight (int | Unset):
    """

    preference: WorkerSelectorTerm
    weight: int | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        preference = self.preference.to_dict()

        weight = self.weight

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "preference": preference,
            }
        )
        if weight is not UNSET:
            field_dict["weight"] = weight

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.worker_selector_term import WorkerSelectorTerm

        d = dict(src_dict)
        preference = WorkerSelectorTerm.from_dict(d.pop("preference"))

        weight = d.pop("weight", UNSET)

        preferred_worker_selector_term = cls(
            preference=preference,
            weight=weight,
        )

        preferred_worker_selector_term.additional_properties = d
        return preferred_worker_selector_term

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
