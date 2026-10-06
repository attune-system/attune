from http import HTTPStatus
from typing import Any, cast
from urllib.parse import quote

import httpx

from ... import errors
from ...client import AuthenticatedClient, Client
from ...models.update_permission_set_request import UpdatePermissionSetRequest
from ...models.update_permission_set_response_200 import UpdatePermissionSetResponse200
from ...types import UNSET, Response, Unset


def _get_kwargs(
    id: int,
    *,
    body: UpdatePermissionSetRequest,
    dry_run: bool | Unset = UNSET,
) -> dict[str, Any]:
    headers: dict[str, Any] = {}

    params: dict[str, Any] = {}

    params["dry_run"] = dry_run

    params = {k: v for k, v in params.items() if v is not UNSET and v is not None}

    _kwargs: dict[str, Any] = {
        "method": "put",
        "url": "/api/v1/permissions/sets/{id}".format(
            id=quote(str(id), safe=""),
        ),
        "params": params,
    }

    _kwargs["json"] = body.to_dict()

    headers["Content-Type"] = "application/json"

    _kwargs["headers"] = headers
    return _kwargs


def _parse_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> Any | UpdatePermissionSetResponse200 | None:
    if response.status_code == 200:
        response_200 = UpdatePermissionSetResponse200.from_dict(response.json())

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
) -> Response[Any | UpdatePermissionSetResponse200]:
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
    body: UpdatePermissionSetRequest,
    dry_run: bool | Unset = UNSET,
) -> Response[Any | UpdatePermissionSetResponse200]:
    """
    Args:
        id (int):
        dry_run (bool | Unset):
        body (UpdatePermissionSetRequest):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Any | UpdatePermissionSetResponse200]
    """

    kwargs = _get_kwargs(
        id=id,
        body=body,
        dry_run=dry_run,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    id: int,
    *,
    client: AuthenticatedClient,
    body: UpdatePermissionSetRequest,
    dry_run: bool | Unset = UNSET,
) -> Any | UpdatePermissionSetResponse200 | None:
    """
    Args:
        id (int):
        dry_run (bool | Unset):
        body (UpdatePermissionSetRequest):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Any | UpdatePermissionSetResponse200
    """

    return sync_detailed(
        id=id,
        client=client,
        body=body,
        dry_run=dry_run,
    ).parsed


async def asyncio_detailed(
    id: int,
    *,
    client: AuthenticatedClient,
    body: UpdatePermissionSetRequest,
    dry_run: bool | Unset = UNSET,
) -> Response[Any | UpdatePermissionSetResponse200]:
    """
    Args:
        id (int):
        dry_run (bool | Unset):
        body (UpdatePermissionSetRequest):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Any | UpdatePermissionSetResponse200]
    """

    kwargs = _get_kwargs(
        id=id,
        body=body,
        dry_run=dry_run,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    id: int,
    *,
    client: AuthenticatedClient,
    body: UpdatePermissionSetRequest,
    dry_run: bool | Unset = UNSET,
) -> Any | UpdatePermissionSetResponse200 | None:
    """
    Args:
        id (int):
        dry_run (bool | Unset):
        body (UpdatePermissionSetRequest):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Any | UpdatePermissionSetResponse200
    """

    return (
        await asyncio_detailed(
            id=id,
            client=client,
            body=body,
            dry_run=dry_run,
        )
    ).parsed
