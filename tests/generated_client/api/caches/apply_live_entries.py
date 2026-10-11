from http import HTTPStatus
from typing import Any
from urllib.parse import quote

import httpx

from ... import errors
from ...client import AuthenticatedClient, Client
from ...models.apply_live_cache_api_response import ApplyLiveCacheApiResponse
from ...models.apply_live_cache_request import ApplyLiveCacheRequest
from ...models.auth_error_response import AuthErrorResponse
from ...models.error_response import ErrorResponse
from ...types import Response


def _get_kwargs(
    namespace: str,
    *,
    body: ApplyLiveCacheRequest,
) -> dict[str, Any]:
    headers: dict[str, Any] = {}

    _kwargs: dict[str, Any] = {
        "method": "post",
        "url": "/api/v1/cache/namespaces/{namespace}/entries/apply".format(
            namespace=quote(str(namespace), safe=""),
        ),
    }

    _kwargs["json"] = body.to_dict()

    headers["Content-Type"] = "application/json"

    _kwargs["headers"] = headers
    return _kwargs


def _parse_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> ApplyLiveCacheApiResponse | AuthErrorResponse | ErrorResponse | None:
    if response.status_code == 200:
        response_200 = ApplyLiveCacheApiResponse.from_dict(response.json())

        return response_200

    if response.status_code == 400:
        response_400 = ErrorResponse.from_dict(response.json())

        return response_400

    if response.status_code == 401:
        response_401 = AuthErrorResponse.from_dict(response.json())

        return response_401

    if response.status_code == 403:

        def _parse_response_403(data: object) -> AuthErrorResponse | ErrorResponse:
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_cache_forbidden_response_type_0 = (
                    AuthErrorResponse.from_dict(data)
                )

                return componentsschemas_cache_forbidden_response_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            if not isinstance(data, dict):
                raise TypeError()
            componentsschemas_cache_forbidden_response_type_1 = ErrorResponse.from_dict(
                data
            )

            return componentsschemas_cache_forbidden_response_type_1

        response_403 = _parse_response_403(response.json())

        return response_403

    if response.status_code == 404:
        response_404 = ErrorResponse.from_dict(response.json())

        return response_404

    if response.status_code == 409:
        response_409 = ErrorResponse.from_dict(response.json())

        return response_409

    if response.status_code == 500:
        response_500 = ErrorResponse.from_dict(response.json())

        return response_500

    if response.status_code == 503:
        response_503 = ErrorResponse.from_dict(response.json())

        return response_503

    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> Response[ApplyLiveCacheApiResponse | AuthErrorResponse | ErrorResponse]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    namespace: str,
    *,
    client: AuthenticatedClient,
    body: ApplyLiveCacheRequest,
) -> Response[ApplyLiveCacheApiResponse | AuthErrorResponse | ErrorResponse]:
    """Applies one atomic live-cache batch with optimistic concurrency and bounded retries.

    Args:
        namespace (str):
        body (ApplyLiveCacheRequest): Atomic mutations to a durable live namespace. No snapshot
            lifecycle is used.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[ApplyLiveCacheApiResponse | AuthErrorResponse | AuthErrorResponse | ErrorResponse | ErrorResponse]
    """

    kwargs = _get_kwargs(
        namespace=namespace,
        body=body,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    namespace: str,
    *,
    client: AuthenticatedClient,
    body: ApplyLiveCacheRequest,
) -> ApplyLiveCacheApiResponse | AuthErrorResponse | ErrorResponse | None:
    """Applies one atomic live-cache batch with optimistic concurrency and bounded retries.

    Args:
        namespace (str):
        body (ApplyLiveCacheRequest): Atomic mutations to a durable live namespace. No snapshot
            lifecycle is used.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        ApplyLiveCacheApiResponse | AuthErrorResponse | AuthErrorResponse | ErrorResponse | ErrorResponse
    """

    return sync_detailed(
        namespace=namespace,
        client=client,
        body=body,
    ).parsed


async def asyncio_detailed(
    namespace: str,
    *,
    client: AuthenticatedClient,
    body: ApplyLiveCacheRequest,
) -> Response[ApplyLiveCacheApiResponse | AuthErrorResponse | ErrorResponse]:
    """Applies one atomic live-cache batch with optimistic concurrency and bounded retries.

    Args:
        namespace (str):
        body (ApplyLiveCacheRequest): Atomic mutations to a durable live namespace. No snapshot
            lifecycle is used.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[ApplyLiveCacheApiResponse | AuthErrorResponse | AuthErrorResponse | ErrorResponse | ErrorResponse]
    """

    kwargs = _get_kwargs(
        namespace=namespace,
        body=body,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    namespace: str,
    *,
    client: AuthenticatedClient,
    body: ApplyLiveCacheRequest,
) -> ApplyLiveCacheApiResponse | AuthErrorResponse | ErrorResponse | None:
    """Applies one atomic live-cache batch with optimistic concurrency and bounded retries.

    Args:
        namespace (str):
        body (ApplyLiveCacheRequest): Atomic mutations to a durable live namespace. No snapshot
            lifecycle is used.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        ApplyLiveCacheApiResponse | AuthErrorResponse | AuthErrorResponse | ErrorResponse | ErrorResponse
    """

    return (
        await asyncio_detailed(
            namespace=namespace,
            client=client,
            body=body,
        )
    ).parsed
