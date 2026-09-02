from http import HTTPStatus
from typing import Any
from urllib.parse import quote

import httpx

from ... import errors
from ...client import AuthenticatedClient, Client
from ...models.auth_error_response import AuthErrorResponse
from ...models.cache_generation_api_response import CacheGenerationApiResponse
from ...models.cache_owner_body import CacheOwnerBody
from ...models.error_response import ErrorResponse
from ...types import Response


def _get_kwargs(
    namespace: str,
    generation_id: int,
    *,
    body: CacheOwnerBody,
) -> dict[str, Any]:
    headers: dict[str, Any] = {}

    _kwargs: dict[str, Any] = {
        "method": "post",
        "url": "/api/v1/cache/namespaces/{namespace}/generations/{generation_id}/abandon".format(
            namespace=quote(str(namespace), safe=""),
            generation_id=quote(str(generation_id), safe=""),
        ),
    }

    _kwargs["json"] = body.to_dict()

    headers["Content-Type"] = "application/json"

    _kwargs["headers"] = headers
    return _kwargs


def _parse_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> AuthErrorResponse | ErrorResponse | CacheGenerationApiResponse | None:
    if response.status_code == 200:
        response_200 = CacheGenerationApiResponse.from_dict(response.json())

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

    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> Response[AuthErrorResponse | ErrorResponse | CacheGenerationApiResponse]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    namespace: str,
    generation_id: int,
    *,
    client: AuthenticatedClient,
    body: CacheOwnerBody,
) -> Response[AuthErrorResponse | ErrorResponse | CacheGenerationApiResponse]:
    """Abandon a staging or ready generation.

    Args:
        namespace (str):
        generation_id (int):
        body (CacheOwnerBody): Owner selector accepted in cache request bodies.

            `owner_ref` is the pack/action/sensor reference; it is omitted for the
            `system` scope and resolved to the authenticated identity for `identity`.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[AuthErrorResponse | AuthErrorResponse | ErrorResponse | CacheGenerationApiResponse | ErrorResponse]
    """

    kwargs = _get_kwargs(
        namespace=namespace,
        generation_id=generation_id,
        body=body,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    namespace: str,
    generation_id: int,
    *,
    client: AuthenticatedClient,
    body: CacheOwnerBody,
) -> AuthErrorResponse | ErrorResponse | CacheGenerationApiResponse | None:
    """Abandon a staging or ready generation.

    Args:
        namespace (str):
        generation_id (int):
        body (CacheOwnerBody): Owner selector accepted in cache request bodies.

            `owner_ref` is the pack/action/sensor reference; it is omitted for the
            `system` scope and resolved to the authenticated identity for `identity`.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        AuthErrorResponse | AuthErrorResponse | ErrorResponse | CacheGenerationApiResponse | ErrorResponse
    """

    return sync_detailed(
        namespace=namespace,
        generation_id=generation_id,
        client=client,
        body=body,
    ).parsed


async def asyncio_detailed(
    namespace: str,
    generation_id: int,
    *,
    client: AuthenticatedClient,
    body: CacheOwnerBody,
) -> Response[AuthErrorResponse | ErrorResponse | CacheGenerationApiResponse]:
    """Abandon a staging or ready generation.

    Args:
        namespace (str):
        generation_id (int):
        body (CacheOwnerBody): Owner selector accepted in cache request bodies.

            `owner_ref` is the pack/action/sensor reference; it is omitted for the
            `system` scope and resolved to the authenticated identity for `identity`.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[AuthErrorResponse | AuthErrorResponse | ErrorResponse | CacheGenerationApiResponse | ErrorResponse]
    """

    kwargs = _get_kwargs(
        namespace=namespace,
        generation_id=generation_id,
        body=body,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    namespace: str,
    generation_id: int,
    *,
    client: AuthenticatedClient,
    body: CacheOwnerBody,
) -> AuthErrorResponse | ErrorResponse | CacheGenerationApiResponse | None:
    """Abandon a staging or ready generation.

    Args:
        namespace (str):
        generation_id (int):
        body (CacheOwnerBody): Owner selector accepted in cache request bodies.

            `owner_ref` is the pack/action/sensor reference; it is omitted for the
            `system` scope and resolved to the authenticated identity for `identity`.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        AuthErrorResponse | AuthErrorResponse | ErrorResponse | CacheGenerationApiResponse | ErrorResponse
    """

    return (
        await asyncio_detailed(
            namespace=namespace,
            generation_id=generation_id,
            client=client,
            body=body,
        )
    ).parsed
