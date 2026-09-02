from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

from ..models.checksum_subject import ChecksumSubject
from ..types import UNSET, Unset

T = TypeVar("T", bound="PackInstallProvenance")


@_attrs_define
class PackInstallProvenance:
    """
    Attributes:
        artifact_type (str): Concrete installed artifact type (git, archive, or a local source type).
        checksum_verified (bool): Whether the checksum was verified against its documented subject.
        fallback_occurred (bool): Whether installation fell back from the preferred registry artifact.
        artifact_url (None | str | Unset): Selected artifact URL or local path.
        checksum (None | str | Unset): Canonical checksum in `algorithm:hash` form. For archives this covers
            the downloaded artifact bytes; for Git and local sources it covers the
            installed directory content.
        checksum_subject (ChecksumSubject | None | Unset):
        git_ref (None | str | Unset): Selected Git branch, tag, or commit, when applicable.
        registry_id (int | None | Unset): API-managed registry row selected for resolution, when known.
        registry_url (None | str | Unset): Registry index URL that resolved the pack, when applicable.
        resolved_pack (None | str | Unset): Canonical resolved registry identity in `pack@version` form.
    """

    artifact_type: str
    checksum_verified: bool
    fallback_occurred: bool
    artifact_url: None | str | Unset = UNSET
    checksum: None | str | Unset = UNSET
    checksum_subject: ChecksumSubject | None | Unset = UNSET
    git_ref: None | str | Unset = UNSET
    registry_id: int | None | Unset = UNSET
    registry_url: None | str | Unset = UNSET
    resolved_pack: None | str | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        artifact_type = self.artifact_type

        checksum_verified = self.checksum_verified

        fallback_occurred = self.fallback_occurred

        artifact_url: None | str | Unset
        if isinstance(self.artifact_url, Unset):
            artifact_url = UNSET
        else:
            artifact_url = self.artifact_url

        checksum: None | str | Unset
        if isinstance(self.checksum, Unset):
            checksum = UNSET
        else:
            checksum = self.checksum

        checksum_subject: None | str | Unset
        if isinstance(self.checksum_subject, Unset):
            checksum_subject = UNSET
        elif isinstance(self.checksum_subject, ChecksumSubject):
            checksum_subject = self.checksum_subject.value
        else:
            checksum_subject = self.checksum_subject

        git_ref: None | str | Unset
        if isinstance(self.git_ref, Unset):
            git_ref = UNSET
        else:
            git_ref = self.git_ref

        registry_id: int | None | Unset
        if isinstance(self.registry_id, Unset):
            registry_id = UNSET
        else:
            registry_id = self.registry_id

        registry_url: None | str | Unset
        if isinstance(self.registry_url, Unset):
            registry_url = UNSET
        else:
            registry_url = self.registry_url

        resolved_pack: None | str | Unset
        if isinstance(self.resolved_pack, Unset):
            resolved_pack = UNSET
        else:
            resolved_pack = self.resolved_pack

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "artifact_type": artifact_type,
                "checksum_verified": checksum_verified,
                "fallback_occurred": fallback_occurred,
            }
        )
        if artifact_url is not UNSET:
            field_dict["artifact_url"] = artifact_url
        if checksum is not UNSET:
            field_dict["checksum"] = checksum
        if checksum_subject is not UNSET:
            field_dict["checksum_subject"] = checksum_subject
        if git_ref is not UNSET:
            field_dict["git_ref"] = git_ref
        if registry_id is not UNSET:
            field_dict["registry_id"] = registry_id
        if registry_url is not UNSET:
            field_dict["registry_url"] = registry_url
        if resolved_pack is not UNSET:
            field_dict["resolved_pack"] = resolved_pack

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        d = dict(src_dict)
        artifact_type = d.pop("artifact_type")

        checksum_verified = d.pop("checksum_verified")

        fallback_occurred = d.pop("fallback_occurred")

        def _parse_artifact_url(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        artifact_url = _parse_artifact_url(d.pop("artifact_url", UNSET))

        def _parse_checksum(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        checksum = _parse_checksum(d.pop("checksum", UNSET))

        def _parse_checksum_subject(data: object) -> ChecksumSubject | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                checksum_subject_type_1 = ChecksumSubject(data)

                return checksum_subject_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(ChecksumSubject | None | Unset, data)

        checksum_subject = _parse_checksum_subject(d.pop("checksum_subject", UNSET))

        def _parse_git_ref(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        git_ref = _parse_git_ref(d.pop("git_ref", UNSET))

        def _parse_registry_id(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        registry_id = _parse_registry_id(d.pop("registry_id", UNSET))

        def _parse_registry_url(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        registry_url = _parse_registry_url(d.pop("registry_url", UNSET))

        def _parse_resolved_pack(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        resolved_pack = _parse_resolved_pack(d.pop("resolved_pack", UNSET))

        pack_install_provenance = cls(
            artifact_type=artifact_type,
            checksum_verified=checksum_verified,
            fallback_occurred=fallback_occurred,
            artifact_url=artifact_url,
            checksum=checksum,
            checksum_subject=checksum_subject,
            git_ref=git_ref,
            registry_id=registry_id,
            registry_url=registry_url,
            resolved_pack=resolved_pack,
        )

        pack_install_provenance.additional_properties = d
        return pack_install_provenance

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
