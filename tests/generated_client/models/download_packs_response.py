from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

if TYPE_CHECKING:
    from ..models.downloaded_pack import DownloadedPack
    from ..models.failed_pack import FailedPack


T = TypeVar("T", bound="DownloadPacksResponse")


@_attrs_define
class DownloadPacksResponse:
    """Response DTO for download packs operation

    Attributes:
        downloaded_packs (list[DownloadedPack]): Successfully downloaded packs
        failed_packs (list[FailedPack]): Failed pack downloads
        failure_count (int): Number of failed downloads
        success_count (int): Number of successful downloads
        total_count (int): Total number of packs requested
    """

    downloaded_packs: list[DownloadedPack]
    failed_packs: list[FailedPack]
    failure_count: int
    success_count: int
    total_count: int
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        downloaded_packs = []
        for downloaded_packs_item_data in self.downloaded_packs:
            downloaded_packs_item = downloaded_packs_item_data.to_dict()
            downloaded_packs.append(downloaded_packs_item)

        failed_packs = []
        for failed_packs_item_data in self.failed_packs:
            failed_packs_item = failed_packs_item_data.to_dict()
            failed_packs.append(failed_packs_item)

        failure_count = self.failure_count

        success_count = self.success_count

        total_count = self.total_count

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "downloaded_packs": downloaded_packs,
                "failed_packs": failed_packs,
                "failure_count": failure_count,
                "success_count": success_count,
                "total_count": total_count,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        from ..models.downloaded_pack import DownloadedPack
        from ..models.failed_pack import FailedPack

        d = dict(src_dict)
        downloaded_packs = []
        _downloaded_packs = d.pop("downloaded_packs")
        for downloaded_packs_item_data in _downloaded_packs:
            downloaded_packs_item = DownloadedPack.from_dict(downloaded_packs_item_data)

            downloaded_packs.append(downloaded_packs_item)

        failed_packs = []
        _failed_packs = d.pop("failed_packs")
        for failed_packs_item_data in _failed_packs:
            failed_packs_item = FailedPack.from_dict(failed_packs_item_data)

            failed_packs.append(failed_packs_item)

        failure_count = d.pop("failure_count")

        success_count = d.pop("success_count")

        total_count = d.pop("total_count")

        download_packs_response = cls(
            downloaded_packs=downloaded_packs,
            failed_packs=failed_packs,
            failure_count=failure_count,
            success_count=success_count,
            total_count=total_count,
        )

        download_packs_response.additional_properties = d
        return download_packs_response

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
