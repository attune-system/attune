from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define
from typing_extensions import Self

T = TypeVar("T", bound="SignKeyJwtRequest")


@_attrs_define
class SignKeyJwtRequest:
    """
    Attributes:
        profile_ref (str):
        subject (str):
        ttl_seconds (int):
    """

    profile_ref: str
    subject: str
    ttl_seconds: int

    def to_dict(self) -> dict[str, Any]:
        profile_ref = self.profile_ref

        subject = self.subject

        ttl_seconds = self.ttl_seconds

        field_dict: dict[str, Any] = {}

        field_dict.update(
            {
                "profile_ref": profile_ref,
                "subject": subject,
                "ttl_seconds": ttl_seconds,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        d = dict(src_dict)
        profile_ref = d.pop("profile_ref")

        subject = d.pop("subject")

        ttl_seconds = d.pop("ttl_seconds")

        sign_key_jwt_request = cls(
            profile_ref=profile_ref,
            subject=subject,
            ttl_seconds=ttl_seconds,
        )

        return sign_key_jwt_request
