from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.nullable_json_patch_type_1 import NullableJsonPatchType1
    from ..models.nullable_string_patch_type_1 import NullableStringPatchType1
    from ..models.set_json import SetJson
    from ..models.set_string import SetString
    from ..models.update_runtime_request_distributions_type_0 import (
        UpdateRuntimeRequestDistributionsType0,
    )
    from ..models.update_runtime_request_execution_config_type_0 import (
        UpdateRuntimeRequestExecutionConfigType0,
    )


T = TypeVar("T", bound="UpdateRuntimeRequest")


@_attrs_define
class UpdateRuntimeRequest:
    """Request DTO for updating a runtime.

    Attributes:
        distributions (None | UpdateRuntimeRequestDistributionsType0): Distribution metadata used for verification and
            platform support
        execution_config (None | UpdateRuntimeRequestExecutionConfigType0): Runtime execution configuration
        description (None | NullableStringPatchType1 | SetString | Unset):
        installation (None | NullableJsonPatchType1 | SetJson | Unset):
        name (None | str | Unset): Display name Example: Python 3.
    """

    distributions: None | UpdateRuntimeRequestDistributionsType0
    execution_config: None | UpdateRuntimeRequestExecutionConfigType0
    description: None | NullableStringPatchType1 | SetString | Unset = UNSET
    installation: None | NullableJsonPatchType1 | SetJson | Unset = UNSET
    name: None | str | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.nullable_json_patch_type_1 import NullableJsonPatchType1
        from ..models.nullable_string_patch_type_1 import NullableStringPatchType1
        from ..models.set_json import SetJson
        from ..models.set_string import SetString
        from ..models.update_runtime_request_distributions_type_0 import (
            UpdateRuntimeRequestDistributionsType0,
        )
        from ..models.update_runtime_request_execution_config_type_0 import (
            UpdateRuntimeRequestExecutionConfigType0,
        )

        distributions: dict[str, Any] | None
        if isinstance(self.distributions, UpdateRuntimeRequestDistributionsType0):
            distributions = self.distributions.to_dict()
        else:
            distributions = self.distributions

        execution_config: dict[str, Any] | None
        if isinstance(self.execution_config, UpdateRuntimeRequestExecutionConfigType0):
            execution_config = self.execution_config.to_dict()
        else:
            execution_config = self.execution_config

        description: dict[str, Any] | None | Unset
        if isinstance(self.description, Unset):
            description = UNSET
        elif isinstance(self.description, SetString):
            description = self.description.to_dict()
        elif isinstance(self.description, NullableStringPatchType1):
            description = self.description.to_dict()
        else:
            description = self.description

        installation: dict[str, Any] | None | Unset
        if isinstance(self.installation, Unset):
            installation = UNSET
        elif isinstance(self.installation, SetJson):
            installation = self.installation.to_dict()
        elif isinstance(self.installation, NullableJsonPatchType1):
            installation = self.installation.to_dict()
        else:
            installation = self.installation

        name: None | str | Unset
        if isinstance(self.name, Unset):
            name = UNSET
        else:
            name = self.name

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "distributions": distributions,
                "execution_config": execution_config,
            }
        )
        if description is not UNSET:
            field_dict["description"] = description
        if installation is not UNSET:
            field_dict["installation"] = installation
        if name is not UNSET:
            field_dict["name"] = name

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.nullable_json_patch_type_1 import NullableJsonPatchType1
        from ..models.nullable_string_patch_type_1 import NullableStringPatchType1
        from ..models.set_json import SetJson
        from ..models.set_string import SetString
        from ..models.update_runtime_request_distributions_type_0 import (
            UpdateRuntimeRequestDistributionsType0,
        )
        from ..models.update_runtime_request_execution_config_type_0 import (
            UpdateRuntimeRequestExecutionConfigType0,
        )

        d = dict(src_dict)

        def _parse_distributions(
            data: object,
        ) -> None | UpdateRuntimeRequestDistributionsType0:
            if data is None:
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                distributions_type_0 = UpdateRuntimeRequestDistributionsType0.from_dict(
                    data
                )

                return distributions_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(None | UpdateRuntimeRequestDistributionsType0, data)

        distributions = _parse_distributions(d.pop("distributions"))

        def _parse_execution_config(
            data: object,
        ) -> None | UpdateRuntimeRequestExecutionConfigType0:
            if data is None:
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                execution_config_type_0 = (
                    UpdateRuntimeRequestExecutionConfigType0.from_dict(data)
                )

                return execution_config_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(None | UpdateRuntimeRequestExecutionConfigType0, data)

        execution_config = _parse_execution_config(d.pop("execution_config"))

        def _parse_description(
            data: object,
        ) -> None | NullableStringPatchType1 | SetString | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_nullable_string_patch_set_string = (
                    SetString.from_dict(data)
                )

                return componentsschemas_nullable_string_patch_set_string
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_nullable_string_patch_type_1 = (
                    NullableStringPatchType1.from_dict(data)
                )

                return componentsschemas_nullable_string_patch_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(None | NullableStringPatchType1 | SetString | Unset, data)

        description = _parse_description(d.pop("description", UNSET))

        def _parse_installation(
            data: object,
        ) -> None | NullableJsonPatchType1 | SetJson | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_nullable_json_patch_set_json = SetJson.from_dict(data)

                return componentsschemas_nullable_json_patch_set_json
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_nullable_json_patch_type_1 = (
                    NullableJsonPatchType1.from_dict(data)
                )

                return componentsschemas_nullable_json_patch_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(None | NullableJsonPatchType1 | SetJson | Unset, data)

        installation = _parse_installation(d.pop("installation", UNSET))

        def _parse_name(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        name = _parse_name(d.pop("name", UNSET))

        update_runtime_request = cls(
            distributions=distributions,
            execution_config=execution_config,
            description=description,
            installation=installation,
            name=name,
        )

        update_runtime_request.additional_properties = d
        return update_runtime_request

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
