from http import HTTPStatus
from typing import Any

import httpx

from ... import errors
from ...client import AuthenticatedClient, Client
from ...models.api_response_pack_install_response import ApiResponsePackInstallResponse
from ...models.api_response_string import ApiResponseString
from ...models.install_pack_request import InstallPackRequest
from ...types import Response


def _get_kwargs(
    *,
    body: InstallPackRequest,
) -> dict[str, Any]:
    headers: dict[str, Any] = {}

    _kwargs: dict[str, Any] = {
        "method": "post",
        "url": "/api/v1/packs/install",
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

    if response.status_code == 501:
        response_501 = ApiResponseString.from_dict(response.json())

        return response_501

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
    body: InstallPackRequest,
) -> Response[ApiResponsePackInstallResponse | ApiResponseString]:
    """Install a pack from remote source (git repository)

    Args:
        body (InstallPackRequest): Request DTO for installing a pack from remote source

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
    body: InstallPackRequest,
) -> ApiResponsePackInstallResponse | ApiResponseString | None:
    """Install a pack from remote source (git repository)

    Args:
        body (InstallPackRequest): Request DTO for installing a pack from remote source

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
    body: InstallPackRequest,
) -> Response[ApiResponsePackInstallResponse | ApiResponseString]:
    """Install a pack from remote source (git repository)

    Args:
        body (InstallPackRequest): Request DTO for installing a pack from remote source

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
    body: InstallPackRequest,
) -> ApiResponsePackInstallResponse | ApiResponseString | None:
    """Install a pack from remote source (git repository)

    Args:
        body (InstallPackRequest): Request DTO for installing a pack from remote source

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
