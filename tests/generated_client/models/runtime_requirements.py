from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.node_js_requirements import NodeJsRequirements
    from ..models.python_requirements import PythonRequirements


T = TypeVar("T", bound="RuntimeRequirements")


@_attrs_define
class RuntimeRequirements:
    """Runtime requirements for a pack

    Attributes:
        pack_ref (str): Pack reference
        nodejs (NodeJsRequirements | None | Unset):
        python (None | PythonRequirements | Unset):
    """

    pack_ref: str
    nodejs: NodeJsRequirements | None | Unset = UNSET
    python: None | PythonRequirements | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.node_js_requirements import NodeJsRequirements
        from ..models.python_requirements import PythonRequirements

        pack_ref = self.pack_ref

        nodejs: dict[str, Any] | None | Unset
        if isinstance(self.nodejs, Unset):
            nodejs = UNSET
        elif isinstance(self.nodejs, NodeJsRequirements):
            nodejs = self.nodejs.to_dict()
        else:
            nodejs = self.nodejs

        python: dict[str, Any] | None | Unset
        if isinstance(self.python, Unset):
            python = UNSET
        elif isinstance(self.python, PythonRequirements):
            python = self.python.to_dict()
        else:
            python = self.python

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "pack_ref": pack_ref,
            }
        )
        if nodejs is not UNSET:
            field_dict["nodejs"] = nodejs
        if python is not UNSET:
            field_dict["python"] = python

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        from ..models.node_js_requirements import NodeJsRequirements
        from ..models.python_requirements import PythonRequirements

        d = dict(src_dict)
        pack_ref = d.pop("pack_ref")

        def _parse_nodejs(data: object) -> NodeJsRequirements | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                nodejs_type_1 = NodeJsRequirements.from_dict(data)

                return nodejs_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(NodeJsRequirements | None | Unset, data)

        nodejs = _parse_nodejs(d.pop("nodejs", UNSET))

        def _parse_python(data: object) -> None | PythonRequirements | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                python_type_1 = PythonRequirements.from_dict(data)

                return python_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(None | PythonRequirements | Unset, data)

        python = _parse_python(d.pop("python", UNSET))

        runtime_requirements = cls(
            pack_ref=pack_ref,
            nodejs=nodejs,
            python=python,
        )

        runtime_requirements.additional_properties = d
        return runtime_requirements

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
