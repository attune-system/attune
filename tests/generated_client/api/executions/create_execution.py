from http import HTTPStatus
from typing import Any, cast

import httpx

from ... import errors
from ...client import AuthenticatedClient, Client
from ...models.create_execution_request import CreateExecutionRequest
from ...models.execution_response import ExecutionResponse
from ...types import Response


def _get_kwargs(
    *,
    body: CreateExecutionRequest,
) -> dict[str, Any]:
    headers: dict[str, Any] = {}

    _kwargs: dict[str, Any] = {
        "method": "post",
        "url": "/api/v1/executions/execute",
    }

    _kwargs["json"] = body.to_dict()

    headers["Content-Type"] = "application/json"

    _kwargs["headers"] = headers
    return _kwargs


def _parse_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> Any | ExecutionResponse | None:
    if response.status_code == 201:
        response_201 = ExecutionResponse.from_dict(response.json())

        return response_201

    if response.status_code == 400:
        response_400 = cast(Any, None)
        return response_400

    if response.status_code == 404:
        response_404 = cast(Any, None)
        return response_404

    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> Response[Any | ExecutionResponse]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    *,
    client: AuthenticatedClient,
    body: CreateExecutionRequest,
) -> Response[Any | ExecutionResponse]:
    """Create a new execution (manual execution)

     This endpoint allows directly executing an action without a trigger or rule.
    The execution is queued and will be picked up by the executor service.

    Args:
        body (CreateExecutionRequest): Request DTO for creating a manual execution

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Any | ExecutionResponse]
    """

    kwargs = _get_kwargs(
        body=body,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    *,
    client: AuthenticatedClient,
    body: CreateExecutionRequest,
) -> Any | ExecutionResponse | None:
    """Create a new execution (manual execution)

     This endpoint allows directly executing an action without a trigger or rule.
    The execution is queued and will be picked up by the executor service.

    Args:
        body (CreateExecutionRequest): Request DTO for creating a manual execution

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Any | ExecutionResponse
    """

    return sync_detailed(
        client=client,
        body=body,
    ).parsed


async def asyncio_detailed(
    *,
    client: AuthenticatedClient,
    body: CreateExecutionRequest,
) -> Response[Any | ExecutionResponse]:
    """Create a new execution (manual execution)

     This endpoint allows directly executing an action without a trigger or rule.
    The execution is queued and will be picked up by the executor service.

    Args:
        body (CreateExecutionRequest): Request DTO for creating a manual execution

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Any | ExecutionResponse]
    """

    kwargs = _get_kwargs(
        body=body,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    *,
    client: AuthenticatedClient,
    body: CreateExecutionRequest,
) -> Any | ExecutionResponse | None:
    """Create a new execution (manual execution)

     This endpoint allows directly executing an action without a trigger or rule.
    The execution is queued and will be picked up by the executor service.

    Args:
        body (CreateExecutionRequest): Request DTO for creating a manual execution

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Any | ExecutionResponse
    """

    return (
        await asyncio_detailed(
            client=client,
            body=body,
        )
    ).parsed
