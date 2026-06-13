from http import HTTPStatus
from typing import Any

import httpx

from ... import errors
from ...client import AuthenticatedClient, Client
from ...models.paginated_response_worker_summary import PaginatedResponseWorkerSummary
from ...models.worker_health_state import WorkerHealthState
from ...models.worker_role import WorkerRole
from ...models.worker_status import WorkerStatus
from ...types import UNSET, Response, Unset


def _get_kwargs(
    *,
    page: int | Unset = UNSET,
    page_size: int | Unset = UNSET,
    role: None | Unset | WorkerRole = UNSET,
    status: None | Unset | WorkerStatus = UNSET,
    cordoned: bool | None | Unset = UNSET,
    health_state: None | Unset | WorkerHealthState = UNSET,
) -> dict[str, Any]:

    params: dict[str, Any] = {}

    params["page"] = page

    params["page_size"] = page_size

    json_role: None | str | Unset
    if isinstance(role, Unset):
        json_role = UNSET
    elif isinstance(role, WorkerRole):
        json_role = role.value
    else:
        json_role = role
    params["role"] = json_role

    json_status: None | str | Unset
    if isinstance(status, Unset):
        json_status = UNSET
    elif isinstance(status, WorkerStatus):
        json_status = status.value
    else:
        json_status = status
    params["status"] = json_status

    json_cordoned: bool | None | Unset
    if isinstance(cordoned, Unset):
        json_cordoned = UNSET
    else:
        json_cordoned = cordoned
    params["cordoned"] = json_cordoned

    json_health_state: None | str | Unset
    if isinstance(health_state, Unset):
        json_health_state = UNSET
    elif isinstance(health_state, WorkerHealthState):
        json_health_state = health_state.value
    else:
        json_health_state = health_state
    params["health_state"] = json_health_state

    params = {k: v for k, v in params.items() if v is not UNSET and v is not None}

    _kwargs: dict[str, Any] = {
        "method": "get",
        "url": "/api/v1/workers",
        "params": params,
    }

    return _kwargs


def _parse_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> PaginatedResponseWorkerSummary | None:
    if response.status_code == 200:
        response_200 = PaginatedResponseWorkerSummary.from_dict(response.json())

        return response_200

    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> Response[PaginatedResponseWorkerSummary]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    *,
    client: AuthenticatedClient,
    page: int | Unset = UNSET,
    page_size: int | Unset = UNSET,
    role: None | Unset | WorkerRole = UNSET,
    status: None | Unset | WorkerStatus = UNSET,
    cordoned: bool | None | Unset = UNSET,
    health_state: None | Unset | WorkerHealthState = UNSET,
) -> Response[PaginatedResponseWorkerSummary]:
    """
    Args:
        page (int | Unset):
        page_size (int | Unset):
        role (None | Unset | WorkerRole):
        status (None | Unset | WorkerStatus):
        cordoned (bool | None | Unset):
        health_state (None | Unset | WorkerHealthState):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[PaginatedResponseWorkerSummary]
    """

    kwargs = _get_kwargs(
        page=page,
        page_size=page_size,
        role=role,
        status=status,
        cordoned=cordoned,
        health_state=health_state,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    *,
    client: AuthenticatedClient,
    page: int | Unset = UNSET,
    page_size: int | Unset = UNSET,
    role: None | Unset | WorkerRole = UNSET,
    status: None | Unset | WorkerStatus = UNSET,
    cordoned: bool | None | Unset = UNSET,
    health_state: None | Unset | WorkerHealthState = UNSET,
) -> PaginatedResponseWorkerSummary | None:
    """
    Args:
        page (int | Unset):
        page_size (int | Unset):
        role (None | Unset | WorkerRole):
        status (None | Unset | WorkerStatus):
        cordoned (bool | None | Unset):
        health_state (None | Unset | WorkerHealthState):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        PaginatedResponseWorkerSummary
    """

    return sync_detailed(
        client=client,
        page=page,
        page_size=page_size,
        role=role,
        status=status,
        cordoned=cordoned,
        health_state=health_state,
    ).parsed


async def asyncio_detailed(
    *,
    client: AuthenticatedClient,
    page: int | Unset = UNSET,
    page_size: int | Unset = UNSET,
    role: None | Unset | WorkerRole = UNSET,
    status: None | Unset | WorkerStatus = UNSET,
    cordoned: bool | None | Unset = UNSET,
    health_state: None | Unset | WorkerHealthState = UNSET,
) -> Response[PaginatedResponseWorkerSummary]:
    """
    Args:
        page (int | Unset):
        page_size (int | Unset):
        role (None | Unset | WorkerRole):
        status (None | Unset | WorkerStatus):
        cordoned (bool | None | Unset):
        health_state (None | Unset | WorkerHealthState):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[PaginatedResponseWorkerSummary]
    """

    kwargs = _get_kwargs(
        page=page,
        page_size=page_size,
        role=role,
        status=status,
        cordoned=cordoned,
        health_state=health_state,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    *,
    client: AuthenticatedClient,
    page: int | Unset = UNSET,
    page_size: int | Unset = UNSET,
    role: None | Unset | WorkerRole = UNSET,
    status: None | Unset | WorkerStatus = UNSET,
    cordoned: bool | None | Unset = UNSET,
    health_state: None | Unset | WorkerHealthState = UNSET,
) -> PaginatedResponseWorkerSummary | None:
    """
    Args:
        page (int | Unset):
        page_size (int | Unset):
        role (None | Unset | WorkerRole):
        status (None | Unset | WorkerStatus):
        cordoned (bool | None | Unset):
        health_state (None | Unset | WorkerHealthState):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        PaginatedResponseWorkerSummary
    """

    return (
        await asyncio_detailed(
            client=client,
            page=page,
            page_size=page_size,
            role=role,
            status=status,
            cordoned=cordoned,
            health_state=health_state,
        )
    ).parsed
