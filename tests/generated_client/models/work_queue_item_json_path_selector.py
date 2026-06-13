from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.work_queue_item_json_path_selector_vars import (
        WorkQueueItemJsonPathSelectorVars,
    )


T = TypeVar("T", bound="WorkQueueItemJsonPathSelector")


@_attrs_define
class WorkQueueItemJsonPathSelector:
    """
    Attributes:
        path (str):  Example: $.payload.customer.id ? (@ == 123).
        vars_ (WorkQueueItemJsonPathSelectorVars | Unset):
    """

    path: str
    vars_: WorkQueueItemJsonPathSelectorVars | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        path = self.path

        vars_: dict[str, Any] | Unset = UNSET
        if not isinstance(self.vars_, Unset):
            vars_ = self.vars_.to_dict()

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "path": path,
            }
        )
        if vars_ is not UNSET:
            field_dict["vars"] = vars_

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.work_queue_item_json_path_selector_vars import (
            WorkQueueItemJsonPathSelectorVars,
        )

        d = dict(src_dict)
        path = d.pop("path")

        _vars_ = d.pop("vars", UNSET)
        vars_: WorkQueueItemJsonPathSelectorVars | Unset
        if isinstance(_vars_, Unset):
            vars_ = UNSET
        else:
            vars_ = WorkQueueItemJsonPathSelectorVars.from_dict(_vars_)

        work_queue_item_json_path_selector = cls(
            path=path,
            vars_=vars_,
        )

        work_queue_item_json_path_selector.additional_properties = d
        return work_queue_item_json_path_selector

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
