from __future__ import annotations

from collections.abc import Mapping
from io import BytesIO
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

from .. import types
from ..types import UNSET, File, Unset

T = TypeVar("T", bound="PackUploadForm")


@_attrs_define
class PackUploadForm:
    """
    Attributes:
        pack (File):
        force (None | str | Unset):
        skip_tests (None | str | Unset):
    """

    pack: File
    force: None | str | Unset = UNSET
    skip_tests: None | str | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        pack = self.pack.to_tuple()

        force: None | str | Unset
        if isinstance(self.force, Unset):
            force = UNSET
        else:
            force = self.force

        skip_tests: None | str | Unset
        if isinstance(self.skip_tests, Unset):
            skip_tests = UNSET
        else:
            skip_tests = self.skip_tests

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "pack": pack,
            }
        )
        if force is not UNSET:
            field_dict["force"] = force
        if skip_tests is not UNSET:
            field_dict["skip_tests"] = skip_tests

        return field_dict

    def to_multipart(self) -> types.RequestFiles:
        files: types.RequestFiles = []

        files.append(("pack", self.pack.to_tuple()))

        if not isinstance(self.force, Unset):
            if isinstance(self.force, str):
                files.append(("force", (None, str(self.force).encode(), "text/plain")))
            else:
                files.append(("force", (None, str(self.force).encode(), "text/plain")))

        if not isinstance(self.skip_tests, Unset):
            if isinstance(self.skip_tests, str):
                files.append(
                    ("skip_tests", (None, str(self.skip_tests).encode(), "text/plain"))
                )
            else:
                files.append(
                    ("skip_tests", (None, str(self.skip_tests).encode(), "text/plain"))
                )

        for prop_name, prop in self.additional_properties.items():
            files.append((prop_name, (None, str(prop).encode(), "text/plain")))

        return files

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        d = dict(src_dict)
        pack = File(payload=BytesIO(d.pop("pack")))

        def _parse_force(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        force = _parse_force(d.pop("force", UNSET))

        def _parse_skip_tests(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        skip_tests = _parse_skip_tests(d.pop("skip_tests", UNSET))

        pack_upload_form = cls(
            pack=pack,
            force=force,
            skip_tests=skip_tests,
        )

        pack_upload_form.additional_properties = d
        return pack_upload_form

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
