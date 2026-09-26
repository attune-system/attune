from __future__ import annotations

import datetime
from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from typing_extensions import Self

from ..models.inquiry_status import InquiryStatus
from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.api_response_inquiry_response_data_response_schema_type_0 import (
        ApiResponseInquiryResponseDataResponseSchemaType0,
    )
    from ..models.api_response_inquiry_response_data_response_type_0 import (
        ApiResponseInquiryResponseDataResponseType0,
    )
    from ..models.inquiry_response_option import InquiryResponseOption


T = TypeVar("T", bound="ApiResponseInquiryResponseData")


@_attrs_define
class ApiResponseInquiryResponseData:
    """Full inquiry response with all details

    Attributes:
        created (datetime.datetime): Creation timestamp Example: 2024-01-13T10:30:00Z.
        created_by_execution (int):
        id (int):
        prompt (str): Prompt text displayed to the user Example: Approve deployment to production?.
        response (ApiResponseInquiryResponseDataResponseType0 | None): Response data provided by the user
        response_options (list[InquiryResponseOption]): Fixed responses that provider controls may select.
        response_schema (ApiResponseInquiryResponseDataResponseSchemaType0 | None): Attune flat schema for expected
            response fields
        status (InquiryStatus):
        updated (datetime.datetime): Last update timestamp Example: 2024-01-13T10:45:00Z.
        assigned_to (int | None | Unset):
        assigned_to_display_name (None | str | Unset):
        assigned_to_login (None | str | Unset):
        created_by_action_ref (None | str | Unset):
        created_by_pack_ref (None | str | Unset):
        purpose (None | str | Unset):
        responded_at (datetime.datetime | None | Unset): When the inquiry was responded to Example:
            2024-01-13T10:45:00Z.
        responded_by (int | None | Unset):
        responded_by_display_name (None | str | Unset):
        responded_by_login (None | str | Unset):
        timeout_at (datetime.datetime | None | Unset): When the inquiry expires Example: 2024-01-13T11:30:00Z.
        workflow_action_ref (None | str | Unset):
        workflow_execution (int | None | Unset):
        workflow_pack_ref (None | str | Unset):
        workflow_root_execution (int | None | Unset):
        workflow_task_name (None | str | Unset):
    """

    created: datetime.datetime
    created_by_execution: int
    id: int
    prompt: str
    response: ApiResponseInquiryResponseDataResponseType0 | None
    response_options: list[InquiryResponseOption]
    response_schema: ApiResponseInquiryResponseDataResponseSchemaType0 | None
    status: InquiryStatus
    updated: datetime.datetime
    assigned_to: int | None | Unset = UNSET
    assigned_to_display_name: None | str | Unset = UNSET
    assigned_to_login: None | str | Unset = UNSET
    created_by_action_ref: None | str | Unset = UNSET
    created_by_pack_ref: None | str | Unset = UNSET
    purpose: None | str | Unset = UNSET
    responded_at: datetime.datetime | None | Unset = UNSET
    responded_by: int | None | Unset = UNSET
    responded_by_display_name: None | str | Unset = UNSET
    responded_by_login: None | str | Unset = UNSET
    timeout_at: datetime.datetime | None | Unset = UNSET
    workflow_action_ref: None | str | Unset = UNSET
    workflow_execution: int | None | Unset = UNSET
    workflow_pack_ref: None | str | Unset = UNSET
    workflow_root_execution: int | None | Unset = UNSET
    workflow_task_name: None | str | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.api_response_inquiry_response_data_response_schema_type_0 import (
            ApiResponseInquiryResponseDataResponseSchemaType0,
        )
        from ..models.api_response_inquiry_response_data_response_type_0 import (
            ApiResponseInquiryResponseDataResponseType0,
        )

        created = self.created.isoformat()

        created_by_execution = self.created_by_execution

        id = self.id

        prompt = self.prompt

        response: dict[str, Any] | None
        if isinstance(self.response, ApiResponseInquiryResponseDataResponseType0):
            response = self.response.to_dict()
        else:
            response = self.response

        response_options = []
        for response_options_item_data in self.response_options:
            response_options_item = response_options_item_data.to_dict()
            response_options.append(response_options_item)

        response_schema: dict[str, Any] | None
        if isinstance(
            self.response_schema, ApiResponseInquiryResponseDataResponseSchemaType0
        ):
            response_schema = self.response_schema.to_dict()
        else:
            response_schema = self.response_schema

        status = self.status.value

        updated = self.updated.isoformat()

        assigned_to: int | None | Unset
        if isinstance(self.assigned_to, Unset):
            assigned_to = UNSET
        else:
            assigned_to = self.assigned_to

        assigned_to_display_name: None | str | Unset
        if isinstance(self.assigned_to_display_name, Unset):
            assigned_to_display_name = UNSET
        else:
            assigned_to_display_name = self.assigned_to_display_name

        assigned_to_login: None | str | Unset
        if isinstance(self.assigned_to_login, Unset):
            assigned_to_login = UNSET
        else:
            assigned_to_login = self.assigned_to_login

        created_by_action_ref: None | str | Unset
        if isinstance(self.created_by_action_ref, Unset):
            created_by_action_ref = UNSET
        else:
            created_by_action_ref = self.created_by_action_ref

        created_by_pack_ref: None | str | Unset
        if isinstance(self.created_by_pack_ref, Unset):
            created_by_pack_ref = UNSET
        else:
            created_by_pack_ref = self.created_by_pack_ref

        purpose: None | str | Unset
        if isinstance(self.purpose, Unset):
            purpose = UNSET
        else:
            purpose = self.purpose

        responded_at: None | str | Unset
        if isinstance(self.responded_at, Unset):
            responded_at = UNSET
        elif isinstance(self.responded_at, datetime.datetime):
            responded_at = self.responded_at.isoformat()
        else:
            responded_at = self.responded_at

        responded_by: int | None | Unset
        if isinstance(self.responded_by, Unset):
            responded_by = UNSET
        else:
            responded_by = self.responded_by

        responded_by_display_name: None | str | Unset
        if isinstance(self.responded_by_display_name, Unset):
            responded_by_display_name = UNSET
        else:
            responded_by_display_name = self.responded_by_display_name

        responded_by_login: None | str | Unset
        if isinstance(self.responded_by_login, Unset):
            responded_by_login = UNSET
        else:
            responded_by_login = self.responded_by_login

        timeout_at: None | str | Unset
        if isinstance(self.timeout_at, Unset):
            timeout_at = UNSET
        elif isinstance(self.timeout_at, datetime.datetime):
            timeout_at = self.timeout_at.isoformat()
        else:
            timeout_at = self.timeout_at

        workflow_action_ref: None | str | Unset
        if isinstance(self.workflow_action_ref, Unset):
            workflow_action_ref = UNSET
        else:
            workflow_action_ref = self.workflow_action_ref

        workflow_execution: int | None | Unset
        if isinstance(self.workflow_execution, Unset):
            workflow_execution = UNSET
        else:
            workflow_execution = self.workflow_execution

        workflow_pack_ref: None | str | Unset
        if isinstance(self.workflow_pack_ref, Unset):
            workflow_pack_ref = UNSET
        else:
            workflow_pack_ref = self.workflow_pack_ref

        workflow_root_execution: int | None | Unset
        if isinstance(self.workflow_root_execution, Unset):
            workflow_root_execution = UNSET
        else:
            workflow_root_execution = self.workflow_root_execution

        workflow_task_name: None | str | Unset
        if isinstance(self.workflow_task_name, Unset):
            workflow_task_name = UNSET
        else:
            workflow_task_name = self.workflow_task_name

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "created": created,
                "created_by_execution": created_by_execution,
                "id": id,
                "prompt": prompt,
                "response": response,
                "response_options": response_options,
                "response_schema": response_schema,
                "status": status,
                "updated": updated,
            }
        )
        if assigned_to is not UNSET:
            field_dict["assigned_to"] = assigned_to
        if assigned_to_display_name is not UNSET:
            field_dict["assigned_to_display_name"] = assigned_to_display_name
        if assigned_to_login is not UNSET:
            field_dict["assigned_to_login"] = assigned_to_login
        if created_by_action_ref is not UNSET:
            field_dict["created_by_action_ref"] = created_by_action_ref
        if created_by_pack_ref is not UNSET:
            field_dict["created_by_pack_ref"] = created_by_pack_ref
        if purpose is not UNSET:
            field_dict["purpose"] = purpose
        if responded_at is not UNSET:
            field_dict["responded_at"] = responded_at
        if responded_by is not UNSET:
            field_dict["responded_by"] = responded_by
        if responded_by_display_name is not UNSET:
            field_dict["responded_by_display_name"] = responded_by_display_name
        if responded_by_login is not UNSET:
            field_dict["responded_by_login"] = responded_by_login
        if timeout_at is not UNSET:
            field_dict["timeout_at"] = timeout_at
        if workflow_action_ref is not UNSET:
            field_dict["workflow_action_ref"] = workflow_action_ref
        if workflow_execution is not UNSET:
            field_dict["workflow_execution"] = workflow_execution
        if workflow_pack_ref is not UNSET:
            field_dict["workflow_pack_ref"] = workflow_pack_ref
        if workflow_root_execution is not UNSET:
            field_dict["workflow_root_execution"] = workflow_root_execution
        if workflow_task_name is not UNSET:
            field_dict["workflow_task_name"] = workflow_task_name

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        from ..models.api_response_inquiry_response_data_response_schema_type_0 import (
            ApiResponseInquiryResponseDataResponseSchemaType0,
        )
        from ..models.api_response_inquiry_response_data_response_type_0 import (
            ApiResponseInquiryResponseDataResponseType0,
        )
        from ..models.inquiry_response_option import (
            InquiryResponseOption,
        )

        d = dict(src_dict)
        created = datetime.datetime.fromisoformat(d.pop("created"))

        created_by_execution = d.pop("created_by_execution")

        id = d.pop("id")

        prompt = d.pop("prompt")

        def _parse_response(
            data: object,
        ) -> ApiResponseInquiryResponseDataResponseType0 | None:
            if data is None:
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                response_type_0 = ApiResponseInquiryResponseDataResponseType0.from_dict(
                    data
                )

                return response_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(ApiResponseInquiryResponseDataResponseType0 | None, data)

        response = _parse_response(d.pop("response"))

        response_options = []
        _response_options = d.pop("response_options")
        for response_options_item_data in _response_options:
            response_options_item = InquiryResponseOption.from_dict(
                response_options_item_data
            )

            response_options.append(response_options_item)

        def _parse_response_schema(
            data: object,
        ) -> ApiResponseInquiryResponseDataResponseSchemaType0 | None:
            if data is None:
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                response_schema_type_0 = (
                    ApiResponseInquiryResponseDataResponseSchemaType0.from_dict(data)
                )

                return response_schema_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(ApiResponseInquiryResponseDataResponseSchemaType0 | None, data)

        response_schema = _parse_response_schema(d.pop("response_schema"))

        status = InquiryStatus(d.pop("status"))

        updated = datetime.datetime.fromisoformat(d.pop("updated"))

        def _parse_assigned_to(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        assigned_to = _parse_assigned_to(d.pop("assigned_to", UNSET))

        def _parse_assigned_to_display_name(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        assigned_to_display_name = _parse_assigned_to_display_name(
            d.pop("assigned_to_display_name", UNSET)
        )

        def _parse_assigned_to_login(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        assigned_to_login = _parse_assigned_to_login(d.pop("assigned_to_login", UNSET))

        def _parse_created_by_action_ref(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        created_by_action_ref = _parse_created_by_action_ref(
            d.pop("created_by_action_ref", UNSET)
        )

        def _parse_created_by_pack_ref(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        created_by_pack_ref = _parse_created_by_pack_ref(
            d.pop("created_by_pack_ref", UNSET)
        )

        def _parse_purpose(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        purpose = _parse_purpose(d.pop("purpose", UNSET))

        def _parse_responded_at(data: object) -> datetime.datetime | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                responded_at_type_0 = datetime.datetime.fromisoformat(data)

                return responded_at_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(datetime.datetime | None | Unset, data)

        responded_at = _parse_responded_at(d.pop("responded_at", UNSET))

        def _parse_responded_by(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        responded_by = _parse_responded_by(d.pop("responded_by", UNSET))

        def _parse_responded_by_display_name(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        responded_by_display_name = _parse_responded_by_display_name(
            d.pop("responded_by_display_name", UNSET)
        )

        def _parse_responded_by_login(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        responded_by_login = _parse_responded_by_login(
            d.pop("responded_by_login", UNSET)
        )

        def _parse_timeout_at(data: object) -> datetime.datetime | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                timeout_at_type_0 = datetime.datetime.fromisoformat(data)

                return timeout_at_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(datetime.datetime | None | Unset, data)

        timeout_at = _parse_timeout_at(d.pop("timeout_at", UNSET))

        def _parse_workflow_action_ref(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        workflow_action_ref = _parse_workflow_action_ref(
            d.pop("workflow_action_ref", UNSET)
        )

        def _parse_workflow_execution(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        workflow_execution = _parse_workflow_execution(
            d.pop("workflow_execution", UNSET)
        )

        def _parse_workflow_pack_ref(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        workflow_pack_ref = _parse_workflow_pack_ref(d.pop("workflow_pack_ref", UNSET))

        def _parse_workflow_root_execution(data: object) -> int | None | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(int | None | Unset, data)

        workflow_root_execution = _parse_workflow_root_execution(
            d.pop("workflow_root_execution", UNSET)
        )

        def _parse_workflow_task_name(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        workflow_task_name = _parse_workflow_task_name(
            d.pop("workflow_task_name", UNSET)
        )

        api_response_inquiry_response_data = cls(
            created=created,
            created_by_execution=created_by_execution,
            id=id,
            prompt=prompt,
            response=response,
            response_options=response_options,
            response_schema=response_schema,
            status=status,
            updated=updated,
            assigned_to=assigned_to,
            assigned_to_display_name=assigned_to_display_name,
            assigned_to_login=assigned_to_login,
            created_by_action_ref=created_by_action_ref,
            created_by_pack_ref=created_by_pack_ref,
            purpose=purpose,
            responded_at=responded_at,
            responded_by=responded_by,
            responded_by_display_name=responded_by_display_name,
            responded_by_login=responded_by_login,
            timeout_at=timeout_at,
            workflow_action_ref=workflow_action_ref,
            workflow_execution=workflow_execution,
            workflow_pack_ref=workflow_pack_ref,
            workflow_root_execution=workflow_root_execution,
            workflow_task_name=workflow_task_name,
        )

        api_response_inquiry_response_data.additional_properties = d
        return api_response_inquiry_response_data

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
