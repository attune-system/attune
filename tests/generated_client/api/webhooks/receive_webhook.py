from http import HTTPStatus
from typing import Any, cast
from urllib.parse import quote

import httpx

from ... import errors
from ...client import AuthenticatedClient, Client
from ...models.webhook_receiver_request import WebhookReceiverRequest
from ...models.webhook_receiver_response import WebhookReceiverResponse
from ...types import Response


def _get_kwargs(
    webhook_key: str,
    *,
    body: WebhookReceiverRequest,
) -> dict[str, Any]:
    headers: dict[str, Any] = {}

    _kwargs: dict[str, Any] = {
        "method": "post",
        "url": "/api/v1/webhooks/{webhook_key}".format(
            webhook_key=quote(str(webhook_key), safe=""),
        ),
    }

    _kwargs["json"] = body.to_dict()

    headers["Content-Type"] = "application/json"

    _kwargs["headers"] = headers
    return _kwargs


def _parse_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> Any | WebhookReceiverResponse | None:
    if response.status_code == 200:
        response_200 = WebhookReceiverResponse.from_dict(response.json())

        return response_200

    if response.status_code == 404:
        response_404 = cast(Any, None)
        return response_404

    if response.status_code == 429:
        response_429 = cast(Any, None)
        return response_429

    if response.status_code == 500:
        response_500 = cast(Any, None)
        return response_500

    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> Response[Any | WebhookReceiverResponse]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    webhook_key: str,
    *,
    client: AuthenticatedClient | Client,
    body: WebhookReceiverRequest,
) -> Response[Any | WebhookReceiverResponse]:
    """Webhook receiver endpoint - receives webhook events and creates events

    Args:
        webhook_key (str):
        body (WebhookReceiverRequest): Request body for webhook receiver endpoint

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Any | WebhookReceiverResponse]
    """

    kwargs = _get_kwargs(
        webhook_key=webhook_key,
        body=body,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    webhook_key: str,
    *,
    client: AuthenticatedClient | Client,
    body: WebhookReceiverRequest,
) -> Any | WebhookReceiverResponse | None:
    """Webhook receiver endpoint - receives webhook events and creates events

    Args:
        webhook_key (str):
        body (WebhookReceiverRequest): Request body for webhook receiver endpoint

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Any | WebhookReceiverResponse
    """

    return sync_detailed(
        webhook_key=webhook_key,
        client=client,
        body=body,
    ).parsed


async def asyncio_detailed(
    webhook_key: str,
    *,
    client: AuthenticatedClient | Client,
    body: WebhookReceiverRequest,
) -> Response[Any | WebhookReceiverResponse]:
    """Webhook receiver endpoint - receives webhook events and creates events

    Args:
        webhook_key (str):
        body (WebhookReceiverRequest): Request body for webhook receiver endpoint

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Any | WebhookReceiverResponse]
    """

    kwargs = _get_kwargs(
        webhook_key=webhook_key,
        body=body,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    webhook_key: str,
    *,
    client: AuthenticatedClient | Client,
    body: WebhookReceiverRequest,
) -> Any | WebhookReceiverResponse | None:
    """Webhook receiver endpoint - receives webhook events and creates events

    Args:
        webhook_key (str):
        body (WebhookReceiverRequest): Request body for webhook receiver endpoint

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Any | WebhookReceiverResponse
    """

    return (
        await asyncio_detailed(
            webhook_key=webhook_key,
            client=client,
            body=body,
        )
    ).parsed
