from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.node_js_environment import NodeJsEnvironment
    from ..models.python_environment import PythonEnvironment


T = TypeVar("T", bound="Environments")


@_attrs_define
class Environments:
    """Environment details

    Attributes:
        nodejs (NodeJsEnvironment | None | Unset):
        python (None | PythonEnvironment | Unset):
    """

    nodejs: NodeJsEnvironment | None | Unset = UNSET
    python: None | PythonEnvironment | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.node_js_environment import NodeJsEnvironment
        from ..models.python_environment import PythonEnvironment

        nodejs: dict[str, Any] | None | Unset
        if isinstance(self.nodejs, Unset):
            nodejs = UNSET
        elif isinstance(self.nodejs, NodeJsEnvironment):
            nodejs = self.nodejs.to_dict()
        else:
            nodejs = self.nodejs

        python: dict[str, Any] | None | Unset
        if isinstance(self.python, Unset):
            python = UNSET
        elif isinstance(self.python, PythonEnvironment):
            python = self.python.to_dict()
        else:
            python = self.python

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update({})
        if nodejs is not UNSET:
            field_dict["nodejs"] = nodejs
        if python is not UNSET:
            field_dict["python"] = python

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        from ..models.node_js_environment import NodeJsEnvironment
        from ..models.python_environment import PythonEnvironment

        d = dict(src_dict)

        def _parse_nodejs(data: object) -> NodeJsEnvironment | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                nodejs_type_1 = NodeJsEnvironment.from_dict(data)

                return nodejs_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(NodeJsEnvironment | None | Unset, data)

        nodejs = _parse_nodejs(d.pop("nodejs", UNSET))

        def _parse_python(data: object) -> None | PythonEnvironment | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                python_type_1 = PythonEnvironment.from_dict(data)

                return python_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(None | PythonEnvironment | Unset, data)

        python = _parse_python(d.pop("python", UNSET))

        environments = cls(
            nodejs=nodejs,
            python=python,
        )

        environments.additional_properties = d
        return environments

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
