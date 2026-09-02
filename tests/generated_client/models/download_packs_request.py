from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

from ..types import UNSET, Unset

T = TypeVar("T", bound="DownloadPacksRequest")


@_attrs_define
class DownloadPacksRequest:
    """Request DTO for downloading packs

    Attributes:
        packs (list[str]): List of explicit Git or archive URLs. Registry refs must use /packs/install. Example:
            ['https://github.com/attune/pack-slack.git', 'https://example.com/aws-2.0.0.tar.gz'].
        ref_spec (None | str | Unset): Git reference (branch, tag, or commit) for git sources Example: v1.0.0.
    """

    packs: list[str]
    ref_spec: None | str | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        packs = self.packs

        ref_spec: None | str | Unset
        if isinstance(self.ref_spec, Unset):
            ref_spec = UNSET
        else:
            ref_spec = self.ref_spec

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "packs": packs,
            }
        )
        if ref_spec is not UNSET:
            field_dict["ref_spec"] = ref_spec

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        d = dict(src_dict)
        packs = cast(list[str], d.pop("packs"))

        def _parse_ref_spec(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        ref_spec = _parse_ref_spec(d.pop("ref_spec", UNSET))

        download_packs_request = cls(
            packs=packs,
            ref_spec=ref_spec,
        )

        download_packs_request.additional_properties = d
        return download_packs_request

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
