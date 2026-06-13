from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.taint_effect import TaintEffect
from ..models.toleration_operator import TolerationOperator
from ..types import UNSET, Unset

T = TypeVar("T", bound="WorkerToleration")


@_attrs_define
class WorkerToleration:
    """
    Attributes:
        key (str):
        effect (None | TaintEffect | Unset):
        operator (TolerationOperator | Unset):
        value (None | str | Unset):
    """

    key: str
    effect: None | TaintEffect | Unset = UNSET
    operator: TolerationOperator | Unset = UNSET
    value: None | str | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        key = self.key

        effect: None | str | Unset
        if isinstance(self.effect, Unset):
            effect = UNSET
        elif isinstance(self.effect, TaintEffect):
            effect = self.effect.value
        else:
            effect = self.effect

        operator: str | Unset = UNSET
        if not isinstance(self.operator, Unset):
            operator = self.operator.value

        value: None | str | Unset
        if isinstance(self.value, Unset):
            value = UNSET
        else:
            value = self.value

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "key": key,
            }
        )
        if effect is not UNSET:
            field_dict["effect"] = effect
        if operator is not UNSET:
            field_dict["operator"] = operator
        if value is not UNSET:
            field_dict["value"] = value

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        key = d.pop("key")

        def _parse_effect(data: object) -> None | TaintEffect | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                effect_type_1 = TaintEffect(data)

                return effect_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(None | TaintEffect | Unset, data)

        effect = _parse_effect(d.pop("effect", UNSET))

        _operator = d.pop("operator", UNSET)
        operator: TolerationOperator | Unset
        if isinstance(_operator, Unset):
            operator = UNSET
        else:
            operator = TolerationOperator(_operator)

        def _parse_value(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        value = _parse_value(d.pop("value", UNSET))

        worker_toleration = cls(
            key=key,
            effect=effect,
            operator=operator,
            value=value,
        )

        worker_toleration.additional_properties = d
        return worker_toleration

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
