import datetime
from http import HTTPStatus
from typing import Any, cast
from uuid import UUID

import httpx

from ... import errors
from ...client import AuthenticatedClient, Client
from ...models.audit_category import AuditCategory
from ...models.audit_outcome import AuditOutcome
from ...models.paginated_response_audit_event_summary import (
    PaginatedResponseAuditEventSummary,
)
from ...types import UNSET, Response, Unset


def _get_kwargs(
    *,
    category: AuditCategory | None | Unset = UNSET,
    event_type: None | str | Unset = UNSET,
    outcome: AuditOutcome | None | Unset = UNSET,
    actor_identity: int | None | Unset = UNSET,
    actor_login: None | str | Unset = UNSET,
    resource_type: None | str | Unset = UNSET,
    resource_id: int | None | Unset = UNSET,
    resource_ref: None | str | Unset = UNSET,
    request_id: None | Unset | UUID = UNSET,
    http_status: int | None | Unset = UNSET,
    http_method: None | str | Unset = UNSET,
    http_path: None | str | Unset = UNSET,
    created_after: datetime.datetime | None | Unset = UNSET,
    created_before: datetime.datetime | None | Unset = UNSET,
    include_total: bool | None | Unset = UNSET,
    page: int | Unset = UNSET,
    per_page: int | Unset = UNSET,
) -> dict[str, Any]:

    params: dict[str, Any] = {}

    json_category: None | str | Unset
    if isinstance(category, Unset):
        json_category = UNSET
    elif isinstance(category, AuditCategory):
        json_category = category.value
    else:
        json_category = category
    params["category"] = json_category

    json_event_type: None | str | Unset
    if isinstance(event_type, Unset):
        json_event_type = UNSET
    else:
        json_event_type = event_type
    params["event_type"] = json_event_type

    json_outcome: None | str | Unset
    if isinstance(outcome, Unset):
        json_outcome = UNSET
    elif isinstance(outcome, AuditOutcome):
        json_outcome = outcome.value
    else:
        json_outcome = outcome
    params["outcome"] = json_outcome

    json_actor_identity: int | None | Unset
    if isinstance(actor_identity, Unset):
        json_actor_identity = UNSET
    else:
        json_actor_identity = actor_identity
    params["actor_identity"] = json_actor_identity

    json_actor_login: None | str | Unset
    if isinstance(actor_login, Unset):
        json_actor_login = UNSET
    else:
        json_actor_login = actor_login
    params["actor_login"] = json_actor_login

    json_resource_type: None | str | Unset
    if isinstance(resource_type, Unset):
        json_resource_type = UNSET
    else:
        json_resource_type = resource_type
    params["resource_type"] = json_resource_type

    json_resource_id: int | None | Unset
    if isinstance(resource_id, Unset):
        json_resource_id = UNSET
    else:
        json_resource_id = resource_id
    params["resource_id"] = json_resource_id

    json_resource_ref: None | str | Unset
    if isinstance(resource_ref, Unset):
        json_resource_ref = UNSET
    else:
        json_resource_ref = resource_ref
    params["resource_ref"] = json_resource_ref

    json_request_id: None | str | Unset
    if isinstance(request_id, Unset):
        json_request_id = UNSET
    elif isinstance(request_id, UUID):
        json_request_id = str(request_id)
    else:
        json_request_id = request_id
    params["request_id"] = json_request_id

    json_http_status: int | None | Unset
    if isinstance(http_status, Unset):
        json_http_status = UNSET
    else:
        json_http_status = http_status
    params["http_status"] = json_http_status

    json_http_method: None | str | Unset
    if isinstance(http_method, Unset):
        json_http_method = UNSET
    else:
        json_http_method = http_method
    params["http_method"] = json_http_method

    json_http_path: None | str | Unset
    if isinstance(http_path, Unset):
        json_http_path = UNSET
    else:
        json_http_path = http_path
    params["http_path"] = json_http_path

    json_created_after: None | str | Unset
    if isinstance(created_after, Unset):
        json_created_after = UNSET
    elif isinstance(created_after, datetime.datetime):
        json_created_after = created_after.isoformat()
    else:
        json_created_after = created_after
    params["created_after"] = json_created_after

    json_created_before: None | str | Unset
    if isinstance(created_before, Unset):
        json_created_before = UNSET
    elif isinstance(created_before, datetime.datetime):
        json_created_before = created_before.isoformat()
    else:
        json_created_before = created_before
    params["created_before"] = json_created_before

    json_include_total: bool | None | Unset
    if isinstance(include_total, Unset):
        json_include_total = UNSET
    else:
        json_include_total = include_total
    params["include_total"] = json_include_total

    params["page"] = page

    params["per_page"] = per_page

    params = {k: v for k, v in params.items() if v is not UNSET and v is not None}

    _kwargs: dict[str, Any] = {
        "method": "get",
        "url": "/api/v1/audit-events",
        "params": params,
    }

    return _kwargs


def _parse_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> Any | PaginatedResponseAuditEventSummary | None:
    if response.status_code == 200:
        response_200 = PaginatedResponseAuditEventSummary.from_dict(response.json())

        return response_200

    if response.status_code == 401:
        response_401 = cast(Any, None)
        return response_401

    if response.status_code == 403:
        response_403 = cast(Any, None)
        return response_403

    if response.status_code == 500:
        response_500 = cast(Any, None)
        return response_500

    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> Response[Any | PaginatedResponseAuditEventSummary]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    *,
    client: AuthenticatedClient,
    category: AuditCategory | None | Unset = UNSET,
    event_type: None | str | Unset = UNSET,
    outcome: AuditOutcome | None | Unset = UNSET,
    actor_identity: int | None | Unset = UNSET,
    actor_login: None | str | Unset = UNSET,
    resource_type: None | str | Unset = UNSET,
    resource_id: int | None | Unset = UNSET,
    resource_ref: None | str | Unset = UNSET,
    request_id: None | Unset | UUID = UNSET,
    http_status: int | None | Unset = UNSET,
    http_method: None | str | Unset = UNSET,
    http_path: None | str | Unset = UNSET,
    created_after: datetime.datetime | None | Unset = UNSET,
    created_before: datetime.datetime | None | Unset = UNSET,
    include_total: bool | None | Unset = UNSET,
    page: int | Unset = UNSET,
    per_page: int | Unset = UNSET,
) -> Response[Any | PaginatedResponseAuditEventSummary]:
    """List audit events with optional filters.

    Args:
        category (AuditCategory | None | Unset):
        event_type (None | str | Unset):
        outcome (AuditOutcome | None | Unset):
        actor_identity (int | None | Unset):
        actor_login (None | str | Unset):
        resource_type (None | str | Unset):
        resource_id (int | None | Unset):
        resource_ref (None | str | Unset):
        request_id (None | Unset | UUID):
        http_status (int | None | Unset):
        http_method (None | str | Unset):
        http_path (None | str | Unset):
        created_after (datetime.datetime | None | Unset):
        created_before (datetime.datetime | None | Unset):
        include_total (bool | None | Unset):
        page (int | Unset):
        per_page (int | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Any | PaginatedResponseAuditEventSummary]
    """

    kwargs = _get_kwargs(
        category=category,
        event_type=event_type,
        outcome=outcome,
        actor_identity=actor_identity,
        actor_login=actor_login,
        resource_type=resource_type,
        resource_id=resource_id,
        resource_ref=resource_ref,
        request_id=request_id,
        http_status=http_status,
        http_method=http_method,
        http_path=http_path,
        created_after=created_after,
        created_before=created_before,
        include_total=include_total,
        page=page,
        per_page=per_page,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    *,
    client: AuthenticatedClient,
    category: AuditCategory | None | Unset = UNSET,
    event_type: None | str | Unset = UNSET,
    outcome: AuditOutcome | None | Unset = UNSET,
    actor_identity: int | None | Unset = UNSET,
    actor_login: None | str | Unset = UNSET,
    resource_type: None | str | Unset = UNSET,
    resource_id: int | None | Unset = UNSET,
    resource_ref: None | str | Unset = UNSET,
    request_id: None | Unset | UUID = UNSET,
    http_status: int | None | Unset = UNSET,
    http_method: None | str | Unset = UNSET,
    http_path: None | str | Unset = UNSET,
    created_after: datetime.datetime | None | Unset = UNSET,
    created_before: datetime.datetime | None | Unset = UNSET,
    include_total: bool | None | Unset = UNSET,
    page: int | Unset = UNSET,
    per_page: int | Unset = UNSET,
) -> Any | PaginatedResponseAuditEventSummary | None:
    """List audit events with optional filters.

    Args:
        category (AuditCategory | None | Unset):
        event_type (None | str | Unset):
        outcome (AuditOutcome | None | Unset):
        actor_identity (int | None | Unset):
        actor_login (None | str | Unset):
        resource_type (None | str | Unset):
        resource_id (int | None | Unset):
        resource_ref (None | str | Unset):
        request_id (None | Unset | UUID):
        http_status (int | None | Unset):
        http_method (None | str | Unset):
        http_path (None | str | Unset):
        created_after (datetime.datetime | None | Unset):
        created_before (datetime.datetime | None | Unset):
        include_total (bool | None | Unset):
        page (int | Unset):
        per_page (int | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Any | PaginatedResponseAuditEventSummary
    """

    return sync_detailed(
        client=client,
        category=category,
        event_type=event_type,
        outcome=outcome,
        actor_identity=actor_identity,
        actor_login=actor_login,
        resource_type=resource_type,
        resource_id=resource_id,
        resource_ref=resource_ref,
        request_id=request_id,
        http_status=http_status,
        http_method=http_method,
        http_path=http_path,
        created_after=created_after,
        created_before=created_before,
        include_total=include_total,
        page=page,
        per_page=per_page,
    ).parsed


async def asyncio_detailed(
    *,
    client: AuthenticatedClient,
    category: AuditCategory | None | Unset = UNSET,
    event_type: None | str | Unset = UNSET,
    outcome: AuditOutcome | None | Unset = UNSET,
    actor_identity: int | None | Unset = UNSET,
    actor_login: None | str | Unset = UNSET,
    resource_type: None | str | Unset = UNSET,
    resource_id: int | None | Unset = UNSET,
    resource_ref: None | str | Unset = UNSET,
    request_id: None | Unset | UUID = UNSET,
    http_status: int | None | Unset = UNSET,
    http_method: None | str | Unset = UNSET,
    http_path: None | str | Unset = UNSET,
    created_after: datetime.datetime | None | Unset = UNSET,
    created_before: datetime.datetime | None | Unset = UNSET,
    include_total: bool | None | Unset = UNSET,
    page: int | Unset = UNSET,
    per_page: int | Unset = UNSET,
) -> Response[Any | PaginatedResponseAuditEventSummary]:
    """List audit events with optional filters.

    Args:
        category (AuditCategory | None | Unset):
        event_type (None | str | Unset):
        outcome (AuditOutcome | None | Unset):
        actor_identity (int | None | Unset):
        actor_login (None | str | Unset):
        resource_type (None | str | Unset):
        resource_id (int | None | Unset):
        resource_ref (None | str | Unset):
        request_id (None | Unset | UUID):
        http_status (int | None | Unset):
        http_method (None | str | Unset):
        http_path (None | str | Unset):
        created_after (datetime.datetime | None | Unset):
        created_before (datetime.datetime | None | Unset):
        include_total (bool | None | Unset):
        page (int | Unset):
        per_page (int | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Any | PaginatedResponseAuditEventSummary]
    """

    kwargs = _get_kwargs(
        category=category,
        event_type=event_type,
        outcome=outcome,
        actor_identity=actor_identity,
        actor_login=actor_login,
        resource_type=resource_type,
        resource_id=resource_id,
        resource_ref=resource_ref,
        request_id=request_id,
        http_status=http_status,
        http_method=http_method,
        http_path=http_path,
        created_after=created_after,
        created_before=created_before,
        include_total=include_total,
        page=page,
        per_page=per_page,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    *,
    client: AuthenticatedClient,
    category: AuditCategory | None | Unset = UNSET,
    event_type: None | str | Unset = UNSET,
    outcome: AuditOutcome | None | Unset = UNSET,
    actor_identity: int | None | Unset = UNSET,
    actor_login: None | str | Unset = UNSET,
    resource_type: None | str | Unset = UNSET,
    resource_id: int | None | Unset = UNSET,
    resource_ref: None | str | Unset = UNSET,
    request_id: None | Unset | UUID = UNSET,
    http_status: int | None | Unset = UNSET,
    http_method: None | str | Unset = UNSET,
    http_path: None | str | Unset = UNSET,
    created_after: datetime.datetime | None | Unset = UNSET,
    created_before: datetime.datetime | None | Unset = UNSET,
    include_total: bool | None | Unset = UNSET,
    page: int | Unset = UNSET,
    per_page: int | Unset = UNSET,
) -> Any | PaginatedResponseAuditEventSummary | None:
    """List audit events with optional filters.

    Args:
        category (AuditCategory | None | Unset):
        event_type (None | str | Unset):
        outcome (AuditOutcome | None | Unset):
        actor_identity (int | None | Unset):
        actor_login (None | str | Unset):
        resource_type (None | str | Unset):
        resource_id (int | None | Unset):
        resource_ref (None | str | Unset):
        request_id (None | Unset | UUID):
        http_status (int | None | Unset):
        http_method (None | str | Unset):
        http_path (None | str | Unset):
        created_after (datetime.datetime | None | Unset):
        created_before (datetime.datetime | None | Unset):
        include_total (bool | None | Unset):
        page (int | Unset):
        per_page (int | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Any | PaginatedResponseAuditEventSummary
    """

    return (
        await asyncio_detailed(
            client=client,
            category=category,
            event_type=event_type,
            outcome=outcome,
            actor_identity=actor_identity,
            actor_login=actor_login,
            resource_type=resource_type,
            resource_id=resource_id,
            resource_ref=resource_ref,
            request_id=request_id,
            http_status=http_status,
            http_method=http_method,
            http_path=http_path,
            created_after=created_after,
            created_before=created_before,
            include_total=include_total,
            page=page,
            per_page=per_page,
        )
    ).parsed
