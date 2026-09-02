from http import HTTPStatus
from typing import Any

import httpx

from ... import errors
from ...client import AuthenticatedClient, Client
from ...models.paginated_response_workflow_summary import (
    PaginatedResponseWorkflowSummary,
)
from ...types import UNSET, Response, Unset


def _get_kwargs(
    *,
    page: int | Unset = UNSET,
    page_size: int | Unset = UNSET,
    tags: None | str | Unset = UNSET,
    search: None | str | Unset = UNSET,
    pack_ref: None | str | Unset = UNSET,
) -> dict[str, Any]:

    params: dict[str, Any] = {}

    params["page"] = page

    params["page_size"] = page_size

    json_tags: None | str | Unset
    if isinstance(tags, Unset):
        json_tags = UNSET
    else:
        json_tags = tags
    params["tags"] = json_tags

    json_search: None | str | Unset
    if isinstance(search, Unset):
        json_search = UNSET
    else:
        json_search = search
    params["search"] = json_search

    json_pack_ref: None | str | Unset
    if isinstance(pack_ref, Unset):
        json_pack_ref = UNSET
    else:
        json_pack_ref = pack_ref
    params["pack_ref"] = json_pack_ref

    params = {k: v for k, v in params.items() if v is not UNSET and v is not None}

    _kwargs: dict[str, Any] = {
        "method": "get",
        "url": "/api/v1/workflows",
        "params": params,
    }

    return _kwargs


def _parse_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> PaginatedResponseWorkflowSummary | None:
    if response.status_code == 200:
        response_200 = PaginatedResponseWorkflowSummary.from_dict(response.json())

        return response_200

    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> Response[PaginatedResponseWorkflowSummary]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    *,
    client: AuthenticatedClient,
    page: int | Unset = UNSET,
    page_size: int | Unset = UNSET,
    tags: None | str | Unset = UNSET,
    search: None | str | Unset = UNSET,
    pack_ref: None | str | Unset = UNSET,
) -> Response[PaginatedResponseWorkflowSummary]:
    """List all workflows with pagination and filtering

    Args:
        page (int | Unset):
        page_size (int | Unset):
        tags (None | str | Unset):
        search (None | str | Unset):
        pack_ref (None | str | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[PaginatedResponseWorkflowSummary]
    """

    kwargs = _get_kwargs(
        page=page,
        page_size=page_size,
        tags=tags,
        search=search,
        pack_ref=pack_ref,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    *,
    client: AuthenticatedClient,
    page: int | Unset = UNSET,
    page_size: int | Unset = UNSET,
    tags: None | str | Unset = UNSET,
    search: None | str | Unset = UNSET,
    pack_ref: None | str | Unset = UNSET,
) -> PaginatedResponseWorkflowSummary | None:
    """List all workflows with pagination and filtering

    Args:
        page (int | Unset):
        page_size (int | Unset):
        tags (None | str | Unset):
        search (None | str | Unset):
        pack_ref (None | str | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        PaginatedResponseWorkflowSummary
    """

    return sync_detailed(
        client=client,
        page=page,
        page_size=page_size,
        tags=tags,
        search=search,
        pack_ref=pack_ref,
    ).parsed


async def asyncio_detailed(
    *,
    client: AuthenticatedClient,
    page: int | Unset = UNSET,
    page_size: int | Unset = UNSET,
    tags: None | str | Unset = UNSET,
    search: None | str | Unset = UNSET,
    pack_ref: None | str | Unset = UNSET,
) -> Response[PaginatedResponseWorkflowSummary]:
    """List all workflows with pagination and filtering

    Args:
        page (int | Unset):
        page_size (int | Unset):
        tags (None | str | Unset):
        search (None | str | Unset):
        pack_ref (None | str | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[PaginatedResponseWorkflowSummary]
    """

    kwargs = _get_kwargs(
        page=page,
        page_size=page_size,
        tags=tags,
        search=search,
        pack_ref=pack_ref,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    *,
    client: AuthenticatedClient,
    page: int | Unset = UNSET,
    page_size: int | Unset = UNSET,
    tags: None | str | Unset = UNSET,
    search: None | str | Unset = UNSET,
    pack_ref: None | str | Unset = UNSET,
) -> PaginatedResponseWorkflowSummary | None:
    """List all workflows with pagination and filtering

    Args:
        page (int | Unset):
        page_size (int | Unset):
        tags (None | str | Unset):
        search (None | str | Unset):
        pack_ref (None | str | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        PaginatedResponseWorkflowSummary
    """

    return (
        await asyncio_detailed(
            client=client,
            page=page,
            page_size=page_size,
            tags=tags,
            search=search,
            pack_ref=pack_ref,
        )
    ).parsed
