from http import HTTPStatus
from typing import Any

import httpx

from ... import errors
from ...client import AuthenticatedClient, Client
from ...models.content_response_200 import ContentResponse200
from ...models.content_response_503 import ContentResponse503
from ...types import Response


def _get_kwargs() -> dict[str, Any]:

    _kwargs: dict[str, Any] = {
        "method": "get",
        "url": "/health/content",
    }

    return _kwargs


def _parse_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> ContentResponse200 | ContentResponse503 | None:
    if response.status_code == 200:
        response_200 = ContentResponse200.from_dict(response.json())

        return response_200

    if response.status_code == 503:
        response_503 = ContentResponse503.from_dict(response.json())

        return response_503

    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> Response[ContentResponse200 | ContentResponse503]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    *,
    client: AuthenticatedClient | Client,
) -> Response[ContentResponse200 | ContentResponse503]:
    """Transitional content and coarse host-capability health.

     Required-pack locks and candidate evidence replace this contract in issue #75.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[ContentResponse200 | ContentResponse503]
    """

    kwargs = _get_kwargs()

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    *,
    client: AuthenticatedClient | Client,
) -> ContentResponse200 | ContentResponse503 | None:
    """Transitional content and coarse host-capability health.

     Required-pack locks and candidate evidence replace this contract in issue #75.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        ContentResponse200 | ContentResponse503
    """

    return sync_detailed(
        client=client,
    ).parsed


async def asyncio_detailed(
    *,
    client: AuthenticatedClient | Client,
) -> Response[ContentResponse200 | ContentResponse503]:
    """Transitional content and coarse host-capability health.

     Required-pack locks and candidate evidence replace this contract in issue #75.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[ContentResponse200 | ContentResponse503]
    """

    kwargs = _get_kwargs()

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    *,
    client: AuthenticatedClient | Client,
) -> ContentResponse200 | ContentResponse503 | None:
    """Transitional content and coarse host-capability health.

     Required-pack locks and candidate evidence replace this contract in issue #75.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        ContentResponse200 | ContentResponse503
    """

    return (
        await asyncio_detailed(
            client=client,
        )
    ).parsed
