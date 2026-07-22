from http import HTTPStatus
from typing import Any, cast
from urllib.parse import quote

import httpx

from ...client import AuthenticatedClient, Client
from ...types import Response, UNSET
from ... import errors

from ...models.auth_error_response import AuthErrorResponse
from ...models.cache_namespace_freshness import CacheNamespaceFreshness
from ...models.cache_namespace_list_api_response import CacheNamespaceListApiResponse
from ...models.error_response import ErrorResponse
from ...models.owner_type import OwnerType
from ...types import UNSET, Unset
from typing import cast



def _get_kwargs(
    *,
    owner_type: OwnerType,
    owner_ref: None | str | Unset = UNSET,
    namespace: None | str | Unset = UNSET,
    freshness: CacheNamespaceFreshness | None | Unset = UNSET,
    limit: int | None | Unset = UNSET,
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

    json_namespace: None | str | Unset
    if isinstance(namespace, Unset):
        json_namespace = UNSET
    else:
        json_namespace = namespace
    params["namespace"] = json_namespace

    json_freshness: None | str | Unset
    if isinstance(freshness, Unset):
        json_freshness = UNSET
    elif isinstance(freshness, CacheNamespaceFreshness):
        json_freshness = freshness.value
    else:
        json_freshness = freshness
    params["freshness"] = json_freshness

    json_limit: int | None | Unset
    if isinstance(limit, Unset):
        json_limit = UNSET
    else:
        json_limit = limit
    params["limit"] = json_limit

    json_cursor: None | str | Unset
    if isinstance(cursor, Unset):
        json_cursor = UNSET
    else:
        json_cursor = cursor
    params["cursor"] = json_cursor


    params = {k: v for k, v in params.items() if v is not UNSET and v is not None}


    _kwargs: dict[str, Any] = {
        "method": "get",
        "url": "/api/v1/cache/namespaces",
        "params": params,
    }


    return _kwargs



def _parse_response(*, client: AuthenticatedClient | Client, response: httpx.Response) -> AuthErrorResponse | AuthErrorResponse | ErrorResponse | CacheNamespaceListApiResponse | ErrorResponse | None:
    if response.status_code == 200:
        response_200 = CacheNamespaceListApiResponse.from_dict(response.json())



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
                componentsschemas_cache_forbidden_response_type_0 = AuthErrorResponse.from_dict(data)



                return componentsschemas_cache_forbidden_response_type_0
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            if not isinstance(data, dict):
                raise TypeError()
            componentsschemas_cache_forbidden_response_type_1 = ErrorResponse.from_dict(data)



            return componentsschemas_cache_forbidden_response_type_1

        response_403 = _parse_response_403(response.json())

        return response_403

    if response.status_code == 500:
        response_500 = ErrorResponse.from_dict(response.json())



        return response_500

    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(*, client: AuthenticatedClient | Client, response: httpx.Response) -> Response[AuthErrorResponse | AuthErrorResponse | ErrorResponse | CacheNamespaceListApiResponse | ErrorResponse]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    *,
    client: AuthenticatedClient,
    owner_type: OwnerType,
    owner_ref: None | str | Unset = UNSET,
    namespace: None | str | Unset = UNSET,
    freshness: CacheNamespaceFreshness | None | Unset = UNSET,
    limit: int | None | Unset = UNSET,
    cursor: None | str | Unset = UNSET,

) -> Response[AuthErrorResponse | AuthErrorResponse | ErrorResponse | CacheNamespaceListApiResponse | ErrorResponse]:
    """ List cache namespaces for one owner scope.

    Args:
        owner_type (OwnerType):
        owner_ref (None | str | Unset):
        namespace (None | str | Unset):
        freshness (CacheNamespaceFreshness | None | Unset):
        limit (int | None | Unset):
        cursor (None | str | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[AuthErrorResponse | AuthErrorResponse | ErrorResponse | CacheNamespaceListApiResponse | ErrorResponse]
     """


    kwargs = _get_kwargs(
        owner_type=owner_type,
owner_ref=owner_ref,
namespace=namespace,
freshness=freshness,
limit=limit,
cursor=cursor,

    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)

def sync(
    *,
    client: AuthenticatedClient,
    owner_type: OwnerType,
    owner_ref: None | str | Unset = UNSET,
    namespace: None | str | Unset = UNSET,
    freshness: CacheNamespaceFreshness | None | Unset = UNSET,
    limit: int | None | Unset = UNSET,
    cursor: None | str | Unset = UNSET,

) -> AuthErrorResponse | AuthErrorResponse | ErrorResponse | CacheNamespaceListApiResponse | ErrorResponse | None:
    """ List cache namespaces for one owner scope.

    Args:
        owner_type (OwnerType):
        owner_ref (None | str | Unset):
        namespace (None | str | Unset):
        freshness (CacheNamespaceFreshness | None | Unset):
        limit (int | None | Unset):
        cursor (None | str | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        AuthErrorResponse | AuthErrorResponse | ErrorResponse | CacheNamespaceListApiResponse | ErrorResponse
     """


    return sync_detailed(
        client=client,
owner_type=owner_type,
owner_ref=owner_ref,
namespace=namespace,
freshness=freshness,
limit=limit,
cursor=cursor,

    ).parsed

async def asyncio_detailed(
    *,
    client: AuthenticatedClient,
    owner_type: OwnerType,
    owner_ref: None | str | Unset = UNSET,
    namespace: None | str | Unset = UNSET,
    freshness: CacheNamespaceFreshness | None | Unset = UNSET,
    limit: int | None | Unset = UNSET,
    cursor: None | str | Unset = UNSET,

) -> Response[AuthErrorResponse | AuthErrorResponse | ErrorResponse | CacheNamespaceListApiResponse | ErrorResponse]:
    """ List cache namespaces for one owner scope.

    Args:
        owner_type (OwnerType):
        owner_ref (None | str | Unset):
        namespace (None | str | Unset):
        freshness (CacheNamespaceFreshness | None | Unset):
        limit (int | None | Unset):
        cursor (None | str | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[AuthErrorResponse | AuthErrorResponse | ErrorResponse | CacheNamespaceListApiResponse | ErrorResponse]
     """


    kwargs = _get_kwargs(
        owner_type=owner_type,
owner_ref=owner_ref,
namespace=namespace,
freshness=freshness,
limit=limit,
cursor=cursor,

    )

    response = await client.get_async_httpx_client().request(
        **kwargs
    )

    return _build_response(client=client, response=response)

async def asyncio(
    *,
    client: AuthenticatedClient,
    owner_type: OwnerType,
    owner_ref: None | str | Unset = UNSET,
    namespace: None | str | Unset = UNSET,
    freshness: CacheNamespaceFreshness | None | Unset = UNSET,
    limit: int | None | Unset = UNSET,
    cursor: None | str | Unset = UNSET,

) -> AuthErrorResponse | AuthErrorResponse | ErrorResponse | CacheNamespaceListApiResponse | ErrorResponse | None:
    """ List cache namespaces for one owner scope.

    Args:
        owner_type (OwnerType):
        owner_ref (None | str | Unset):
        namespace (None | str | Unset):
        freshness (CacheNamespaceFreshness | None | Unset):
        limit (int | None | Unset):
        cursor (None | str | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        AuthErrorResponse | AuthErrorResponse | ErrorResponse | CacheNamespaceListApiResponse | ErrorResponse
     """


    return (await asyncio_detailed(
        client=client,
owner_type=owner_type,
owner_ref=owner_ref,
namespace=namespace,
freshness=freshness,
limit=limit,
cursor=cursor,

    )).parsed
