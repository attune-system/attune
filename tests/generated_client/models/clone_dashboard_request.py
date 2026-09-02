from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define
from typing_extensions import Self

T = TypeVar("T", bound="CloneDashboardRequest")


@_attrs_define
class CloneDashboardRequest:
    """
    Attributes:
        ref (str):  Example: core.operations_home_copy.
    """

    ref: str

    def to_dict(self) -> dict[str, Any]:
        ref = self.ref

        field_dict: dict[str, Any] = {}

        field_dict.update(
            {
                "ref": ref,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        d = dict(src_dict)
        ref = d.pop("ref")

        clone_dashboard_request = cls(
            ref=ref,
        )

        return clone_dashboard_request
