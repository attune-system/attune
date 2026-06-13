from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.create_runtime_request_distributions import (
        CreateRuntimeRequestDistributions,
    )
    from ..models.create_runtime_request_execution_config import (
        CreateRuntimeRequestExecutionConfig,
    )
    from ..models.create_runtime_request_installation_type_0 import (
        CreateRuntimeRequestInstallationType0,
    )


T = TypeVar("T", bound="CreateRuntimeRequest")


@_attrs_define
class CreateRuntimeRequest:
    """Request DTO for creating a runtime.

    Attributes:
        name (str): Display name Example: Python.
        ref (str): Unique reference identifier (e.g. "core.python", "core.nodejs") Example: core.python.
        description (None | str | Unset): Optional human-readable description Example: Python runtime with virtualenv
            support.
        distributions (CreateRuntimeRequestDistributions | Unset): Distribution metadata used for verification and
            platform support
        execution_config (CreateRuntimeRequestExecutionConfig | Unset): Runtime execution configuration
        installation (CreateRuntimeRequestInstallationType0 | None | Unset): Optional installation metadata
        pack_ref (None | str | Unset): Optional pack reference this runtime belongs to Example: core.
    """

    name: str
    ref: str
    description: None | str | Unset = UNSET
    distributions: CreateRuntimeRequestDistributions | Unset = UNSET
    execution_config: CreateRuntimeRequestExecutionConfig | Unset = UNSET
    installation: CreateRuntimeRequestInstallationType0 | None | Unset = UNSET
    pack_ref: None | str | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.create_runtime_request_installation_type_0 import (
            CreateRuntimeRequestInstallationType0,
        )

        name = self.name

        ref = self.ref

        description: None | str | Unset
        if isinstance(self.description, Unset):
            description = UNSET
        else:
            description = self.description

        distributions: dict[str, Any] | Unset = UNSET
        if not isinstance(self.distributions, Unset):
            distributions = self.distributions.to_dict()

        execution_config: dict[str, Any] | Unset = UNSET
        if not isinstance(self.execution_config, Unset):
            execution_config = self.execution_config.to_dict()

        installation: dict[str, Any] | None | Unset
        if isinstance(self.installation, Unset):
            installation = UNSET
        elif isinstance(self.installation, CreateRuntimeRequestInstallationType0):
            installation = self.installation.to_dict()
        else:
            installation = self.installation

        pack_ref: None | str | Unset
        if isinstance(self.pack_ref, Unset):
            pack_ref = UNSET
        else:
            pack_ref = self.pack_ref

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "name": name,
                "ref": ref,
            }
        )
        if description is not UNSET:
            field_dict["description"] = description
        if distributions is not UNSET:
            field_dict["distributions"] = distributions
        if execution_config is not UNSET:
            field_dict["execution_config"] = execution_config
        if installation is not UNSET:
            field_dict["installation"] = installation
        if pack_ref is not UNSET:
            field_dict["pack_ref"] = pack_ref

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.create_runtime_request_distributions import (
            CreateRuntimeRequestDistributions,
        )
        from ..models.create_runtime_request_execution_config import (
            CreateRuntimeRequestExecutionConfig,
        )
        from ..models.create_runtime_request_installation_type_0 import (
            CreateRuntimeRequestInstallationType0,
        )

        d = dict(src_dict)
        name = d.pop("name")

        ref = d.pop("ref")

        def _parse_description(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        description = _parse_description(d.pop("description", UNSET))

        _distributions = d.pop("distributions", UNSET)
        distributions: CreateRuntimeRequestDistributions | Unset
        if isinstance(_distributions, Unset):
            distributions = UNSET
        else:
            distributions = CreateRuntimeRequestDistributions.from_dict(_distributions)

        _execution_config = d.pop("execution_config", UNSET)
        execution_config: CreateRuntimeRequestExecutionConfig | Unset
        if isinstance(_execution_config, Unset):
            execution_config = UNSET
        else:
            execution_config = CreateRuntimeRequestExecutionConfig.from_dict(
                _execution_config
            )

        def _parse_installation(
            data: object,
        ) -> CreateRuntimeRequestInstallationType0 | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                installation_type_0 = CreateRuntimeRequestInstallationType0.from_dict(
                    data
                )

                return installation_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(CreateRuntimeRequestInstallationType0 | None | Unset, data)

        installation = _parse_installation(d.pop("installation", UNSET))

        def _parse_pack_ref(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        pack_ref = _parse_pack_ref(d.pop("pack_ref", UNSET))

        create_runtime_request = cls(
            name=name,
            ref=ref,
            description=description,
            distributions=distributions,
            execution_config=execution_config,
            installation=installation,
            pack_ref=pack_ref,
        )

        create_runtime_request.additional_properties = d
        return create_runtime_request

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
