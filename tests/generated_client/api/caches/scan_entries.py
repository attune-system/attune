from http import HTTPStatus
from typing import Any
from urllib.parse import quote

import httpx

from ... import errors
from ...client import AuthenticatedClient, Client
from ...models.auth_error_response import AuthErrorResponse
from ...models.cache_scan_page_api_response import CacheScanPageApiResponse
from ...models.error_response import ErrorResponse
from ...models.owner_type import OwnerType
from ...types import UNSET, Response, Unset


def _get_kwargs(
    namespace: str,
    *,
    owner_type: OwnerType,
    owner_ref: None | str | Unset = UNSET,
    limit: int | None | Unset = UNSET,
    require_fresh: bool | Unset = UNSET,
    generation: int | None | Unset = UNSET,
    cursor: None | str | Unset = UNSET,
) -> dict[str, Any]:

    params: dict[str, Any] = {}

    json_owner_type = owner_type.value
    params["owner_type"] = json_owner_type

    json_owner_ref: None | str | Unset
    if isinstance(owner_ref, Unset):
        json_owner_ref = UNSET
    else:
        json_owner_ref = owner_ref
    params["owner_ref"] = json_owner_ref

    json_limit: int | None | Unset
    if isinstance(limit, Unset):
        json_limit = UNSET
    else:
        json_limit = limit
    params["limit"] = json_limit

    params["require_fresh"] = require_fresh

    json_generation: int | None | Unset
    if isinstance(generation, Unset):
        json_generation = UNSET
    else:
        json_generation = generation
    params["generation"] = json_generation

    json_cursor: None | str | Unset
    if isinstance(cursor, Unset):
        json_cursor = UNSET
    else:
        json_cursor = cursor
    params["cursor"] = json_cursor

    params = {k: v for k, v in params.items() if v is not UNSET and v is not None}

    _kwargs: dict[str, Any] = {
        "method": "get",
        "url": "/api/v1/cache/namespaces/{namespace}/entries".format(
            namespace=quote(str(namespace), safe=""),
        ),
        "params": params,
    }

    return _kwargs


def _parse_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> AuthErrorResponse | ErrorResponse | CacheScanPageApiResponse | None:
    if response.status_code == 200:
        response_200 = CacheScanPageApiResponse.from_dict(response.json())

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
) -> Response[AuthErrorResponse | ErrorResponse | CacheScanPageApiResponse]:
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
    owner_type: OwnerType,
    owner_ref: None | str | Unset = UNSET,
    limit: int | None | Unset = UNSET,
    require_fresh: bool | Unset = UNSET,
    generation: int | None | Unset = UNSET,
    cursor: None | str | Unset = UNSET,
) -> Response[AuthErrorResponse | ErrorResponse | CacheScanPageApiResponse]:
    """Generation-pinned cursor scan.

    Args:
        namespace (str):
        owner_type (OwnerType):
        owner_ref (None | str | Unset):
        limit (int | None | Unset):
        require_fresh (bool | Unset):
        generation (int | None | Unset):
        cursor (None | str | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[AuthErrorResponse | AuthErrorResponse | ErrorResponse | CacheScanPageApiResponse | ErrorResponse]
    """

    kwargs = _get_kwargs(
        namespace=namespace,
        owner_type=owner_type,
        owner_ref=owner_ref,
        limit=limit,
        require_fresh=require_fresh,
        generation=generation,
        cursor=cursor,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    namespace: str,
    *,
    client: AuthenticatedClient,
    owner_type: OwnerType,
    owner_ref: None | str | Unset = UNSET,
    limit: int | None | Unset = UNSET,
    require_fresh: bool | Unset = UNSET,
    generation: int | None | Unset = UNSET,
    cursor: None | str | Unset = UNSET,
) -> AuthErrorResponse | ErrorResponse | CacheScanPageApiResponse | None:
    """Generation-pinned cursor scan.

    Args:
        namespace (str):
        owner_type (OwnerType):
        owner_ref (None | str | Unset):
        limit (int | None | Unset):
        require_fresh (bool | Unset):
        generation (int | None | Unset):
        cursor (None | str | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        AuthErrorResponse | AuthErrorResponse | ErrorResponse | CacheScanPageApiResponse | ErrorResponse
    """

    return sync_detailed(
        namespace=namespace,
        client=client,
        owner_type=owner_type,
        owner_ref=owner_ref,
        limit=limit,
        require_fresh=require_fresh,
        generation=generation,
        cursor=cursor,
    ).parsed


async def asyncio_detailed(
    namespace: str,
    *,
    client: AuthenticatedClient,
    owner_type: OwnerType,
    owner_ref: None | str | Unset = UNSET,
    limit: int | None | Unset = UNSET,
    require_fresh: bool | Unset = UNSET,
    generation: int | None | Unset = UNSET,
    cursor: None | str | Unset = UNSET,
) -> Response[AuthErrorResponse | ErrorResponse | CacheScanPageApiResponse]:
    """Generation-pinned cursor scan.

    Args:
        namespace (str):
        owner_type (OwnerType):
        owner_ref (None | str | Unset):
        limit (int | None | Unset):
        require_fresh (bool | Unset):
        generation (int | None | Unset):
        cursor (None | str | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[AuthErrorResponse | AuthErrorResponse | ErrorResponse | CacheScanPageApiResponse | ErrorResponse]
    """

    kwargs = _get_kwargs(
        namespace=namespace,
        owner_type=owner_type,
        owner_ref=owner_ref,
        limit=limit,
        require_fresh=require_fresh,
        generation=generation,
        cursor=cursor,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    namespace: str,
    *,
    client: AuthenticatedClient,
    owner_type: OwnerType,
    owner_ref: None | str | Unset = UNSET,
    limit: int | None | Unset = UNSET,
    require_fresh: bool | Unset = UNSET,
    generation: int | None | Unset = UNSET,
    cursor: None | str | Unset = UNSET,
) -> AuthErrorResponse | ErrorResponse | CacheScanPageApiResponse | None:
    """Generation-pinned cursor scan.

    Args:
        namespace (str):
        owner_type (OwnerType):
        owner_ref (None | str | Unset):
        limit (int | None | Unset):
        require_fresh (bool | Unset):
        generation (int | None | Unset):
        cursor (None | str | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        AuthErrorResponse | AuthErrorResponse | ErrorResponse | CacheScanPageApiResponse | ErrorResponse
    """

    return (
        await asyncio_detailed(
            namespace=namespace,
            client=client,
            owner_type=owner_type,
            owner_ref=owner_ref,
            limit=limit,
            require_fresh=require_fresh,
            generation=generation,
            cursor=cursor,
        )
    ).parsed
