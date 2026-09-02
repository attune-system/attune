from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

from ..types import UNSET, Unset

T = TypeVar("T", bound="DownloadedPack")


@_attrs_define
class DownloadedPack:
    """Information about a downloaded pack

    Attributes:
        pack_path (str): Local path to downloaded pack
        pack_ref (str): Pack reference from pack.yaml
        pack_version (str): Pack version from pack.yaml
        source (str): Original source
        source_type (str): Source type (git, http, registry)
        checksum (None | str | Unset): Directory checksum
        git_commit (None | str | Unset): Git commit hash (for git sources)
    """

    pack_path: str
    pack_ref: str
    pack_version: str
    source: str
    source_type: str
    checksum: None | str | Unset = UNSET
    git_commit: None | str | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        pack_path = self.pack_path

        pack_ref = self.pack_ref

        pack_version = self.pack_version

        source = self.source

        source_type = self.source_type

        checksum: None | str | Unset
        if isinstance(self.checksum, Unset):
            checksum = UNSET
        else:
            checksum = self.checksum

        git_commit: None | str | Unset
        if isinstance(self.git_commit, Unset):
            git_commit = UNSET
        else:
            git_commit = self.git_commit

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "pack_path": pack_path,
                "pack_ref": pack_ref,
                "pack_version": pack_version,
                "source": source,
                "source_type": source_type,
            }
        )
        if checksum is not UNSET:
            field_dict["checksum"] = checksum
        if git_commit is not UNSET:
            field_dict["git_commit"] = git_commit

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        d = dict(src_dict)
        pack_path = d.pop("pack_path")

        pack_ref = d.pop("pack_ref")

        pack_version = d.pop("pack_version")

        source = d.pop("source")

        source_type = d.pop("source_type")

        def _parse_checksum(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        checksum = _parse_checksum(d.pop("checksum", UNSET))

        def _parse_git_commit(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        git_commit = _parse_git_commit(d.pop("git_commit", UNSET))

        downloaded_pack = cls(
            pack_path=pack_path,
            pack_ref=pack_ref,
            pack_version=pack_version,
            source=source,
            source_type=source_type,
            checksum=checksum,
            git_commit=git_commit,
        )

        downloaded_pack.additional_properties = d
        return downloaded_pack

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
