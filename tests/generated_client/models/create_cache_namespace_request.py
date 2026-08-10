from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..models.owner_type import OwnerType
from ..types import UNSET, Unset
from typing import cast






T = TypeVar("T", bound="CreateCacheNamespaceRequest")



@_attrs_define
class CreateCacheNamespaceRequest:
    """ Create a new owner-scoped cache namespace.

        Attributes:
            namespace (str): Normalized lowercase namespace, e.g. `salesforce.users`. Example: salesforce.users.
            owner_type (OwnerType):
            freshness_target_seconds (int | None | Unset):
            max_generation_bytes (int | None | Unset):
            max_records_per_generation (int | None | Unset):
            max_retained_bytes (int | None | Unset):
            max_retained_generations (int | None | Unset): Number of published generations retained. At least two are
                required so
                readers can complete traversal of the prior snapshot after promotion. Example: 2.
            max_staging_generations (int | None | Unset):
            owner_ref (None | str | Unset):
     """

    namespace: str
    owner_type: OwnerType
    freshness_target_seconds: int | None | Unset = UNSET
    max_generation_bytes: int | None | Unset = UNSET
    max_records_per_generation: int | None | Unset = UNSET
    max_retained_bytes: int | None | Unset = UNSET
    max_retained_generations: int | None | Unset = UNSET
    max_staging_generations: int | None | Unset = UNSET
    owner_ref: None | str | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)





    def to_dict(self) -> dict[str, Any]:
        namespace = self.namespace

        owner_type = self.owner_type.value

        freshness_target_seconds: int | None | Unset
        if isinstance(self.freshness_target_seconds, Unset):
            freshness_target_seconds = UNSET
        else:
            freshness_target_seconds = self.freshness_target_seconds

        max_generation_bytes: int | None | Unset
        if isinstance(self.max_generation_bytes, Unset):
            max_generation_bytes = UNSET
        else:
            max_generation_bytes = self.max_generation_bytes

        max_records_per_generation: int | None | Unset
        if isinstance(self.max_records_per_generation, Unset):
            max_records_per_generation = UNSET
        else:
            max_records_per_generation = self.max_records_per_generation

        max_retained_bytes: int | None | Unset
        if isinstance(self.max_retained_bytes, Unset):
            max_retained_bytes = UNSET
        else:
            max_retained_bytes = self.max_retained_bytes

        max_retained_generations: int | None | Unset
        if isinstance(self.max_retained_generations, Unset):
            max_retained_generations = UNSET
        else:
            max_retained_generations = self.max_retained_generations

        max_staging_generations: int | None | Unset
        if isinstance(self.max_staging_generations, Unset):
            max_staging_generations = UNSET
        else:
            max_staging_generations = self.max_staging_generations

        owner_ref: None | str | Unset
        if isinstance(self.owner_ref, Unset):
            owner_ref = UNSET
        else:
            owner_ref = self.owner_ref


        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update({
            "namespace": namespace,
            "owner_type": owner_type,
        })
        if freshness_target_seconds is not UNSET:
            field_dict["freshness_target_seconds"] = freshness_target_seconds
        if max_generation_bytes is not UNSET:
            field_dict["max_generation_bytes"] = max_generation_bytes
        if max_records_per_generation is not UNSET:
            field_dict["max_records_per_generation"] = max_records_per_generation
        if max_retained_bytes is not UNSET:
            field_dict["max_retained_bytes"] = max_retained_bytes
        if max_retained_generations is not UNSET:
            field_dict["max_retained_generations"] = max_retained_generations
        if max_staging_generations is not UNSET:
            field_dict["max_staging_generations"] = max_staging_generations
        if owner_ref is not UNSET:
            field_dict["owner_ref"] = owner_ref

        return field_dict



    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        namespace = d.pop("namespace")

        owner_type = OwnerType(d.pop("owner_type"))




        def _parse_freshness_target_seconds(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        freshness_target_seconds = _parse_freshness_target_seconds(d.pop("freshness_target_seconds", UNSET))


        def _parse_max_generation_bytes(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        max_generation_bytes = _parse_max_generation_bytes(d.pop("max_generation_bytes", UNSET))


        def _parse_max_records_per_generation(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        max_records_per_generation = _parse_max_records_per_generation(d.pop("max_records_per_generation", UNSET))


        def _parse_max_retained_bytes(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        max_retained_bytes = _parse_max_retained_bytes(d.pop("max_retained_bytes", UNSET))


        def _parse_max_retained_generations(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        max_retained_generations = _parse_max_retained_generations(d.pop("max_retained_generations", UNSET))


        def _parse_max_staging_generations(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        max_staging_generations = _parse_max_staging_generations(d.pop("max_staging_generations", UNSET))


        def _parse_owner_ref(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        owner_ref = _parse_owner_ref(d.pop("owner_ref", UNSET))


        create_cache_namespace_request = cls(
            namespace=namespace,
            owner_type=owner_type,
            freshness_target_seconds=freshness_target_seconds,
            max_generation_bytes=max_generation_bytes,
            max_records_per_generation=max_records_per_generation,
            max_retained_bytes=max_retained_bytes,
            max_retained_generations=max_retained_generations,
            max_staging_generations=max_staging_generations,
            owner_ref=owner_ref,
        )


        create_cache_namespace_request.additional_properties = d
        return create_cache_namespace_request

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
