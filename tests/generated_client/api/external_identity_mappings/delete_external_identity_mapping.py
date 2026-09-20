from http import HTTPStatus
from typing import Any, cast
from urllib.parse import quote

import httpx

from ... import errors
from ...client import AuthenticatedClient, Client
from ...models.api_response_success_response import ApiResponseSuccessResponse
from ...types import Response


def _get_kwargs(
    integration_identity: int,
    mapping_id: int,
) -> dict[str, Any]:

    _kwargs: dict[str, Any] = {
        "method": "delete",
        "url": "/api/v1/identities/{integration_identity}/external-identity-mappings/{mapping_id}".format(
            integration_identity=quote(str(integration_identity), safe=""),
            mapping_id=quote(str(mapping_id), safe=""),
        ),
    }

    return _kwargs


def _parse_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> Any | ApiResponseSuccessResponse | None:
    if response.status_code == 200:
        response_200 = ApiResponseSuccessResponse.from_dict(response.json())

        return response_200

    if response.status_code == 401:
        response_401 = cast(Any, None)
        return response_401

    if response.status_code == 403:
        response_403 = cast(Any, None)
        return response_403

    if response.status_code == 404:
        response_404 = cast(Any, None)
        return response_404

    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> Response[Any | ApiResponseSuccessResponse]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    integration_identity: int,
    mapping_id: int,
    *,
    client: AuthenticatedClient,
) -> Response[Any | ApiResponseSuccessResponse]:
    """
    Args:
        integration_identity (int):
        mapping_id (int):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Any | ApiResponseSuccessResponse]
    """

    kwargs = _get_kwargs(
        integration_identity=integration_identity,
        mapping_id=mapping_id,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    integration_identity: int,
    mapping_id: int,
    *,
    client: AuthenticatedClient,
) -> Any | ApiResponseSuccessResponse | None:
    """
    Args:
        integration_identity (int):
        mapping_id (int):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Any | ApiResponseSuccessResponse
    """

    return sync_detailed(
        integration_identity=integration_identity,
        mapping_id=mapping_id,
        client=client,
    ).parsed


async def asyncio_detailed(
    integration_identity: int,
    mapping_id: int,
    *,
    client: AuthenticatedClient,
) -> Response[Any | ApiResponseSuccessResponse]:
    """
    Args:
        integration_identity (int):
        mapping_id (int):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Any | ApiResponseSuccessResponse]
    """

    kwargs = _get_kwargs(
        integration_identity=integration_identity,
        mapping_id=mapping_id,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    integration_identity: int,
    mapping_id: int,
    *,
    client: AuthenticatedClient,
) -> Any | ApiResponseSuccessResponse | None:
    """
    Args:
        integration_identity (int):
        mapping_id (int):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Any | ApiResponseSuccessResponse
    """

    return (
        await asyncio_detailed(
            integration_identity=integration_identity,
            mapping_id=mapping_id,
            client=client,
        )
    ).parsed
