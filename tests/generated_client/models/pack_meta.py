from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

from ..types import UNSET, Unset

T = TypeVar("T", bound="PackMeta")


@_attrs_define
class PackMeta:
    """Additional pack metadata

    Attributes:
        downloads (int | None | Unset): Download count
        stars (int | None | Unset): Star/rating count
        tested_attune_versions (list[str] | Unset): Tested Attune versions
    """

    downloads: int | None | Unset = UNSET
    stars: int | None | Unset = UNSET
    tested_attune_versions: list[str] | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        downloads: int | None | Unset
        if isinstance(self.downloads, Unset):
            downloads = UNSET
        else:
            downloads = self.downloads

        stars: int | None | Unset
        if isinstance(self.stars, Unset):
            stars = UNSET
        else:
            stars = self.stars

        tested_attune_versions: list[str] | Unset = UNSET
        if not isinstance(self.tested_attune_versions, Unset):
            tested_attune_versions = self.tested_attune_versions

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update({})
        if downloads is not UNSET:
            field_dict["downloads"] = downloads
        if stars is not UNSET:
            field_dict["stars"] = stars
        if tested_attune_versions is not UNSET:
            field_dict["tested_attune_versions"] = tested_attune_versions

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        d = dict(src_dict)

        def _parse_downloads(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        downloads = _parse_downloads(d.pop("downloads", UNSET))

        def _parse_stars(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        stars = _parse_stars(d.pop("stars", UNSET))

        tested_attune_versions = cast(list[str], d.pop("tested_attune_versions", UNSET))

        pack_meta = cls(
            downloads=downloads,
            stars=stars,
            tested_attune_versions=tested_attune_versions,
        )

        pack_meta.additional_properties = d
        return pack_meta

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
