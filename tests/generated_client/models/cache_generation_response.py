from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..models.cache_generation_state import CacheGenerationState
from typing import cast
import datetime






T = TypeVar("T", bound="CacheGenerationResponse")



@_attrs_define
class CacheGenerationResponse:
    """ Immutable generation metadata. Also serves as the refresh-lifecycle
    operation response for create/upload/seal/promote/abandon.

        Attributes:
            activated (datetime.datetime | None):
            checksum (None | str):
            checksum_algorithm (None | str):
            client_refresh_id (str):
            created (datetime.datetime):
            created_by (int | None):
            expected_active_generation_id (int | None):
            expected_chunk_count (int):
            expected_record_count (int | None):
            expected_size_bytes (int | None):
            failed (datetime.datetime | None):
            failure_reason (None | str):
            generation_id (int):
            namespace_id (int):
            readable_until (datetime.datetime | None):
            record_count (int):
            retired (datetime.datetime | None):
            sealed (datetime.datetime | None):
            size_bytes (int):
            source_revision (None | str):
            status (CacheGenerationState): Lifecycle state for an immutable cache generation.
     """

    activated: datetime.datetime | None
    checksum: None | str
    checksum_algorithm: None | str
    client_refresh_id: str
    created: datetime.datetime
    created_by: int | None
    expected_active_generation_id: int | None
    expected_chunk_count: int
    expected_record_count: int | None
    expected_size_bytes: int | None
    failed: datetime.datetime | None
    failure_reason: None | str
    generation_id: int
    namespace_id: int
    readable_until: datetime.datetime | None
    record_count: int
    retired: datetime.datetime | None
    sealed: datetime.datetime | None
    size_bytes: int
    source_revision: None | str
    status: CacheGenerationState
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)





    def to_dict(self) -> dict[str, Any]:
        activated: None | str
        if isinstance(self.activated, datetime.datetime):
            activated = self.activated.isoformat()
        else:
            activated = self.activated

        checksum: None | str
        checksum = self.checksum

        checksum_algorithm: None | str
        checksum_algorithm = self.checksum_algorithm

        client_refresh_id = self.client_refresh_id

        created = self.created.isoformat()

        created_by: int | None
        created_by = self.created_by

        expected_active_generation_id: int | None
        expected_active_generation_id = self.expected_active_generation_id

        expected_chunk_count = self.expected_chunk_count

        expected_record_count: int | None
        expected_record_count = self.expected_record_count

        expected_size_bytes: int | None
        expected_size_bytes = self.expected_size_bytes

        failed: None | str
        if isinstance(self.failed, datetime.datetime):
            failed = self.failed.isoformat()
        else:
            failed = self.failed

        failure_reason: None | str
        failure_reason = self.failure_reason

        generation_id = self.generation_id

        namespace_id = self.namespace_id

        readable_until: None | str
        if isinstance(self.readable_until, datetime.datetime):
            readable_until = self.readable_until.isoformat()
        else:
            readable_until = self.readable_until

        record_count = self.record_count

        retired: None | str
        if isinstance(self.retired, datetime.datetime):
            retired = self.retired.isoformat()
        else:
            retired = self.retired

        sealed: None | str
        if isinstance(self.sealed, datetime.datetime):
            sealed = self.sealed.isoformat()
        else:
            sealed = self.sealed

        size_bytes = self.size_bytes

        source_revision: None | str
        source_revision = self.source_revision

        status = self.status.value


        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update({
            "activated": activated,
            "checksum": checksum,
            "checksum_algorithm": checksum_algorithm,
            "client_refresh_id": client_refresh_id,
            "created": created,
            "created_by": created_by,
            "expected_active_generation_id": expected_active_generation_id,
            "expected_chunk_count": expected_chunk_count,
            "expected_record_count": expected_record_count,
            "expected_size_bytes": expected_size_bytes,
            "failed": failed,
            "failure_reason": failure_reason,
            "generation_id": generation_id,
            "namespace_id": namespace_id,
            "readable_until": readable_until,
            "record_count": record_count,
            "retired": retired,
            "sealed": sealed,
            "size_bytes": size_bytes,
            "source_revision": source_revision,
            "status": status,
        })

        return field_dict



    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        def _parse_activated(data: object) -> datetime.datetime | None:
            if data is None:
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                activated_type_0 = datetime.datetime.fromisoformat(data)



                return activated_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(datetime.datetime | None, data)

        activated = _parse_activated(d.pop("activated"))


        def _parse_checksum(data: object) -> None | str:
            if data is None:
                return data
            return cast(None | str, data)

        checksum = _parse_checksum(d.pop("checksum"))


        def _parse_checksum_algorithm(data: object) -> None | str:
            if data is None:
                return data
            return cast(None | str, data)

        checksum_algorithm = _parse_checksum_algorithm(d.pop("checksum_algorithm"))


        client_refresh_id = d.pop("client_refresh_id")

        created = datetime.datetime.fromisoformat(d.pop("created"))




        def _parse_created_by(data: object) -> int | None:
            if data is None:
                return data
            return cast(int | None, data)

        created_by = _parse_created_by(d.pop("created_by"))


        def _parse_expected_active_generation_id(data: object) -> int | None:
            if data is None:
                return data
            return cast(int | None, data)

        expected_active_generation_id = _parse_expected_active_generation_id(d.pop("expected_active_generation_id"))


        expected_chunk_count = d.pop("expected_chunk_count")

        def _parse_expected_record_count(data: object) -> int | None:
            if data is None:
                return data
            return cast(int | None, data)

        expected_record_count = _parse_expected_record_count(d.pop("expected_record_count"))


        def _parse_expected_size_bytes(data: object) -> int | None:
            if data is None:
                return data
            return cast(int | None, data)

        expected_size_bytes = _parse_expected_size_bytes(d.pop("expected_size_bytes"))


        def _parse_failed(data: object) -> datetime.datetime | None:
            if data is None:
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                failed_type_0 = datetime.datetime.fromisoformat(data)



                return failed_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(datetime.datetime | None, data)

        failed = _parse_failed(d.pop("failed"))


        def _parse_failure_reason(data: object) -> None | str:
            if data is None:
                return data
            return cast(None | str, data)

        failure_reason = _parse_failure_reason(d.pop("failure_reason"))


        generation_id = d.pop("generation_id")

        namespace_id = d.pop("namespace_id")

        def _parse_readable_until(data: object) -> datetime.datetime | None:
            if data is None:
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                readable_until_type_0 = datetime.datetime.fromisoformat(data)



                return readable_until_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(datetime.datetime | None, data)

        readable_until = _parse_readable_until(d.pop("readable_until"))


        record_count = d.pop("record_count")

        def _parse_retired(data: object) -> datetime.datetime | None:
            if data is None:
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                retired_type_0 = datetime.datetime.fromisoformat(data)



                return retired_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(datetime.datetime | None, data)

        retired = _parse_retired(d.pop("retired"))


        def _parse_sealed(data: object) -> datetime.datetime | None:
            if data is None:
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                sealed_type_0 = datetime.datetime.fromisoformat(data)



                return sealed_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(datetime.datetime | None, data)

        sealed = _parse_sealed(d.pop("sealed"))


        size_bytes = d.pop("size_bytes")

        def _parse_source_revision(data: object) -> None | str:
            if data is None:
                return data
            return cast(None | str, data)

        source_revision = _parse_source_revision(d.pop("source_revision"))


        status = CacheGenerationState(d.pop("status"))




        cache_generation_response = cls(
            activated=activated,
            checksum=checksum,
            checksum_algorithm=checksum_algorithm,
            client_refresh_id=client_refresh_id,
            created=created,
            created_by=created_by,
            expected_active_generation_id=expected_active_generation_id,
            expected_chunk_count=expected_chunk_count,
            expected_record_count=expected_record_count,
            expected_size_bytes=expected_size_bytes,
            failed=failed,
            failure_reason=failure_reason,
            generation_id=generation_id,
            namespace_id=namespace_id,
            readable_until=readable_until,
            record_count=record_count,
            retired=retired,
            sealed=sealed,
            size_bytes=size_bytes,
            source_revision=source_revision,
            status=status,
        )


        cache_generation_response.additional_properties = d
        return cache_generation_response

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
