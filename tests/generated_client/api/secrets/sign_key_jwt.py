from http import HTTPStatus
from typing import Any
from urllib.parse import quote

import httpx

from ... import errors
from ...client import AuthenticatedClient, Client
from ...models.sign_key_jwt_request import SignKeyJwtRequest
from ...models.sign_key_jwt_response_200 import SignKeyJwtResponse200
from ...types import Response


def _get_kwargs(
    ref: str,
    *,
    body: SignKeyJwtRequest,
) -> dict[str, Any]:
    headers: dict[str, Any] = {}

    _kwargs: dict[str, Any] = {
        "method": "post",
        "url": "/api/v1/keys/{ref}/sign-jwt".format(
            ref=quote(str(ref), safe=""),
        ),
    }

    _kwargs["json"] = body.to_dict()

    headers["Content-Type"] = "application/json"

    _kwargs["headers"] = headers
    return _kwargs


def _parse_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> SignKeyJwtResponse200 | None:
    if response.status_code == 200:
        response_200 = SignKeyJwtResponse200.from_dict(response.json())

        return response_200

    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> Response[SignKeyJwtResponse200]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    ref: str,
    *,
    client: AuthenticatedClient,
    body: SignKeyJwtRequest,
) -> Response[SignKeyJwtResponse200]:
    """
    Args:
        ref (str):
        body (SignKeyJwtRequest):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[SignKeyJwtResponse200]
    """

    kwargs = _get_kwargs(
        ref=ref,
        body=body,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    ref: str,
    *,
    client: AuthenticatedClient,
    body: SignKeyJwtRequest,
) -> SignKeyJwtResponse200 | None:
    """
    Args:
        ref (str):
        body (SignKeyJwtRequest):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        SignKeyJwtResponse200
    """

    return sync_detailed(
        ref=ref,
        client=client,
        body=body,
    ).parsed


async def asyncio_detailed(
    ref: str,
    *,
    client: AuthenticatedClient,
    body: SignKeyJwtRequest,
) -> Response[SignKeyJwtResponse200]:
    """
    Args:
        ref (str):
        body (SignKeyJwtRequest):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[SignKeyJwtResponse200]
    """

    kwargs = _get_kwargs(
        ref=ref,
        body=body,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    ref: str,
    *,
    client: AuthenticatedClient,
    body: SignKeyJwtRequest,
) -> SignKeyJwtResponse200 | None:
    """
    Args:
        ref (str):
        body (SignKeyJwtRequest):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        SignKeyJwtResponse200
    """

    return (
        await asyncio_detailed(
            ref=ref,
            client=client,
            body=body,
        )
    ).parsed
