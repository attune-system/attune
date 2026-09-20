from http import HTTPStatus
from typing import Any, cast
from urllib.parse import quote

import httpx

from ... import errors
from ...client import AuthenticatedClient, Client
from ...models.paginated_response_external_identity_mapping_response import (
    PaginatedResponseExternalIdentityMappingResponse,
)
from ...types import UNSET, Response, Unset


def _get_kwargs(
    integration_identity: int,
    *,
    page: int | Unset = UNSET,
    page_size: int | Unset = UNSET,
) -> dict[str, Any]:

    params: dict[str, Any] = {}

    params["page"] = page

    params["page_size"] = page_size

    params = {k: v for k, v in params.items() if v is not UNSET and v is not None}

    _kwargs: dict[str, Any] = {
        "method": "get",
        "url": "/api/v1/identities/{integration_identity}/external-identity-mappings".format(
            integration_identity=quote(str(integration_identity), safe=""),
        ),
        "params": params,
    }

    return _kwargs


def _parse_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> Any | PaginatedResponseExternalIdentityMappingResponse | None:
    if response.status_code == 200:
        response_200 = PaginatedResponseExternalIdentityMappingResponse.from_dict(
            response.json()
        )

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
) -> Response[Any | PaginatedResponseExternalIdentityMappingResponse]:
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
    page: int | Unset = UNSET,
    page_size: int | Unset = UNSET,
) -> Response[Any | PaginatedResponseExternalIdentityMappingResponse]:
    """
    Args:
        integration_identity (int):
        page (int | Unset):
        page_size (int | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Any | PaginatedResponseExternalIdentityMappingResponse]
    """

    kwargs = _get_kwargs(
        integration_identity=integration_identity,
        page=page,
        page_size=page_size,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    integration_identity: int,
    *,
    client: AuthenticatedClient,
    page: int | Unset = UNSET,
    page_size: int | Unset = UNSET,
) -> Any | PaginatedResponseExternalIdentityMappingResponse | None:
    """
    Args:
        integration_identity (int):
        page (int | Unset):
        page_size (int | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Any | PaginatedResponseExternalIdentityMappingResponse
    """

    return sync_detailed(
        integration_identity=integration_identity,
        client=client,
        page=page,
        page_size=page_size,
    ).parsed


async def asyncio_detailed(
    integration_identity: int,
    *,
    client: AuthenticatedClient,
    page: int | Unset = UNSET,
    page_size: int | Unset = UNSET,
) -> Response[Any | PaginatedResponseExternalIdentityMappingResponse]:
    """
    Args:
        integration_identity (int):
        page (int | Unset):
        page_size (int | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Any | PaginatedResponseExternalIdentityMappingResponse]
    """

    kwargs = _get_kwargs(
        integration_identity=integration_identity,
        page=page,
        page_size=page_size,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    integration_identity: int,
    *,
    client: AuthenticatedClient,
    page: int | Unset = UNSET,
    page_size: int | Unset = UNSET,
) -> Any | PaginatedResponseExternalIdentityMappingResponse | None:
    """
    Args:
        integration_identity (int):
        page (int | Unset):
        page_size (int | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Any | PaginatedResponseExternalIdentityMappingResponse
    """

    return (
        await asyncio_detailed(
            integration_identity=integration_identity,
            client=client,
            page=page,
            page_size=page_size,
        )
    ).parsed
