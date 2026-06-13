from __future__ import annotations

import datetime
from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.runtime_response_distributions import RuntimeResponseDistributions
    from ..models.runtime_response_execution_config import (
        RuntimeResponseExecutionConfig,
    )
    from ..models.runtime_response_installation_type_0 import (
        RuntimeResponseInstallationType0,
    )


T = TypeVar("T", bound="RuntimeResponse")


@_attrs_define
class RuntimeResponse:
    """Full runtime response.

    Attributes:
        created (datetime.datetime):  Example: 2024-01-13T10:30:00Z.
        distributions (RuntimeResponseDistributions):
        execution_config (RuntimeResponseExecutionConfig):
        id (int):  Example: 1.
        installation (None | RuntimeResponseInstallationType0):
        name (str):  Example: Python.
        ref (str):  Example: core.python.
        updated (datetime.datetime):  Example: 2024-01-13T10:30:00Z.
        description (None | str | Unset):  Example: Python runtime with virtualenv support.
        pack (int | None | Unset):  Example: 1.
        pack_ref (None | str | Unset):  Example: core.
    """

    created: datetime.datetime
    distributions: RuntimeResponseDistributions
    execution_config: RuntimeResponseExecutionConfig
    id: int
    installation: None | RuntimeResponseInstallationType0
    name: str
    ref: str
    updated: datetime.datetime
    description: None | str | Unset = UNSET
    pack: int | None | Unset = UNSET
    pack_ref: None | str | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.runtime_response_installation_type_0 import (
            RuntimeResponseInstallationType0,
        )

        created = self.created.isoformat()

        distributions = self.distributions.to_dict()

        execution_config = self.execution_config.to_dict()

        id = self.id

        installation: dict[str, Any] | None
        if isinstance(self.installation, RuntimeResponseInstallationType0):
            installation = self.installation.to_dict()
        else:
            installation = self.installation

        name = self.name

        ref = self.ref

        updated = self.updated.isoformat()

        description: None | str | Unset
        if isinstance(self.description, Unset):
            description = UNSET
        else:
            description = self.description

        pack: int | None | Unset
        if isinstance(self.pack, Unset):
            pack = UNSET
        else:
            pack = self.pack

        pack_ref: None | str | Unset
        if isinstance(self.pack_ref, Unset):
            pack_ref = UNSET
        else:
            pack_ref = self.pack_ref

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "created": created,
                "distributions": distributions,
                "execution_config": execution_config,
                "id": id,
                "installation": installation,
                "name": name,
                "ref": ref,
                "updated": updated,
            }
        )
        if description is not UNSET:
            field_dict["description"] = description
        if pack is not UNSET:
            field_dict["pack"] = pack
        if pack_ref is not UNSET:
            field_dict["pack_ref"] = pack_ref

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.runtime_response_distributions import RuntimeResponseDistributions
        from ..models.runtime_response_execution_config import (
            RuntimeResponseExecutionConfig,
        )
        from ..models.runtime_response_installation_type_0 import (
            RuntimeResponseInstallationType0,
        )

        d = dict(src_dict)
        created = datetime.datetime.fromisoformat(d.pop("created"))

        distributions = RuntimeResponseDistributions.from_dict(d.pop("distributions"))

        execution_config = RuntimeResponseExecutionConfig.from_dict(
            d.pop("execution_config")
        )

        id = d.pop("id")

        def _parse_installation(
            data: object,
        ) -> None | RuntimeResponseInstallationType0:
            if data is None:
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                installation_type_0 = RuntimeResponseInstallationType0.from_dict(data)

                return installation_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(None | RuntimeResponseInstallationType0, data)

        installation = _parse_installation(d.pop("installation"))

        name = d.pop("name")

        ref = d.pop("ref")

        updated = datetime.datetime.fromisoformat(d.pop("updated"))

        def _parse_description(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        description = _parse_description(d.pop("description", UNSET))

        def _parse_pack(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        pack = _parse_pack(d.pop("pack", UNSET))

        def _parse_pack_ref(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        pack_ref = _parse_pack_ref(d.pop("pack_ref", UNSET))

        runtime_response = cls(
            created=created,
            distributions=distributions,
            execution_config=execution_config,
            id=id,
            installation=installation,
            name=name,
            ref=ref,
            updated=updated,
            description=description,
            pack=pack,
            pack_ref=pack_ref,
        )

        runtime_response.additional_properties = d
        return runtime_response

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
