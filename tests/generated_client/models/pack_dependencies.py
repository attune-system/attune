from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from typing_extensions import Self

from ..types import UNSET, Unset

T = TypeVar("T", bound="PackDependencies")


@_attrs_define
class PackDependencies:
    """Pack dependencies

    Attributes:
        attune_version (None | str | Unset): Attune version requirement (semver)
        nodejs_version (None | str | Unset): Node.js version requirement
        packs (list[str] | Unset): Pack dependencies (format: "ref@version")
        python_version (None | str | Unset): Python version requirement
    """

    attune_version: None | str | Unset = UNSET
    nodejs_version: None | str | Unset = UNSET
    packs: list[str] | Unset = UNSET
    python_version: None | str | Unset = UNSET

    def to_dict(self) -> dict[str, Any]:
        attune_version: None | str | Unset
        if isinstance(self.attune_version, Unset):
            attune_version = UNSET
        else:
            attune_version = self.attune_version

        nodejs_version: None | str | Unset
        if isinstance(self.nodejs_version, Unset):
            nodejs_version = UNSET
        else:
            nodejs_version = self.nodejs_version

        packs: list[str] | Unset = UNSET
        if not isinstance(self.packs, Unset):
            packs = self.packs

        python_version: None | str | Unset
        if isinstance(self.python_version, Unset):
            python_version = UNSET
        else:
            python_version = self.python_version

        field_dict: dict[str, Any] = {}

        field_dict.update({})
        if attune_version is not UNSET:
            field_dict["attune_version"] = attune_version
        if nodejs_version is not UNSET:
            field_dict["nodejs_version"] = nodejs_version
        if packs is not UNSET:
            field_dict["packs"] = packs
        if python_version is not UNSET:
            field_dict["python_version"] = python_version

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        d = dict(src_dict)

        def _parse_attune_version(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        attune_version = _parse_attune_version(d.pop("attune_version", UNSET))

        def _parse_nodejs_version(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        nodejs_version = _parse_nodejs_version(d.pop("nodejs_version", UNSET))

        packs = cast(list[str], d.pop("packs", UNSET))

        def _parse_python_version(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        python_version = _parse_python_version(d.pop("python_version", UNSET))

        pack_dependencies = cls(
            attune_version=attune_version,
            nodejs_version=nodejs_version,
            packs=packs,
            python_version=python_version,
        )

        return pack_dependencies
