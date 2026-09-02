from __future__ import annotations

import datetime
from collections.abc import Mapping
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

from ..types import UNSET, Unset

T = TypeVar("T", bound="ApiResponsePackInstallStatusResponseData")


@_attrs_define
class ApiResponsePackInstallStatusResponseData:
    """Response describing a tracked pack installation attempt.

    Attributes:
        install_id (int): Pack install record id
        pack_ref (str): Pack reference this install attempt belongs to
        pack_version (str): Pack version being installed
        started_at (datetime.datetime): When installation activities started
        status (str): pending, running, activating, succeeded, failed, or rolled_back
        trigger_reason (str): Why the install was triggered (install, update, manual, validation)
        error_message (None | str | Unset): Failure detail, when the install failed
        finished_at (datetime.datetime | None | Unset): When the install reached a terminal state
        test_execution_id (int | None | Unset): ID of the pack_test_execution row produced by the run, when available
        test_result (Any | Unset): Snapshot of the PackTestResult, when available
    """

    install_id: int
    pack_ref: str
    pack_version: str
    started_at: datetime.datetime
    status: str
    trigger_reason: str
    error_message: None | str | Unset = UNSET
    finished_at: datetime.datetime | None | Unset = UNSET
    test_execution_id: int | None | Unset = UNSET
    test_result: Any | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        install_id = self.install_id

        pack_ref = self.pack_ref

        pack_version = self.pack_version

        started_at = self.started_at.isoformat()

        status = self.status

        trigger_reason = self.trigger_reason

        error_message: None | str | Unset
        if isinstance(self.error_message, Unset):
            error_message = UNSET
        else:
            error_message = self.error_message

        finished_at: None | str | Unset
        if isinstance(self.finished_at, Unset):
            finished_at = UNSET
        elif isinstance(self.finished_at, datetime.datetime):
            finished_at = self.finished_at.isoformat()
        else:
            finished_at = self.finished_at

        test_execution_id: int | None | Unset
        if isinstance(self.test_execution_id, Unset):
            test_execution_id = UNSET
        else:
            test_execution_id = self.test_execution_id

        test_result = self.test_result

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "install_id": install_id,
                "pack_ref": pack_ref,
                "pack_version": pack_version,
                "started_at": started_at,
                "status": status,
                "trigger_reason": trigger_reason,
            }
        )
        if error_message is not UNSET:
            field_dict["error_message"] = error_message
        if finished_at is not UNSET:
            field_dict["finished_at"] = finished_at
        if test_execution_id is not UNSET:
            field_dict["test_execution_id"] = test_execution_id
        if test_result is not UNSET:
            field_dict["test_result"] = test_result

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        d = dict(src_dict)
        install_id = d.pop("install_id")

        pack_ref = d.pop("pack_ref")

        pack_version = d.pop("pack_version")

        started_at = datetime.datetime.fromisoformat(d.pop("started_at"))

        status = d.pop("status")

        trigger_reason = d.pop("trigger_reason")

        def _parse_error_message(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        error_message = _parse_error_message(d.pop("error_message", UNSET))

        def _parse_finished_at(data: object) -> datetime.datetime | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                finished_at_type_0 = datetime.datetime.fromisoformat(data)

                return finished_at_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(datetime.datetime | None | Unset, data)

        finished_at = _parse_finished_at(d.pop("finished_at", UNSET))

        def _parse_test_execution_id(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        test_execution_id = _parse_test_execution_id(d.pop("test_execution_id", UNSET))

        test_result = d.pop("test_result", UNSET)

        api_response_pack_install_status_response_data = cls(
            install_id=install_id,
            pack_ref=pack_ref,
            pack_version=pack_version,
            started_at=started_at,
            status=status,
            trigger_reason=trigger_reason,
            error_message=error_message,
            finished_at=finished_at,
            test_execution_id=test_execution_id,
            test_result=test_result,
        )

        api_response_pack_install_status_response_data.additional_properties = d
        return api_response_pack_install_status_response_data

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
