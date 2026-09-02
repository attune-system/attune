from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

from ..types import UNSET, Unset

T = TypeVar("T", bound="BuildPackEnvsRequest")


@_attrs_define
class BuildPackEnvsRequest:
    """Request DTO for building pack environments

    Attributes:
        pack_paths (list[str]): List of pack directory paths Example: ['/tmp/attune-packs/slack'].
        force_rebuild (bool | Unset): Force rebuild of existing environments Example: False.
        nodejs_version (str | Unset): Node.js version to use Example: 20.
        packs_base_dir (None | str | Unset): Base directory for permanent pack storage Example: /opt/attune/packs.
        python_version (str | Unset): Python version to use Example: 3.11.
        skip_nodejs (bool | Unset): Skip building Node.js environments Example: False.
        skip_python (bool | Unset): Skip building Python environments Example: False.
        timeout (int | Unset): Timeout in seconds for building each environment Example: 600.
    """

    pack_paths: list[str]
    force_rebuild: bool | Unset = UNSET
    nodejs_version: str | Unset = UNSET
    packs_base_dir: None | str | Unset = UNSET
    python_version: str | Unset = UNSET
    skip_nodejs: bool | Unset = UNSET
    skip_python: bool | Unset = UNSET
    timeout: int | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        pack_paths = self.pack_paths

        force_rebuild = self.force_rebuild

        nodejs_version = self.nodejs_version

        packs_base_dir: None | str | Unset
        if isinstance(self.packs_base_dir, Unset):
            packs_base_dir = UNSET
        else:
            packs_base_dir = self.packs_base_dir

        python_version = self.python_version

        skip_nodejs = self.skip_nodejs

        skip_python = self.skip_python

        timeout = self.timeout

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "pack_paths": pack_paths,
            }
        )
        if force_rebuild is not UNSET:
            field_dict["force_rebuild"] = force_rebuild
        if nodejs_version is not UNSET:
            field_dict["nodejs_version"] = nodejs_version
        if packs_base_dir is not UNSET:
            field_dict["packs_base_dir"] = packs_base_dir
        if python_version is not UNSET:
            field_dict["python_version"] = python_version
        if skip_nodejs is not UNSET:
            field_dict["skip_nodejs"] = skip_nodejs
        if skip_python is not UNSET:
            field_dict["skip_python"] = skip_python
        if timeout is not UNSET:
            field_dict["timeout"] = timeout

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        d = dict(src_dict)
        pack_paths = cast(list[str], d.pop("pack_paths"))

        force_rebuild = d.pop("force_rebuild", UNSET)

        nodejs_version = d.pop("nodejs_version", UNSET)

        def _parse_packs_base_dir(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        packs_base_dir = _parse_packs_base_dir(d.pop("packs_base_dir", UNSET))

        python_version = d.pop("python_version", UNSET)

        skip_nodejs = d.pop("skip_nodejs", UNSET)

        skip_python = d.pop("skip_python", UNSET)

        timeout = d.pop("timeout", UNSET)

        build_pack_envs_request = cls(
            pack_paths=pack_paths,
            force_rebuild=force_rebuild,
            nodejs_version=nodejs_version,
            packs_base_dir=packs_base_dir,
            python_version=python_version,
            skip_nodejs=skip_nodejs,
            skip_python=skip_python,
            timeout=timeout,
        )

        build_pack_envs_request.additional_properties = d
        return build_pack_envs_request

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
