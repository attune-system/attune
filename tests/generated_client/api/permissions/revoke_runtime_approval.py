from http import HTTPStatus
from typing import Any
from urllib.parse import quote

import httpx

from ... import errors
from ...client import AuthenticatedClient, Client
from ...models.api_response_runtime_approval_response import (
    ApiResponseRuntimeApprovalResponse,
)
from ...types import Response


def _get_kwargs(
    pack_ref: str,
    id: int,
) -> dict[str, Any]:

    _kwargs: dict[str, Any] = {
        "method": "post",
        "url": "/api/v1/packs/{pack_ref}/external-approvals/{id}/revoke".format(
            pack_ref=quote(str(pack_ref), safe=""),
            id=quote(str(id), safe=""),
        ),
    }

    return _kwargs


def _parse_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> ApiResponseRuntimeApprovalResponse | None:
    if response.status_code == 200:
        response_200 = ApiResponseRuntimeApprovalResponse.from_dict(response.json())

        return response_200

    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> Response[ApiResponseRuntimeApprovalResponse]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    pack_ref: str,
    id: int,
    *,
    client: AuthenticatedClient,
) -> Response[ApiResponseRuntimeApprovalResponse]:
    """
    Args:
        pack_ref (str):
        id (int):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[ApiResponseRuntimeApprovalResponse]
    """

    kwargs = _get_kwargs(
        pack_ref=pack_ref,
        id=id,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    pack_ref: str,
    id: int,
    *,
    client: AuthenticatedClient,
) -> ApiResponseRuntimeApprovalResponse | None:
    """
    Args:
        pack_ref (str):
        id (int):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        ApiResponseRuntimeApprovalResponse
    """

    return sync_detailed(
        pack_ref=pack_ref,
        id=id,
        client=client,
    ).parsed


async def asyncio_detailed(
    pack_ref: str,
    id: int,
    *,
    client: AuthenticatedClient,
) -> Response[ApiResponseRuntimeApprovalResponse]:
    """
    Args:
        pack_ref (str):
        id (int):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[ApiResponseRuntimeApprovalResponse]
    """

    kwargs = _get_kwargs(
        pack_ref=pack_ref,
        id=id,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    pack_ref: str,
    id: int,
    *,
    client: AuthenticatedClient,
) -> ApiResponseRuntimeApprovalResponse | None:
    """
    Args:
        pack_ref (str):
        id (int):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        ApiResponseRuntimeApprovalResponse
    """

    return (
        await asyncio_detailed(
            pack_ref=pack_ref,
            id=id,
            client=client,
        )
    ).parsed
