from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.preferred_worker_selector_term import PreferredWorkerSelectorTerm
    from ..models.worker_selector_term import WorkerSelectorTerm


T = TypeVar("T", bound="WorkerAffinity")


@_attrs_define
class WorkerAffinity:
    """
    Attributes:
        anti_affinity (list[WorkerSelectorTerm] | Unset):
        preferred (list[PreferredWorkerSelectorTerm] | Unset):
        required (list[WorkerSelectorTerm] | Unset):
    """

    anti_affinity: list[WorkerSelectorTerm] | Unset = UNSET
    preferred: list[PreferredWorkerSelectorTerm] | Unset = UNSET
    required: list[WorkerSelectorTerm] | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        anti_affinity: list[dict[str, Any]] | Unset = UNSET
        if not isinstance(self.anti_affinity, Unset):
            anti_affinity = []
            for anti_affinity_item_data in self.anti_affinity:
                anti_affinity_item = anti_affinity_item_data.to_dict()
                anti_affinity.append(anti_affinity_item)

        preferred: list[dict[str, Any]] | Unset = UNSET
        if not isinstance(self.preferred, Unset):
            preferred = []
            for preferred_item_data in self.preferred:
                preferred_item = preferred_item_data.to_dict()
                preferred.append(preferred_item)

        required: list[dict[str, Any]] | Unset = UNSET
        if not isinstance(self.required, Unset):
            required = []
            for required_item_data in self.required:
                required_item = required_item_data.to_dict()
                required.append(required_item)

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update({})
        if anti_affinity is not UNSET:
            field_dict["anti_affinity"] = anti_affinity
        if preferred is not UNSET:
            field_dict["preferred"] = preferred
        if required is not UNSET:
            field_dict["required"] = required

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.preferred_worker_selector_term import PreferredWorkerSelectorTerm
        from ..models.worker_selector_term import WorkerSelectorTerm

        d = dict(src_dict)
        _anti_affinity = d.pop("anti_affinity", UNSET)
        anti_affinity: list[WorkerSelectorTerm] | Unset = UNSET
        if _anti_affinity is not UNSET:
            anti_affinity = []
            for anti_affinity_item_data in _anti_affinity:
                anti_affinity_item = WorkerSelectorTerm.from_dict(
                    anti_affinity_item_data
                )

                anti_affinity.append(anti_affinity_item)

        _preferred = d.pop("preferred", UNSET)
        preferred: list[PreferredWorkerSelectorTerm] | Unset = UNSET
        if _preferred is not UNSET:
            preferred = []
            for preferred_item_data in _preferred:
                preferred_item = PreferredWorkerSelectorTerm.from_dict(
                    preferred_item_data
                )

                preferred.append(preferred_item)

        _required = d.pop("required", UNSET)
        required: list[WorkerSelectorTerm] | Unset = UNSET
        if _required is not UNSET:
            required = []
            for required_item_data in _required:
                required_item = WorkerSelectorTerm.from_dict(required_item_data)

                required.append(required_item)

        worker_affinity = cls(
            anti_affinity=anti_affinity,
            preferred=preferred,
            required=required,
        )

        worker_affinity.additional_properties = d
        return worker_affinity

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
