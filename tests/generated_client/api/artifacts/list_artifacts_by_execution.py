from http import HTTPStatus
from typing import Any, cast
from urllib.parse import quote

import httpx

from ... import errors
from ...client import AuthenticatedClient, Client
from ...models.artifact_cursor_page_artifact_summary import (
    ArtifactCursorPageArtifactSummary,
)
from ...models.artifact_type import ArtifactType
from ...types import UNSET, Response, Unset


def _get_kwargs(
    execution_id: int,
    *,
    cursor: None | str | Unset = UNSET,
    per_page: int | Unset = UNSET,
    type_: ArtifactType | None | Unset = UNSET,
) -> dict[str, Any]:

    params: dict[str, Any] = {}

    json_cursor: None | str | Unset
    if isinstance(cursor, Unset):
        json_cursor = UNSET
    else:
        json_cursor = cursor
    params["cursor"] = json_cursor

    params["per_page"] = per_page

    json_type_: None | str | Unset
    if isinstance(type_, Unset):
        json_type_ = UNSET
    elif isinstance(type_, ArtifactType):
        json_type_ = type_.value
    else:
        json_type_ = type_
    params["type"] = json_type_

    params = {k: v for k, v in params.items() if v is not UNSET and v is not None}

    _kwargs: dict[str, Any] = {
        "method": "get",
        "url": "/api/v1/executions/{execution_id}/artifacts".format(
            execution_id=quote(str(execution_id), safe=""),
        ),
        "params": params,
    }

    return _kwargs


def _parse_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> Any | ArtifactCursorPageArtifactSummary | None:
    if response.status_code == 200:
        response_200 = ArtifactCursorPageArtifactSummary.from_dict(response.json())

        return response_200

    if response.status_code == 400:
        response_400 = cast(Any, None)
        return response_400

    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> Response[Any | ArtifactCursorPageArtifactSummary]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    execution_id: int,
    *,
    client: AuthenticatedClient,
    cursor: None | str | Unset = UNSET,
    per_page: int | Unset = UNSET,
    type_: ArtifactType | None | Unset = UNSET,
) -> Response[Any | ArtifactCursorPageArtifactSummary]:
    """List metadata and the highest exact-execution version for each artifact.

    Args:
        execution_id (int):
        cursor (None | str | Unset):
        per_page (int | Unset):
        type_ (ArtifactType | None | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Any | ArtifactCursorPageArtifactSummary]
    """

    kwargs = _get_kwargs(
        execution_id=execution_id,
        cursor=cursor,
        per_page=per_page,
        type_=type_,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    execution_id: int,
    *,
    client: AuthenticatedClient,
    cursor: None | str | Unset = UNSET,
    per_page: int | Unset = UNSET,
    type_: ArtifactType | None | Unset = UNSET,
) -> Any | ArtifactCursorPageArtifactSummary | None:
    """List metadata and the highest exact-execution version for each artifact.

    Args:
        execution_id (int):
        cursor (None | str | Unset):
        per_page (int | Unset):
        type_ (ArtifactType | None | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Any | ArtifactCursorPageArtifactSummary
    """

    return sync_detailed(
        execution_id=execution_id,
        client=client,
        cursor=cursor,
        per_page=per_page,
        type_=type_,
    ).parsed


async def asyncio_detailed(
    execution_id: int,
    *,
    client: AuthenticatedClient,
    cursor: None | str | Unset = UNSET,
    per_page: int | Unset = UNSET,
    type_: ArtifactType | None | Unset = UNSET,
) -> Response[Any | ArtifactCursorPageArtifactSummary]:
    """List metadata and the highest exact-execution version for each artifact.

    Args:
        execution_id (int):
        cursor (None | str | Unset):
        per_page (int | Unset):
        type_ (ArtifactType | None | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Any | ArtifactCursorPageArtifactSummary]
    """

    kwargs = _get_kwargs(
        execution_id=execution_id,
        cursor=cursor,
        per_page=per_page,
        type_=type_,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    execution_id: int,
    *,
    client: AuthenticatedClient,
    cursor: None | str | Unset = UNSET,
    per_page: int | Unset = UNSET,
    type_: ArtifactType | None | Unset = UNSET,
) -> Any | ArtifactCursorPageArtifactSummary | None:
    """List metadata and the highest exact-execution version for each artifact.

    Args:
        execution_id (int):
        cursor (None | str | Unset):
        per_page (int | Unset):
        type_ (ArtifactType | None | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Any | ArtifactCursorPageArtifactSummary
    """

    return (
        await asyncio_detailed(
            execution_id=execution_id,
            client=client,
            cursor=cursor,
            per_page=per_page,
            type_=type_,
        )
    ).parsed
