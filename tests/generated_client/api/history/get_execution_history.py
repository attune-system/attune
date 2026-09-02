import datetime
from http import HTTPStatus
from typing import Any
from urllib.parse import quote

import httpx

from ... import errors
from ...client import AuthenticatedClient, Client
from ...models.paginated_response_history_record_response import (
    PaginatedResponseHistoryRecordResponse,
)
from ...types import UNSET, Response, Unset


def _get_kwargs(
    id: int,
    *,
    entity_id: int | None | Unset = UNSET,
    entity_ref: None | str | Unset = UNSET,
    operation: None | str | Unset = UNSET,
    changed_field: None | str | Unset = UNSET,
    since: datetime.datetime | None | Unset = UNSET,
    until: datetime.datetime | None | Unset = UNSET,
    page: int | Unset = UNSET,
    page_size: int | Unset = UNSET,
) -> dict[str, Any]:

    params: dict[str, Any] = {}

    json_entity_id: int | None | Unset
    if isinstance(entity_id, Unset):
        json_entity_id = UNSET
    else:
        json_entity_id = entity_id
    params["entity_id"] = json_entity_id

    json_entity_ref: None | str | Unset
    if isinstance(entity_ref, Unset):
        json_entity_ref = UNSET
    else:
        json_entity_ref = entity_ref
    params["entity_ref"] = json_entity_ref

    json_operation: None | str | Unset
    if isinstance(operation, Unset):
        json_operation = UNSET
    else:
        json_operation = operation
    params["operation"] = json_operation

    json_changed_field: None | str | Unset
    if isinstance(changed_field, Unset):
        json_changed_field = UNSET
    else:
        json_changed_field = changed_field
    params["changed_field"] = json_changed_field

    json_since: None | str | Unset
    if isinstance(since, Unset):
        json_since = UNSET
    elif isinstance(since, datetime.datetime):
        json_since = since.isoformat()
    else:
        json_since = since
    params["since"] = json_since

    json_until: None | str | Unset
    if isinstance(until, Unset):
        json_until = UNSET
    elif isinstance(until, datetime.datetime):
        json_until = until.isoformat()
    else:
        json_until = until
    params["until"] = json_until

    params["page"] = page

    params["page_size"] = page_size

    params = {k: v for k, v in params.items() if v is not UNSET and v is not None}

    _kwargs: dict[str, Any] = {
        "method": "get",
        "url": "/api/v1/executions/{id}/history".format(
            id=quote(str(id), safe=""),
        ),
        "params": params,
    }

    return _kwargs


def _parse_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> PaginatedResponseHistoryRecordResponse | None:
    if response.status_code == 200:
        response_200 = PaginatedResponseHistoryRecordResponse.from_dict(response.json())

        return response_200

    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> Response[PaginatedResponseHistoryRecordResponse]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    id: int,
    *,
    client: AuthenticatedClient,
    entity_id: int | None | Unset = UNSET,
    entity_ref: None | str | Unset = UNSET,
    operation: None | str | Unset = UNSET,
    changed_field: None | str | Unset = UNSET,
    since: datetime.datetime | None | Unset = UNSET,
    until: datetime.datetime | None | Unset = UNSET,
    page: int | Unset = UNSET,
    page_size: int | Unset = UNSET,
) -> Response[PaginatedResponseHistoryRecordResponse]:
    """Get history for a specific execution by ID.

     Returns all change records for the given execution, ordered by time descending.

    Args:
        id (int):
        entity_id (int | None | Unset):
        entity_ref (None | str | Unset):
        operation (None | str | Unset):
        changed_field (None | str | Unset):
        since (datetime.datetime | None | Unset):
        until (datetime.datetime | None | Unset):
        page (int | Unset):
        page_size (int | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[PaginatedResponseHistoryRecordResponse]
    """

    kwargs = _get_kwargs(
        id=id,
        entity_id=entity_id,
        entity_ref=entity_ref,
        operation=operation,
        changed_field=changed_field,
        since=since,
        until=until,
        page=page,
        page_size=page_size,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    id: int,
    *,
    client: AuthenticatedClient,
    entity_id: int | None | Unset = UNSET,
    entity_ref: None | str | Unset = UNSET,
    operation: None | str | Unset = UNSET,
    changed_field: None | str | Unset = UNSET,
    since: datetime.datetime | None | Unset = UNSET,
    until: datetime.datetime | None | Unset = UNSET,
    page: int | Unset = UNSET,
    page_size: int | Unset = UNSET,
) -> PaginatedResponseHistoryRecordResponse | None:
    """Get history for a specific execution by ID.

     Returns all change records for the given execution, ordered by time descending.

    Args:
        id (int):
        entity_id (int | None | Unset):
        entity_ref (None | str | Unset):
        operation (None | str | Unset):
        changed_field (None | str | Unset):
        since (datetime.datetime | None | Unset):
        until (datetime.datetime | None | Unset):
        page (int | Unset):
        page_size (int | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        PaginatedResponseHistoryRecordResponse
    """

    return sync_detailed(
        id=id,
        client=client,
        entity_id=entity_id,
        entity_ref=entity_ref,
        operation=operation,
        changed_field=changed_field,
        since=since,
        until=until,
        page=page,
        page_size=page_size,
    ).parsed


async def asyncio_detailed(
    id: int,
    *,
    client: AuthenticatedClient,
    entity_id: int | None | Unset = UNSET,
    entity_ref: None | str | Unset = UNSET,
    operation: None | str | Unset = UNSET,
    changed_field: None | str | Unset = UNSET,
    since: datetime.datetime | None | Unset = UNSET,
    until: datetime.datetime | None | Unset = UNSET,
    page: int | Unset = UNSET,
    page_size: int | Unset = UNSET,
) -> Response[PaginatedResponseHistoryRecordResponse]:
    """Get history for a specific execution by ID.

     Returns all change records for the given execution, ordered by time descending.

    Args:
        id (int):
        entity_id (int | None | Unset):
        entity_ref (None | str | Unset):
        operation (None | str | Unset):
        changed_field (None | str | Unset):
        since (datetime.datetime | None | Unset):
        until (datetime.datetime | None | Unset):
        page (int | Unset):
        page_size (int | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[PaginatedResponseHistoryRecordResponse]
    """

    kwargs = _get_kwargs(
        id=id,
        entity_id=entity_id,
        entity_ref=entity_ref,
        operation=operation,
        changed_field=changed_field,
        since=since,
        until=until,
        page=page,
        page_size=page_size,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    id: int,
    *,
    client: AuthenticatedClient,
    entity_id: int | None | Unset = UNSET,
    entity_ref: None | str | Unset = UNSET,
    operation: None | str | Unset = UNSET,
    changed_field: None | str | Unset = UNSET,
    since: datetime.datetime | None | Unset = UNSET,
    until: datetime.datetime | None | Unset = UNSET,
    page: int | Unset = UNSET,
    page_size: int | Unset = UNSET,
) -> PaginatedResponseHistoryRecordResponse | None:
    """Get history for a specific execution by ID.

     Returns all change records for the given execution, ordered by time descending.

    Args:
        id (int):
        entity_id (int | None | Unset):
        entity_ref (None | str | Unset):
        operation (None | str | Unset):
        changed_field (None | str | Unset):
        since (datetime.datetime | None | Unset):
        until (datetime.datetime | None | Unset):
        page (int | Unset):
        page_size (int | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        PaginatedResponseHistoryRecordResponse
    """

    return (
        await asyncio_detailed(
            id=id,
            client=client,
            entity_id=entity_id,
            entity_ref=entity_ref,
            operation=operation,
            changed_field=changed_field,
            since=since,
            until=until,
            page=page,
            page_size=page_size,
        )
    ).parsed
