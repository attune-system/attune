from http import HTTPStatus
from typing import Any, cast
from urllib.parse import quote

import httpx

from ... import errors
from ...client import AuthenticatedClient, Client
from ...models.api_response_external_identity_mapping_response import (
    ApiResponseExternalIdentityMappingResponse,
)
from ...models.create_external_identity_mapping_request import (
    CreateExternalIdentityMappingRequest,
)
from ...types import Response


def _get_kwargs(
    integration_identity: int,
    *,
    body: CreateExternalIdentityMappingRequest,
) -> dict[str, Any]:
    headers: dict[str, Any] = {}

    _kwargs: dict[str, Any] = {
        "method": "post",
        "url": "/api/v1/identities/{integration_identity}/external-identity-mappings".format(
            integration_identity=quote(str(integration_identity), safe=""),
        ),
    }

    _kwargs["json"] = body.to_dict()

    headers["Content-Type"] = "application/json"

    _kwargs["headers"] = headers
    return _kwargs


def _parse_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> Any | ApiResponseExternalIdentityMappingResponse | None:
    if response.status_code == 201:
        response_201 = ApiResponseExternalIdentityMappingResponse.from_dict(
            response.json()
        )

        return response_201

    if response.status_code == 401:
        response_401 = cast(Any, None)
        return response_401

    if response.status_code == 403:
        response_403 = cast(Any, None)
        return response_403

    if response.status_code == 404:
        response_404 = cast(Any, None)
        return response_404

    if response.status_code == 409:
        response_409 = cast(Any, None)
        return response_409

    if response.status_code == 422:
        response_422 = cast(Any, None)
        return response_422

    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> Response[Any | ApiResponseExternalIdentityMappingResponse]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    integration_identity: int,
    *,
    client: AuthenticatedClient,
    body: CreateExternalIdentityMappingRequest,
) -> Response[Any | ApiResponseExternalIdentityMappingResponse]:
    """
    Args:
        integration_identity (int):
        body (CreateExternalIdentityMappingRequest):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Any | ApiResponseExternalIdentityMappingResponse]
    """

    kwargs = _get_kwargs(
        integration_identity=integration_identity,
        body=body,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    integration_identity: int,
    *,
    client: AuthenticatedClient,
    body: CreateExternalIdentityMappingRequest,
) -> Any | ApiResponseExternalIdentityMappingResponse | None:
    """
    Args:
        integration_identity (int):
        body (CreateExternalIdentityMappingRequest):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Any | ApiResponseExternalIdentityMappingResponse
    """

    return sync_detailed(
        integration_identity=integration_identity,
        client=client,
        body=body,
    ).parsed


async def asyncio_detailed(
    integration_identity: int,
    *,
    client: AuthenticatedClient,
    body: CreateExternalIdentityMappingRequest,
) -> Response[Any | ApiResponseExternalIdentityMappingResponse]:
    """
    Args:
        integration_identity (int):
        body (CreateExternalIdentityMappingRequest):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Any | ApiResponseExternalIdentityMappingResponse]
    """

    kwargs = _get_kwargs(
        integration_identity=integration_identity,
        body=body,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    integration_identity: int,
    *,
    client: AuthenticatedClient,
    body: CreateExternalIdentityMappingRequest,
) -> Any | ApiResponseExternalIdentityMappingResponse | None:
    """
    Args:
        integration_identity (int):
        body (CreateExternalIdentityMappingRequest):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Any | ApiResponseExternalIdentityMappingResponse
    """

    return (
        await asyncio_detailed(
            integration_identity=integration_identity,
            client=client,
            body=body,
        )
    ).parsed
