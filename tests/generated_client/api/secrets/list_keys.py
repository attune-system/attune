from http import HTTPStatus
from typing import Any

import httpx

from ... import errors
from ...client import AuthenticatedClient, Client
from ...models.owner_type import OwnerType
from ...models.paginated_response_key_summary import PaginatedResponseKeySummary
from ...types import UNSET, Response, Unset


def _get_kwargs(
    *,
    owner_type: None | OwnerType | Unset = UNSET,
    owner: None | str | Unset = UNSET,
    page: int | Unset = UNSET,
    per_page: int | Unset = UNSET,
) -> dict[str, Any]:

    params: dict[str, Any] = {}

    json_owner_type: None | str | Unset
    if isinstance(owner_type, Unset):
        json_owner_type = UNSET
    elif isinstance(owner_type, OwnerType):
        json_owner_type = owner_type.value
    else:
        json_owner_type = owner_type
    params["owner_type"] = json_owner_type

    json_owner: None | str | Unset
    if isinstance(owner, Unset):
        json_owner = UNSET
    else:
        json_owner = owner
    params["owner"] = json_owner

    params["page"] = page

    params["per_page"] = per_page

    params = {k: v for k, v in params.items() if v is not UNSET and v is not None}

    _kwargs: dict[str, Any] = {
        "method": "get",
        "url": "/api/v1/keys",
        "params": params,
    }

    return _kwargs


def _parse_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> PaginatedResponseKeySummary | None:
    if response.status_code == 200:
        response_200 = PaginatedResponseKeySummary.from_dict(response.json())

        return response_200

    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> Response[PaginatedResponseKeySummary]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    *,
    client: AuthenticatedClient,
    owner_type: None | OwnerType | Unset = UNSET,
    owner: None | str | Unset = UNSET,
    page: int | Unset = UNSET,
    per_page: int | Unset = UNSET,
) -> Response[PaginatedResponseKeySummary]:
    """List all keys with pagination and optional filters (values redacted)

    Args:
        owner_type (None | OwnerType | Unset):
        owner (None | str | Unset):
        page (int | Unset):
        per_page (int | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[PaginatedResponseKeySummary]
    """

    kwargs = _get_kwargs(
        owner_type=owner_type,
        owner=owner,
        page=page,
        per_page=per_page,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    *,
    client: AuthenticatedClient,
    owner_type: None | OwnerType | Unset = UNSET,
    owner: None | str | Unset = UNSET,
    page: int | Unset = UNSET,
    per_page: int | Unset = UNSET,
) -> PaginatedResponseKeySummary | None:
    """List all keys with pagination and optional filters (values redacted)

    Args:
        owner_type (None | OwnerType | Unset):
        owner (None | str | Unset):
        page (int | Unset):
        per_page (int | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        PaginatedResponseKeySummary
    """

    return sync_detailed(
        client=client,
        owner_type=owner_type,
        owner=owner,
        page=page,
        per_page=per_page,
    ).parsed


async def asyncio_detailed(
    *,
    client: AuthenticatedClient,
    owner_type: None | OwnerType | Unset = UNSET,
    owner: None | str | Unset = UNSET,
    page: int | Unset = UNSET,
    per_page: int | Unset = UNSET,
) -> Response[PaginatedResponseKeySummary]:
    """List all keys with pagination and optional filters (values redacted)

    Args:
        owner_type (None | OwnerType | Unset):
        owner (None | str | Unset):
        page (int | Unset):
        per_page (int | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[PaginatedResponseKeySummary]
    """

    kwargs = _get_kwargs(
        owner_type=owner_type,
        owner=owner,
        page=page,
        per_page=per_page,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    *,
    client: AuthenticatedClient,
    owner_type: None | OwnerType | Unset = UNSET,
    owner: None | str | Unset = UNSET,
    page: int | Unset = UNSET,
    per_page: int | Unset = UNSET,
) -> PaginatedResponseKeySummary | None:
    """List all keys with pagination and optional filters (values redacted)

    Args:
        owner_type (None | OwnerType | Unset):
        owner (None | str | Unset):
        page (int | Unset):
        per_page (int | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        PaginatedResponseKeySummary
    """

    return (
        await asyncio_detailed(
            client=client,
            owner_type=owner_type,
            owner=owner,
            page=page,
            per_page=per_page,
        )
    ).parsed
