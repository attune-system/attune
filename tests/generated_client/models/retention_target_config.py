from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="RetentionTargetConfig")


@_attrs_define
class RetentionTargetConfig:
    """Runtime database row retention settings.

    A target with `max_age_seconds: None` keeps rows forever (purging disabled).
    A target with `max_age_seconds: Some(n)` purges rows older than `n` seconds.

        Attributes:
            max_age_seconds (int | None | Unset): Maximum row age in seconds. `None` means keep forever (no purging).
    """

    max_age_seconds: int | None | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        max_age_seconds: int | None | Unset
        if isinstance(self.max_age_seconds, Unset):
            max_age_seconds = UNSET
        else:
            max_age_seconds = self.max_age_seconds

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update({})
        if max_age_seconds is not UNSET:
            field_dict["max_age_seconds"] = max_age_seconds

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)

        def _parse_max_age_seconds(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        max_age_seconds = _parse_max_age_seconds(d.pop("max_age_seconds", UNSET))

        retention_target_config = cls(
            max_age_seconds=max_age_seconds,
        )

        retention_target_config.additional_properties = d
        return retention_target_config

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
