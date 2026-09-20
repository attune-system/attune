from __future__ import annotations

import datetime
from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

from ..types import UNSET, Unset

T = TypeVar("T", bound="PackReleaseResponse")


@_attrs_define
class PackReleaseResponse:
    """Public metadata for an immutable pack release.

    Attributes:
        archive_size (int):
        created (datetime.datetime):
        digest (str):
        id (int):
        is_active (bool):
        version (str):
        inactive_since (datetime.datetime | None | Unset):
    """

    archive_size: int
    created: datetime.datetime
    digest: str
    id: int
    is_active: bool
    version: str
    inactive_since: datetime.datetime | None | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        archive_size = self.archive_size

        created = self.created.isoformat()

        digest = self.digest

        id = self.id

        is_active = self.is_active

        version = self.version

        inactive_since: None | str | Unset
        if isinstance(self.inactive_since, Unset):
            inactive_since = UNSET
        elif isinstance(self.inactive_since, datetime.datetime):
            inactive_since = self.inactive_since.isoformat()
        else:
            inactive_since = self.inactive_since

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "archive_size": archive_size,
                "created": created,
                "digest": digest,
                "id": id,
                "is_active": is_active,
                "version": version,
            }
        )
        if inactive_since is not UNSET:
            field_dict["inactive_since"] = inactive_since

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        d = dict(src_dict)
        archive_size = d.pop("archive_size")

        created = datetime.datetime.fromisoformat(d.pop("created"))

        digest = d.pop("digest")

        id = d.pop("id")

        is_active = d.pop("is_active")

        version = d.pop("version")

        def _parse_inactive_since(data: object) -> datetime.datetime | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                inactive_since_type_0 = datetime.datetime.fromisoformat(data)

                return inactive_since_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(datetime.datetime | None | Unset, data)

        inactive_since = _parse_inactive_since(d.pop("inactive_since", UNSET))

        pack_release_response = cls(
            archive_size=archive_size,
            created=created,
            digest=digest,
            id=id,
            is_active=is_active,
            version=version,
            inactive_since=inactive_since,
        )

        pack_release_response.additional_properties = d
        return pack_release_response

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
