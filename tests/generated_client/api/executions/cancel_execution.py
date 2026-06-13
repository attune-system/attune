from http import HTTPStatus
from typing import Any, cast
from urllib.parse import quote

import httpx

from ... import errors
from ...client import AuthenticatedClient, Client
from ...models.cancel_execution_response_200 import CancelExecutionResponse200
from ...types import Response


def _get_kwargs(
    id: int,
) -> dict[str, Any]:

    _kwargs: dict[str, Any] = {
        "method": "post",
        "url": "/api/v1/executions/{id}/cancel".format(
            id=quote(str(id), safe=""),
        ),
    }

    return _kwargs


def _parse_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> Any | CancelExecutionResponse200 | None:
    if response.status_code == 200:
        response_200 = CancelExecutionResponse200.from_dict(response.json())

        return response_200

    if response.status_code == 404:
        response_404 = cast(Any, None)
        return response_404

    if response.status_code == 409:
        response_409 = cast(Any, None)
        return response_409

    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> Response[Any | CancelExecutionResponse200]:
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
) -> Response[Any | CancelExecutionResponse200]:
    """Cancel a running execution

     This endpoint requests cancellation of an execution. The execution must be in a
    cancellable state (requested, scheduling, scheduled, running, or canceling).
    For running executions, the worker will send SIGINT to the process, then SIGTERM
    after a 10-second grace period if it hasn't stopped.

    **Workflow cascading**: When a workflow (parent) execution is cancelled, all of
    its incomplete child task executions are also cancelled. Children that haven't
    reached a worker yet are set to Cancelled immediately; children that are running
    receive a cancel MQ message so their worker can gracefully stop the process.
    The workflow_execution record is also marked as Cancelled to prevent the
    scheduler from dispatching any further tasks.

    Args:
        id (int):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Any | CancelExecutionResponse200]
    """

    kwargs = _get_kwargs(
        id=id,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    id: int,
    *,
    client: AuthenticatedClient,
) -> Any | CancelExecutionResponse200 | None:
    """Cancel a running execution

     This endpoint requests cancellation of an execution. The execution must be in a
    cancellable state (requested, scheduling, scheduled, running, or canceling).
    For running executions, the worker will send SIGINT to the process, then SIGTERM
    after a 10-second grace period if it hasn't stopped.

    **Workflow cascading**: When a workflow (parent) execution is cancelled, all of
    its incomplete child task executions are also cancelled. Children that haven't
    reached a worker yet are set to Cancelled immediately; children that are running
    receive a cancel MQ message so their worker can gracefully stop the process.
    The workflow_execution record is also marked as Cancelled to prevent the
    scheduler from dispatching any further tasks.

    Args:
        id (int):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Any | CancelExecutionResponse200
    """

    return sync_detailed(
        id=id,
        client=client,
    ).parsed


async def asyncio_detailed(
    id: int,
    *,
    client: AuthenticatedClient,
) -> Response[Any | CancelExecutionResponse200]:
    """Cancel a running execution

     This endpoint requests cancellation of an execution. The execution must be in a
    cancellable state (requested, scheduling, scheduled, running, or canceling).
    For running executions, the worker will send SIGINT to the process, then SIGTERM
    after a 10-second grace period if it hasn't stopped.

    **Workflow cascading**: When a workflow (parent) execution is cancelled, all of
    its incomplete child task executions are also cancelled. Children that haven't
    reached a worker yet are set to Cancelled immediately; children that are running
    receive a cancel MQ message so their worker can gracefully stop the process.
    The workflow_execution record is also marked as Cancelled to prevent the
    scheduler from dispatching any further tasks.

    Args:
        id (int):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Any | CancelExecutionResponse200]
    """

    kwargs = _get_kwargs(
        id=id,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    id: int,
    *,
    client: AuthenticatedClient,
) -> Any | CancelExecutionResponse200 | None:
    """Cancel a running execution

     This endpoint requests cancellation of an execution. The execution must be in a
    cancellable state (requested, scheduling, scheduled, running, or canceling).
    For running executions, the worker will send SIGINT to the process, then SIGTERM
    after a 10-second grace period if it hasn't stopped.

    **Workflow cascading**: When a workflow (parent) execution is cancelled, all of
    its incomplete child task executions are also cancelled. Children that haven't
    reached a worker yet are set to Cancelled immediately; children that are running
    receive a cancel MQ message so their worker can gracefully stop the process.
    The workflow_execution record is also marked as Cancelled to prevent the
    scheduler from dispatching any further tasks.

    Args:
        id (int):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Any | CancelExecutionResponse200
    """

    return (
        await asyncio_detailed(
            id=id,
            client=client,
        )
    ).parsed
