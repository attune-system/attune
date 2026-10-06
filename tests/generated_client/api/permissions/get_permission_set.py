from http import HTTPStatus
from typing import Any
from urllib.parse import quote

import httpx

from ... import errors
from ...client import AuthenticatedClient, Client
from ...models.get_permission_set_response_200 import GetPermissionSetResponse200
from ...types import Response


def _get_kwargs(
    permission_set_ref: str,
) -> dict[str, Any]:

    _kwargs: dict[str, Any] = {
        "method": "get",
        "url": "/api/v1/permissions/sets/by-ref/{permission_set_ref}".format(
            permission_set_ref=quote(str(permission_set_ref), safe=""),
        ),
    }

    return _kwargs


def _parse_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> GetPermissionSetResponse200 | None:
    if response.status_code == 200:
        response_200 = GetPermissionSetResponse200.from_dict(response.json())

        return response_200

    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> Response[GetPermissionSetResponse200]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    permission_set_ref: str,
    *,
    client: AuthenticatedClient,
) -> Response[GetPermissionSetResponse200]:
    """
    Args:
        permission_set_ref (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[GetPermissionSetResponse200]
    """

    kwargs = _get_kwargs(
        permission_set_ref=permission_set_ref,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    permission_set_ref: str,
    *,
    client: AuthenticatedClient,
) -> GetPermissionSetResponse200 | None:
    """
    Args:
        permission_set_ref (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        GetPermissionSetResponse200
    """

    return sync_detailed(
        permission_set_ref=permission_set_ref,
        client=client,
    ).parsed


async def asyncio_detailed(
    permission_set_ref: str,
    *,
    client: AuthenticatedClient,
) -> Response[GetPermissionSetResponse200]:
    """
    Args:
        permission_set_ref (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[GetPermissionSetResponse200]
    """

    kwargs = _get_kwargs(
        permission_set_ref=permission_set_ref,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    permission_set_ref: str,
    *,
    client: AuthenticatedClient,
) -> GetPermissionSetResponse200 | None:
    """
    Args:
        permission_set_ref (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        GetPermissionSetResponse200
    """

    return (
        await asyncio_detailed(
            permission_set_ref=permission_set_ref,
            client=client,
        )
    ).parsed
