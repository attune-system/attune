from __future__ import annotations

import datetime
from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.history_record_response_new_values import (
        HistoryRecordResponseNewValues,
    )
    from ..models.history_record_response_old_values import (
        HistoryRecordResponseOldValues,
    )


T = TypeVar("T", bound="HistoryRecordResponse")


@_attrs_define
class HistoryRecordResponse:
    """Response DTO for a single entity history record.

    Attributes:
        changed_fields (list[str]): Names of fields that changed (empty for INSERT/DELETE) Example: ['status',
            'result'].
        entity_id (int): The primary key of the changed entity Example: 42.
        new_values (HistoryRecordResponseNewValues): New values of changed fields (null for DELETE)
        old_values (HistoryRecordResponseOldValues): Previous values of changed fields (null for INSERT)
        operation (str): The operation: `INSERT`, `UPDATE`, or `DELETE` Example: UPDATE.
        time (datetime.datetime): When the change occurred Example: 2026-02-26T10:30:00Z.
        entity_ref (None | str | Unset): Denormalized human-readable identifier (e.g., action_ref, worker name) Example:
            core.http_request.
    """

    changed_fields: list[str]
    entity_id: int
    new_values: HistoryRecordResponseNewValues
    old_values: HistoryRecordResponseOldValues
    operation: str
    time: datetime.datetime
    entity_ref: None | str | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        changed_fields = self.changed_fields

        entity_id = self.entity_id

        new_values = self.new_values.to_dict()

        old_values = self.old_values.to_dict()

        operation = self.operation

        time = self.time.isoformat()

        entity_ref: None | str | Unset
        if isinstance(self.entity_ref, Unset):
            entity_ref = UNSET
        else:
            entity_ref = self.entity_ref

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "changed_fields": changed_fields,
                "entity_id": entity_id,
                "new_values": new_values,
                "old_values": old_values,
                "operation": operation,
                "time": time,
            }
        )
        if entity_ref is not UNSET:
            field_dict["entity_ref"] = entity_ref

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        from ..models.history_record_response_new_values import (
            HistoryRecordResponseNewValues,
        )
        from ..models.history_record_response_old_values import (
            HistoryRecordResponseOldValues,
        )

        d = dict(src_dict)
        changed_fields = cast(list[str], d.pop("changed_fields"))

        entity_id = d.pop("entity_id")

        new_values = HistoryRecordResponseNewValues.from_dict(d.pop("new_values"))

        old_values = HistoryRecordResponseOldValues.from_dict(d.pop("old_values"))

        operation = d.pop("operation")

        time = datetime.datetime.fromisoformat(d.pop("time"))

        def _parse_entity_ref(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        entity_ref = _parse_entity_ref(d.pop("entity_ref", UNSET))

        history_record_response = cls(
            changed_fields=changed_fields,
            entity_id=entity_id,
            new_values=new_values,
            old_values=old_values,
            operation=operation,
            time=time,
            entity_ref=entity_ref,
        )

        history_record_response.additional_properties = d
        return history_record_response

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
