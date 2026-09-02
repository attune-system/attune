from http import HTTPStatus
from typing import Any, cast
from urllib.parse import quote

import httpx

from ... import errors
from ...client import AuthenticatedClient, Client
from ...models.api_response_work_queue_response import ApiResponseWorkQueueResponse
from ...types import UNSET, Response, Unset


def _get_kwargs(
    ref: str,
    *,
    enabled: bool | Unset = UNSET,
    is_adhoc: bool | Unset = UNSET,
    search: str | Unset = UNSET,
    referencing_pack_ref: str | Unset = UNSET,
    page: int | Unset = UNSET,
    per_page: int | Unset = UNSET,
) -> dict[str, Any]:

    params: dict[str, Any] = {}

    params["enabled"] = enabled

    params["is_adhoc"] = is_adhoc

    params["search"] = search

    params["referencing_pack_ref"] = referencing_pack_ref

    params["page"] = page

    params["per_page"] = per_page

    params = {k: v for k, v in params.items() if v is not UNSET and v is not None}

    _kwargs: dict[str, Any] = {
        "method": "get",
        "url": "/api/v1/queues/{ref}".format(
            ref=quote(str(ref), safe=""),
        ),
        "params": params,
    }

    return _kwargs


def _parse_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> Any | ApiResponseWorkQueueResponse | None:
    if response.status_code == 200:
        response_200 = ApiResponseWorkQueueResponse.from_dict(response.json())

        return response_200

    if response.status_code == 404:
        response_404 = cast(Any, None)
        return response_404

    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> Response[Any | ApiResponseWorkQueueResponse]:
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
    enabled: bool | Unset = UNSET,
    is_adhoc: bool | Unset = UNSET,
    search: str | Unset = UNSET,
    referencing_pack_ref: str | Unset = UNSET,
    page: int | Unset = UNSET,
    per_page: int | Unset = UNSET,
) -> Response[Any | ApiResponseWorkQueueResponse]:
    """
    Args:
        ref (str):
        enabled (bool | Unset):
        is_adhoc (bool | Unset):
        search (str | Unset):
        referencing_pack_ref (str | Unset):
        page (int | Unset):
        per_page (int | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Any | ApiResponseWorkQueueResponse]
    """

    kwargs = _get_kwargs(
        ref=ref,
        enabled=enabled,
        is_adhoc=is_adhoc,
        search=search,
        referencing_pack_ref=referencing_pack_ref,
        page=page,
        per_page=per_page,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    ref: str,
    *,
    client: AuthenticatedClient,
    enabled: bool | Unset = UNSET,
    is_adhoc: bool | Unset = UNSET,
    search: str | Unset = UNSET,
    referencing_pack_ref: str | Unset = UNSET,
    page: int | Unset = UNSET,
    per_page: int | Unset = UNSET,
) -> Any | ApiResponseWorkQueueResponse | None:
    """
    Args:
        ref (str):
        enabled (bool | Unset):
        is_adhoc (bool | Unset):
        search (str | Unset):
        referencing_pack_ref (str | Unset):
        page (int | Unset):
        per_page (int | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Any | ApiResponseWorkQueueResponse
    """

    return sync_detailed(
        ref=ref,
        client=client,
        enabled=enabled,
        is_adhoc=is_adhoc,
        search=search,
        referencing_pack_ref=referencing_pack_ref,
        page=page,
        per_page=per_page,
    ).parsed


async def asyncio_detailed(
    ref: str,
    *,
    client: AuthenticatedClient,
    enabled: bool | Unset = UNSET,
    is_adhoc: bool | Unset = UNSET,
    search: str | Unset = UNSET,
    referencing_pack_ref: str | Unset = UNSET,
    page: int | Unset = UNSET,
    per_page: int | Unset = UNSET,
) -> Response[Any | ApiResponseWorkQueueResponse]:
    """
    Args:
        ref (str):
        enabled (bool | Unset):
        is_adhoc (bool | Unset):
        search (str | Unset):
        referencing_pack_ref (str | Unset):
        page (int | Unset):
        per_page (int | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Any | ApiResponseWorkQueueResponse]
    """

    kwargs = _get_kwargs(
        ref=ref,
        enabled=enabled,
        is_adhoc=is_adhoc,
        search=search,
        referencing_pack_ref=referencing_pack_ref,
        page=page,
        per_page=per_page,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    ref: str,
    *,
    client: AuthenticatedClient,
    enabled: bool | Unset = UNSET,
    is_adhoc: bool | Unset = UNSET,
    search: str | Unset = UNSET,
    referencing_pack_ref: str | Unset = UNSET,
    page: int | Unset = UNSET,
    per_page: int | Unset = UNSET,
) -> Any | ApiResponseWorkQueueResponse | None:
    """
    Args:
        ref (str):
        enabled (bool | Unset):
        is_adhoc (bool | Unset):
        search (str | Unset):
        referencing_pack_ref (str | Unset):
        page (int | Unset):
        per_page (int | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Any | ApiResponseWorkQueueResponse
    """

    return (
        await asyncio_detailed(
            ref=ref,
            client=client,
            enabled=enabled,
            is_adhoc=is_adhoc,
            search=search,
            referencing_pack_ref=referencing_pack_ref,
            page=page,
            per_page=per_page,
        )
    ).parsed
