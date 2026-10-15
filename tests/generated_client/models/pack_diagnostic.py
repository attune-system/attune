from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

from ..models.pack_diagnostic_severity import PackDiagnosticSeverity
from ..types import UNSET, Unset

T = TypeVar("T", bound="PackDiagnostic")


@_attrs_define
class PackDiagnostic:
    """
    Attributes:
        code (str):
        message (str):
        severity (PackDiagnosticSeverity):
        column (int | None | Unset):
        component_kind (None | str | Unset):
        component_ref (None | str | Unset):
        hint (None | str | Unset):
        line (int | None | Unset):
        metadata_path (None | str | Unset):
        path (None | str | Unset):
        reference_chain (list[str] | Unset):
    """

    code: str
    message: str
    severity: PackDiagnosticSeverity
    column: int | None | Unset = UNSET
    component_kind: None | str | Unset = UNSET
    component_ref: None | str | Unset = UNSET
    hint: None | str | Unset = UNSET
    line: int | None | Unset = UNSET
    metadata_path: None | str | Unset = UNSET
    path: None | str | Unset = UNSET
    reference_chain: list[str] | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        code = self.code

        message = self.message

        severity = self.severity.value

        column: int | None | Unset
        if isinstance(self.column, Unset):
            column = UNSET
        else:
            column = self.column

        component_kind: None | str | Unset
        if isinstance(self.component_kind, Unset):
            component_kind = UNSET
        else:
            component_kind = self.component_kind

        component_ref: None | str | Unset
        if isinstance(self.component_ref, Unset):
            component_ref = UNSET
        else:
            component_ref = self.component_ref

        hint: None | str | Unset
        if isinstance(self.hint, Unset):
            hint = UNSET
        else:
            hint = self.hint

        line: int | None | Unset
        if isinstance(self.line, Unset):
            line = UNSET
        else:
            line = self.line

        metadata_path: None | str | Unset
        if isinstance(self.metadata_path, Unset):
            metadata_path = UNSET
        else:
            metadata_path = self.metadata_path

        path: None | str | Unset
        if isinstance(self.path, Unset):
            path = UNSET
        else:
            path = self.path

        reference_chain: list[str] | Unset = UNSET
        if not isinstance(self.reference_chain, Unset):
            reference_chain = self.reference_chain

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "code": code,
                "message": message,
                "severity": severity,
            }
        )
        if column is not UNSET:
            field_dict["column"] = column
        if component_kind is not UNSET:
            field_dict["component_kind"] = component_kind
        if component_ref is not UNSET:
            field_dict["component_ref"] = component_ref
        if hint is not UNSET:
            field_dict["hint"] = hint
        if line is not UNSET:
            field_dict["line"] = line
        if metadata_path is not UNSET:
            field_dict["metadata_path"] = metadata_path
        if path is not UNSET:
            field_dict["path"] = path
        if reference_chain is not UNSET:
            field_dict["reference_chain"] = reference_chain

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        d = dict(src_dict)
        code = d.pop("code")

        message = d.pop("message")

        severity = PackDiagnosticSeverity(d.pop("severity"))

        def _parse_column(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        column = _parse_column(d.pop("column", UNSET))

        def _parse_component_kind(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        component_kind = _parse_component_kind(d.pop("component_kind", UNSET))

        def _parse_component_ref(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        component_ref = _parse_component_ref(d.pop("component_ref", UNSET))

        def _parse_hint(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        hint = _parse_hint(d.pop("hint", UNSET))

        def _parse_line(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        line = _parse_line(d.pop("line", UNSET))

        def _parse_metadata_path(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        metadata_path = _parse_metadata_path(d.pop("metadata_path", UNSET))

        def _parse_path(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        path = _parse_path(d.pop("path", UNSET))

        reference_chain = cast(list[str], d.pop("reference_chain", UNSET))

        pack_diagnostic = cls(
            code=code,
            message=message,
            severity=severity,
            column=column,
            component_kind=component_kind,
            component_ref=component_ref,
            hint=hint,
            line=line,
            metadata_path=metadata_path,
            path=path,
            reference_chain=reference_chain,
        )

        pack_diagnostic.additional_properties = d
        return pack_diagnostic

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
