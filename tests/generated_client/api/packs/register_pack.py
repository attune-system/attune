from http import HTTPStatus
from typing import Any

import httpx

from ... import errors
from ...client import AuthenticatedClient, Client
from ...models.api_response_pack_install_response import ApiResponsePackInstallResponse
from ...models.api_response_string import ApiResponseString
from ...models.register_pack_request import RegisterPackRequest
from ...types import Response


def _get_kwargs(
    *,
    body: RegisterPackRequest,
) -> dict[str, Any]:
    headers: dict[str, Any] = {}

    _kwargs: dict[str, Any] = {
        "method": "post",
        "url": "/api/v1/packs/register",
    }

    _kwargs["json"] = body.to_dict()

    headers["Content-Type"] = "application/json"

    _kwargs["headers"] = headers
    return _kwargs


def _parse_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> ApiResponsePackInstallResponse | ApiResponseString | None:
    if response.status_code == 201:
        response_201 = ApiResponsePackInstallResponse.from_dict(response.json())

        return response_201

    if response.status_code == 400:
        response_400 = ApiResponseString.from_dict(response.json())

        return response_400

    if response.status_code == 409:
        response_409 = ApiResponseString.from_dict(response.json())

        return response_409

    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> Response[ApiResponsePackInstallResponse | ApiResponseString]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    *,
    client: AuthenticatedClient,
    body: RegisterPackRequest,
) -> Response[ApiResponsePackInstallResponse | ApiResponseString]:
    """Register a pack from local filesystem

    Args:
        body (RegisterPackRequest): Request DTO for registering a pack from local filesystem

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[ApiResponsePackInstallResponse | ApiResponseString]
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
    body: RegisterPackRequest,
) -> ApiResponsePackInstallResponse | ApiResponseString | None:
    """Register a pack from local filesystem

    Args:
        body (RegisterPackRequest): Request DTO for registering a pack from local filesystem

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        ApiResponsePackInstallResponse | ApiResponseString
    """

    return sync_detailed(
        client=client,
        body=body,
    ).parsed


async def asyncio_detailed(
    *,
    client: AuthenticatedClient,
    body: RegisterPackRequest,
) -> Response[ApiResponsePackInstallResponse | ApiResponseString]:
    """Register a pack from local filesystem

    Args:
        body (RegisterPackRequest): Request DTO for registering a pack from local filesystem

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[ApiResponsePackInstallResponse | ApiResponseString]
    """

    kwargs = _get_kwargs(
        body=body,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    *,
    client: AuthenticatedClient,
    body: RegisterPackRequest,
) -> ApiResponsePackInstallResponse | ApiResponseString | None:
    """Register a pack from local filesystem

    Args:
        body (RegisterPackRequest): Request DTO for registering a pack from local filesystem

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        ApiResponsePackInstallResponse | ApiResponseString
    """

    return (
        await asyncio_detailed(
            client=client,
            body=body,
        )
    ).parsed
