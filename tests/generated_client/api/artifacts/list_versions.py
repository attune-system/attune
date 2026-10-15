from http import HTTPStatus
from typing import Any, cast
from urllib.parse import quote

import httpx

from ... import errors
from ...client import AuthenticatedClient, Client
from ...models.paginated_response_artifact_version_summary import (
    PaginatedResponseArtifactVersionSummary,
)
from ...types import UNSET, Response, Unset


def _get_kwargs(
    id: int,
    *,
    page: int | Unset = UNSET,
    per_page: int | Unset = UNSET,
) -> dict[str, Any]:

    params: dict[str, Any] = {}

    params["page"] = page

    params["per_page"] = per_page

    params = {k: v for k, v in params.items() if v is not UNSET and v is not None}

    _kwargs: dict[str, Any] = {
        "method": "get",
        "url": "/api/v1/artifacts/{id}/versions".format(
            id=quote(str(id), safe=""),
        ),
        "params": params,
    }

    return _kwargs


def _parse_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> Any | PaginatedResponseArtifactVersionSummary | None:
    if response.status_code == 200:
        response_200 = PaginatedResponseArtifactVersionSummary.from_dict(
            response.json()
        )

        return response_200

    if response.status_code == 400:
        response_400 = cast(Any, None)
        return response_400

    if response.status_code == 404:
        response_404 = cast(Any, None)
        return response_404

    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> Response[Any | PaginatedResponseArtifactVersionSummary]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    id: int,
    *,
    client: AuthenticatedClient,
    page: int | Unset = UNSET,
    per_page: int | Unset = UNSET,
) -> Response[Any | PaginatedResponseArtifactVersionSummary]:
    """List a visibility-filtered page of versions without binary/JSON content.

    Args:
        id (int):
        page (int | Unset):
        per_page (int | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Any | PaginatedResponseArtifactVersionSummary]
    """

    kwargs = _get_kwargs(
        id=id,
        page=page,
        per_page=per_page,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    id: int,
    *,
    client: AuthenticatedClient,
    page: int | Unset = UNSET,
    per_page: int | Unset = UNSET,
) -> Any | PaginatedResponseArtifactVersionSummary | None:
    """List a visibility-filtered page of versions without binary/JSON content.

    Args:
        id (int):
        page (int | Unset):
        per_page (int | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Any | PaginatedResponseArtifactVersionSummary
    """

    return sync_detailed(
        id=id,
        client=client,
        page=page,
        per_page=per_page,
    ).parsed


async def asyncio_detailed(
    id: int,
    *,
    client: AuthenticatedClient,
    page: int | Unset = UNSET,
    per_page: int | Unset = UNSET,
) -> Response[Any | PaginatedResponseArtifactVersionSummary]:
    """List a visibility-filtered page of versions without binary/JSON content.

    Args:
        id (int):
        page (int | Unset):
        per_page (int | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Any | PaginatedResponseArtifactVersionSummary]
    """

    kwargs = _get_kwargs(
        id=id,
        page=page,
        per_page=per_page,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    id: int,
    *,
    client: AuthenticatedClient,
    page: int | Unset = UNSET,
    per_page: int | Unset = UNSET,
) -> Any | PaginatedResponseArtifactVersionSummary | None:
    """List a visibility-filtered page of versions without binary/JSON content.

    Args:
        id (int):
        page (int | Unset):
        per_page (int | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Any | PaginatedResponseArtifactVersionSummary
    """

    return (
        await asyncio_detailed(
            id=id,
            client=client,
            page=page,
            per_page=per_page,
        )
    ).parsed
