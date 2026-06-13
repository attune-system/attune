from __future__ import annotations

import datetime
from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.worker_health_state import WorkerHealthState
from ..models.worker_role import WorkerRole
from ..models.worker_status import WorkerStatus
from ..models.worker_type import WorkerType
from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.worker_load_snapshot import WorkerLoadSnapshot
    from ..models.worker_runtime_support import WorkerRuntimeSupport


T = TypeVar("T", bound="PaginatedResponseWorkerSummaryItemsItem")


@_attrs_define
class PaginatedResponseWorkerSummaryItemsItem:
    """
    Attributes:
        cordoned (bool):
        created (datetime.datetime):  Example: 2026-04-11T13:26:37Z.
        health_state (WorkerHealthState):
        heartbeat_stale (bool):
        id (int):  Example: 1.
        load (WorkerLoadSnapshot):
        name (str):  Example: worker-build-01.
        supported_runtimes (list[WorkerRuntimeSupport]):
        updated (datetime.datetime):  Example: 2026-04-11T13:26:37Z.
        worker_role (WorkerRole):
        worker_type (WorkerType):
        cordon_reason (None | str | Unset):
        cordoned_at (datetime.datetime | None | Unset):  Example: 2026-04-11T13:26:37Z.
        cordoned_by (int | None | Unset):  Example: 1.
        heartbeat_age_seconds (int | None | Unset):  Example: 42.
        host (None | str | Unset):  Example: worker-build-01.
        last_heartbeat (datetime.datetime | None | Unset):  Example: 2026-04-11T13:26:37Z.
        port (int | None | Unset):  Example: 8082.
        status (None | Unset | WorkerStatus):
    """

    cordoned: bool
    created: datetime.datetime
    health_state: WorkerHealthState
    heartbeat_stale: bool
    id: int
    load: WorkerLoadSnapshot
    name: str
    supported_runtimes: list[WorkerRuntimeSupport]
    updated: datetime.datetime
    worker_role: WorkerRole
    worker_type: WorkerType
    cordon_reason: None | str | Unset = UNSET
    cordoned_at: datetime.datetime | None | Unset = UNSET
    cordoned_by: int | None | Unset = UNSET
    heartbeat_age_seconds: int | None | Unset = UNSET
    host: None | str | Unset = UNSET
    last_heartbeat: datetime.datetime | None | Unset = UNSET
    port: int | None | Unset = UNSET
    status: None | Unset | WorkerStatus = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        cordoned = self.cordoned

        created = self.created.isoformat()

        health_state = self.health_state.value

        heartbeat_stale = self.heartbeat_stale

        id = self.id

        load = self.load.to_dict()

        name = self.name

        supported_runtimes = []
        for supported_runtimes_item_data in self.supported_runtimes:
            supported_runtimes_item = supported_runtimes_item_data.to_dict()
            supported_runtimes.append(supported_runtimes_item)

        updated = self.updated.isoformat()

        worker_role = self.worker_role.value

        worker_type = self.worker_type.value

        cordon_reason: None | str | Unset
        if isinstance(self.cordon_reason, Unset):
            cordon_reason = UNSET
        else:
            cordon_reason = self.cordon_reason

        cordoned_at: None | str | Unset
        if isinstance(self.cordoned_at, Unset):
            cordoned_at = UNSET
        elif isinstance(self.cordoned_at, datetime.datetime):
            cordoned_at = self.cordoned_at.isoformat()
        else:
            cordoned_at = self.cordoned_at

        cordoned_by: int | None | Unset
        if isinstance(self.cordoned_by, Unset):
            cordoned_by = UNSET
        else:
            cordoned_by = self.cordoned_by

        heartbeat_age_seconds: int | None | Unset
        if isinstance(self.heartbeat_age_seconds, Unset):
            heartbeat_age_seconds = UNSET
        else:
            heartbeat_age_seconds = self.heartbeat_age_seconds

        host: None | str | Unset
        if isinstance(self.host, Unset):
            host = UNSET
        else:
            host = self.host

        last_heartbeat: None | str | Unset
        if isinstance(self.last_heartbeat, Unset):
            last_heartbeat = UNSET
        elif isinstance(self.last_heartbeat, datetime.datetime):
            last_heartbeat = self.last_heartbeat.isoformat()
        else:
            last_heartbeat = self.last_heartbeat

        port: int | None | Unset
        if isinstance(self.port, Unset):
            port = UNSET
        else:
            port = self.port

        status: None | str | Unset
        if isinstance(self.status, Unset):
            status = UNSET
        elif isinstance(self.status, WorkerStatus):
            status = self.status.value
        else:
            status = self.status

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "cordoned": cordoned,
                "created": created,
                "health_state": health_state,
                "heartbeat_stale": heartbeat_stale,
                "id": id,
                "load": load,
                "name": name,
                "supported_runtimes": supported_runtimes,
                "updated": updated,
                "worker_role": worker_role,
                "worker_type": worker_type,
            }
        )
        if cordon_reason is not UNSET:
            field_dict["cordon_reason"] = cordon_reason
        if cordoned_at is not UNSET:
            field_dict["cordoned_at"] = cordoned_at
        if cordoned_by is not UNSET:
            field_dict["cordoned_by"] = cordoned_by
        if heartbeat_age_seconds is not UNSET:
            field_dict["heartbeat_age_seconds"] = heartbeat_age_seconds
        if host is not UNSET:
            field_dict["host"] = host
        if last_heartbeat is not UNSET:
            field_dict["last_heartbeat"] = last_heartbeat
        if port is not UNSET:
            field_dict["port"] = port
        if status is not UNSET:
            field_dict["status"] = status

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.worker_load_snapshot import WorkerLoadSnapshot
        from ..models.worker_runtime_support import WorkerRuntimeSupport

        d = dict(src_dict)
        cordoned = d.pop("cordoned")

        created = datetime.datetime.fromisoformat(d.pop("created"))

        health_state = WorkerHealthState(d.pop("health_state"))

        heartbeat_stale = d.pop("heartbeat_stale")

        id = d.pop("id")

        load = WorkerLoadSnapshot.from_dict(d.pop("load"))

        name = d.pop("name")

        supported_runtimes = []
        _supported_runtimes = d.pop("supported_runtimes")
        for supported_runtimes_item_data in _supported_runtimes:
            supported_runtimes_item = WorkerRuntimeSupport.from_dict(
                supported_runtimes_item_data
            )

            supported_runtimes.append(supported_runtimes_item)

        updated = datetime.datetime.fromisoformat(d.pop("updated"))

        worker_role = WorkerRole(d.pop("worker_role"))

        worker_type = WorkerType(d.pop("worker_type"))

        def _parse_cordon_reason(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        cordon_reason = _parse_cordon_reason(d.pop("cordon_reason", UNSET))

        def _parse_cordoned_at(data: object) -> datetime.datetime | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                cordoned_at_type_0 = datetime.datetime.fromisoformat(data)

                return cordoned_at_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(datetime.datetime | None | Unset, data)

        cordoned_at = _parse_cordoned_at(d.pop("cordoned_at", UNSET))

        def _parse_cordoned_by(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        cordoned_by = _parse_cordoned_by(d.pop("cordoned_by", UNSET))

        def _parse_heartbeat_age_seconds(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        heartbeat_age_seconds = _parse_heartbeat_age_seconds(
            d.pop("heartbeat_age_seconds", UNSET)
        )

        def _parse_host(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        host = _parse_host(d.pop("host", UNSET))

        def _parse_last_heartbeat(data: object) -> datetime.datetime | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                last_heartbeat_type_0 = datetime.datetime.fromisoformat(data)

                return last_heartbeat_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(datetime.datetime | None | Unset, data)

        last_heartbeat = _parse_last_heartbeat(d.pop("last_heartbeat", UNSET))

        def _parse_port(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        port = _parse_port(d.pop("port", UNSET))

        def _parse_status(data: object) -> None | Unset | WorkerStatus:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                status_type_1 = WorkerStatus(data)

                return status_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(None | Unset | WorkerStatus, data)

        status = _parse_status(d.pop("status", UNSET))

        paginated_response_worker_summary_items_item = cls(
            cordoned=cordoned,
            created=created,
            health_state=health_state,
            heartbeat_stale=heartbeat_stale,
            id=id,
            load=load,
            name=name,
            supported_runtimes=supported_runtimes,
            updated=updated,
            worker_role=worker_role,
            worker_type=worker_type,
            cordon_reason=cordon_reason,
            cordoned_at=cordoned_at,
            cordoned_by=cordoned_by,
            heartbeat_age_seconds=heartbeat_age_seconds,
            host=host,
            last_heartbeat=last_heartbeat,
            port=port,
            status=status,
        )

        paginated_response_worker_summary_items_item.additional_properties = d
        return paginated_response_worker_summary_items_item

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
