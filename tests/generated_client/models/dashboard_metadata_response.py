from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..models.dashboard_scope_type import DashboardScopeType
from ..models.dashboard_visibility import DashboardVisibility
from ..types import UNSET, Unset
from typing import cast
import datetime

if TYPE_CHECKING:
  from ..models.dashboard_metadata_response_spec import DashboardMetadataResponseSpec





T = TypeVar("T", bound="DashboardMetadataResponse")



@_attrs_define
class DashboardMetadataResponse:
    """ 
        Attributes:
            created (datetime.datetime):
            enabled (bool):
            id (int):
            is_adhoc (bool):
            is_default_home (bool):
            label (str):
            ref (str):
            revision (int):
            scope_ref (str):
            scope_type (DashboardScopeType):
            spec (DashboardMetadataResponseSpec):
            spec_version (int):
            tags (list[str]):
            updated (datetime.datetime):
            visibility (DashboardVisibility):
            description (None | str | Unset):
            owner_identity (int | None | Unset):
            pack (int | None | Unset):
     """

    created: datetime.datetime
    enabled: bool
    id: int
    is_adhoc: bool
    is_default_home: bool
    label: str
    ref: str
    revision: int
    scope_ref: str
    scope_type: DashboardScopeType
    spec: DashboardMetadataResponseSpec
    spec_version: int
    tags: list[str]
    updated: datetime.datetime
    visibility: DashboardVisibility
    description: None | str | Unset = UNSET
    owner_identity: int | None | Unset = UNSET
    pack: int | None | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)





    def to_dict(self) -> dict[str, Any]:
        from ..models.dashboard_metadata_response_spec import DashboardMetadataResponseSpec
        created = self.created.isoformat()

        enabled = self.enabled

        id = self.id

        is_adhoc = self.is_adhoc

        is_default_home = self.is_default_home

        label = self.label

        ref = self.ref

        revision = self.revision

        scope_ref = self.scope_ref

        scope_type = self.scope_type.value

        spec = self.spec.to_dict()

        spec_version = self.spec_version

        tags = self.tags



        updated = self.updated.isoformat()

        visibility = self.visibility.value

        description: None | str | Unset
        if isinstance(self.description, Unset):
            description = UNSET
        else:
            description = self.description

        owner_identity: int | None | Unset
        if isinstance(self.owner_identity, Unset):
            owner_identity = UNSET
        else:
            owner_identity = self.owner_identity

        pack: int | None | Unset
        if isinstance(self.pack, Unset):
            pack = UNSET
        else:
            pack = self.pack


        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update({
            "created": created,
            "enabled": enabled,
            "id": id,
            "is_adhoc": is_adhoc,
            "is_default_home": is_default_home,
            "label": label,
            "ref": ref,
            "revision": revision,
            "scope_ref": scope_ref,
            "scope_type": scope_type,
            "spec": spec,
            "spec_version": spec_version,
            "tags": tags,
            "updated": updated,
            "visibility": visibility,
        })
        if description is not UNSET:
            field_dict["description"] = description
        if owner_identity is not UNSET:
            field_dict["owner_identity"] = owner_identity
        if pack is not UNSET:
            field_dict["pack"] = pack

        return field_dict



    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.dashboard_metadata_response_spec import DashboardMetadataResponseSpec
        d = dict(src_dict)
        created = datetime.datetime.fromisoformat(d.pop("created"))




        enabled = d.pop("enabled")

        id = d.pop("id")

        is_adhoc = d.pop("is_adhoc")

        is_default_home = d.pop("is_default_home")

        label = d.pop("label")

        ref = d.pop("ref")

        revision = d.pop("revision")

        scope_ref = d.pop("scope_ref")

        scope_type = DashboardScopeType(d.pop("scope_type"))




        spec = DashboardMetadataResponseSpec.from_dict(d.pop("spec"))




        spec_version = d.pop("spec_version")

        tags = cast(list[str], d.pop("tags"))


        updated = datetime.datetime.fromisoformat(d.pop("updated"))




        visibility = DashboardVisibility(d.pop("visibility"))




        def _parse_description(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        description = _parse_description(d.pop("description", UNSET))


        def _parse_owner_identity(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        owner_identity = _parse_owner_identity(d.pop("owner_identity", UNSET))


        def _parse_pack(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        pack = _parse_pack(d.pop("pack", UNSET))


        dashboard_metadata_response = cls(
            created=created,
            enabled=enabled,
            id=id,
            is_adhoc=is_adhoc,
            is_default_home=is_default_home,
            label=label,
            ref=ref,
            revision=revision,
            scope_ref=scope_ref,
            scope_type=scope_type,
            spec=spec,
            spec_version=spec_version,
            tags=tags,
            updated=updated,
            visibility=visibility,
            description=description,
            owner_identity=owner_identity,
            pack=pack,
        )


        dashboard_metadata_response.additional_properties = d
        return dashboard_metadata_response

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
