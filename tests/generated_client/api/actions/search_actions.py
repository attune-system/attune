from http import HTTPStatus
from typing import Any, cast

import httpx

from ... import errors
from ...client import AuthenticatedClient, Client
from ...models.paginated_response_action_search_hit import (
    PaginatedResponseActionSearchHit,
)
from ...types import UNSET, Response, Unset


def _get_kwargs(
    *,
    q: None | str | Unset = UNSET,
    packs: None | str | Unset = UNSET,
    referencing_pack_ref: None | str | Unset = UNSET,
    page: int | Unset = UNSET,
    page_size: int | Unset = UNSET,
) -> dict[str, Any]:

    params: dict[str, Any] = {}

    json_q: None | str | Unset
    if isinstance(q, Unset):
        json_q = UNSET
    else:
        json_q = q
    params["q"] = json_q

    json_packs: None | str | Unset
    if isinstance(packs, Unset):
        json_packs = UNSET
    else:
        json_packs = packs
    params["packs"] = json_packs

    json_referencing_pack_ref: None | str | Unset
    if isinstance(referencing_pack_ref, Unset):
        json_referencing_pack_ref = UNSET
    else:
        json_referencing_pack_ref = referencing_pack_ref
    params["referencing_pack_ref"] = json_referencing_pack_ref

    params["page"] = page

    params["page_size"] = page_size

    params = {k: v for k, v in params.items() if v is not UNSET and v is not None}

    _kwargs: dict[str, Any] = {
        "method": "get",
        "url": "/api/v1/actions/search",
        "params": params,
    }

    return _kwargs


def _parse_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> Any | PaginatedResponseActionSearchHit | None:
    if response.status_code == 200:
        response_200 = PaginatedResponseActionSearchHit.from_dict(response.json())

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
) -> Response[Any | PaginatedResponseActionSearchHit]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    *,
    client: AuthenticatedClient,
    q: None | str | Unset = UNSET,
    packs: None | str | Unset = UNSET,
    referencing_pack_ref: None | str | Unset = UNSET,
    page: int | Unset = UNSET,
    page_size: int | Unset = UNSET,
) -> Response[Any | PaginatedResponseActionSearchHit]:
    """Search for actions by keyword and pack filter.

     Returns lean `ActionSearchHit` rows optimized for action discovery — useful
    for AI agents and human browsing of large action catalogs. Whitespace-separated
    tokens in `q` are AND-matched (each token must appear in at least one of
    `ref`, `label`, `description`, or `pack_ref`).

    Args:
        q (None | str | Unset):
        packs (None | str | Unset):
        referencing_pack_ref (None | str | Unset):
        page (int | Unset):
        page_size (int | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Any | PaginatedResponseActionSearchHit]
    """

    kwargs = _get_kwargs(
        q=q,
        packs=packs,
        referencing_pack_ref=referencing_pack_ref,
        page=page,
        page_size=page_size,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    *,
    client: AuthenticatedClient,
    q: None | str | Unset = UNSET,
    packs: None | str | Unset = UNSET,
    referencing_pack_ref: None | str | Unset = UNSET,
    page: int | Unset = UNSET,
    page_size: int | Unset = UNSET,
) -> Any | PaginatedResponseActionSearchHit | None:
    """Search for actions by keyword and pack filter.

     Returns lean `ActionSearchHit` rows optimized for action discovery — useful
    for AI agents and human browsing of large action catalogs. Whitespace-separated
    tokens in `q` are AND-matched (each token must appear in at least one of
    `ref`, `label`, `description`, or `pack_ref`).

    Args:
        q (None | str | Unset):
        packs (None | str | Unset):
        referencing_pack_ref (None | str | Unset):
        page (int | Unset):
        page_size (int | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Any | PaginatedResponseActionSearchHit
    """

    return sync_detailed(
        client=client,
        q=q,
        packs=packs,
        referencing_pack_ref=referencing_pack_ref,
        page=page,
        page_size=page_size,
    ).parsed


async def asyncio_detailed(
    *,
    client: AuthenticatedClient,
    q: None | str | Unset = UNSET,
    packs: None | str | Unset = UNSET,
    referencing_pack_ref: None | str | Unset = UNSET,
    page: int | Unset = UNSET,
    page_size: int | Unset = UNSET,
) -> Response[Any | PaginatedResponseActionSearchHit]:
    """Search for actions by keyword and pack filter.

     Returns lean `ActionSearchHit` rows optimized for action discovery — useful
    for AI agents and human browsing of large action catalogs. Whitespace-separated
    tokens in `q` are AND-matched (each token must appear in at least one of
    `ref`, `label`, `description`, or `pack_ref`).

    Args:
        q (None | str | Unset):
        packs (None | str | Unset):
        referencing_pack_ref (None | str | Unset):
        page (int | Unset):
        page_size (int | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Any | PaginatedResponseActionSearchHit]
    """

    kwargs = _get_kwargs(
        q=q,
        packs=packs,
        referencing_pack_ref=referencing_pack_ref,
        page=page,
        page_size=page_size,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    *,
    client: AuthenticatedClient,
    q: None | str | Unset = UNSET,
    packs: None | str | Unset = UNSET,
    referencing_pack_ref: None | str | Unset = UNSET,
    page: int | Unset = UNSET,
    page_size: int | Unset = UNSET,
) -> Any | PaginatedResponseActionSearchHit | None:
    """Search for actions by keyword and pack filter.

     Returns lean `ActionSearchHit` rows optimized for action discovery — useful
    for AI agents and human browsing of large action catalogs. Whitespace-separated
    tokens in `q` are AND-matched (each token must appear in at least one of
    `ref`, `label`, `description`, or `pack_ref`).

    Args:
        q (None | str | Unset):
        packs (None | str | Unset):
        referencing_pack_ref (None | str | Unset):
        page (int | Unset):
        page_size (int | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Any | PaginatedResponseActionSearchHit
    """

    return (
        await asyncio_detailed(
            client=client,
            q=q,
            packs=packs,
            referencing_pack_ref=referencing_pack_ref,
            page=page,
            page_size=page_size,
        )
    ).parsed
