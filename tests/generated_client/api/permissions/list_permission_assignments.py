from http import HTTPStatus
from typing import Any

import httpx

from ... import errors
from ...client import AuthenticatedClient, Client
from ...models.paginated_response_permission_binding_response import (
    PaginatedResponsePermissionBindingResponse,
)
from ...types import UNSET, Response, Unset


def _get_kwargs(
    *,
    page: int | Unset = UNSET,
    page_size: int | Unset = UNSET,
    identity_id: int | None | Unset = UNSET,
    identity_login: None | str | Unset = UNSET,
    role: None | str | Unset = UNSET,
    permission_set_ref: None | str | Unset = UNSET,
) -> dict[str, Any]:

    params: dict[str, Any] = {}

    params["page"] = page

    params["page_size"] = page_size

    json_identity_id: int | None | Unset
    if isinstance(identity_id, Unset):
        json_identity_id = UNSET
    else:
        json_identity_id = identity_id
    params["identity_id"] = json_identity_id

    json_identity_login: None | str | Unset
    if isinstance(identity_login, Unset):
        json_identity_login = UNSET
    else:
        json_identity_login = identity_login
    params["identity_login"] = json_identity_login

    json_role: None | str | Unset
    if isinstance(role, Unset):
        json_role = UNSET
    else:
        json_role = role
    params["role"] = json_role

    json_permission_set_ref: None | str | Unset
    if isinstance(permission_set_ref, Unset):
        json_permission_set_ref = UNSET
    else:
        json_permission_set_ref = permission_set_ref
    params["permission_set_ref"] = json_permission_set_ref

    params = {k: v for k, v in params.items() if v is not UNSET and v is not None}

    _kwargs: dict[str, Any] = {
        "method": "get",
        "url": "/api/v1/permissions/assignments",
        "params": params,
    }

    return _kwargs


def _parse_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> PaginatedResponsePermissionBindingResponse | None:
    if response.status_code == 200:
        response_200 = PaginatedResponsePermissionBindingResponse.from_dict(
            response.json()
        )

        return response_200

    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> Response[PaginatedResponsePermissionBindingResponse]:
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
    identity_id: int | None | Unset = UNSET,
    identity_login: None | str | Unset = UNSET,
    role: None | str | Unset = UNSET,
    permission_set_ref: None | str | Unset = UNSET,
) -> Response[PaginatedResponsePermissionBindingResponse]:
    """
    Args:
        page (int | Unset):
        page_size (int | Unset):
        identity_id (int | None | Unset):
        identity_login (None | str | Unset):
        role (None | str | Unset):
        permission_set_ref (None | str | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[PaginatedResponsePermissionBindingResponse]
    """

    kwargs = _get_kwargs(
        page=page,
        page_size=page_size,
        identity_id=identity_id,
        identity_login=identity_login,
        role=role,
        permission_set_ref=permission_set_ref,
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
    identity_id: int | None | Unset = UNSET,
    identity_login: None | str | Unset = UNSET,
    role: None | str | Unset = UNSET,
    permission_set_ref: None | str | Unset = UNSET,
) -> PaginatedResponsePermissionBindingResponse | None:
    """
    Args:
        page (int | Unset):
        page_size (int | Unset):
        identity_id (int | None | Unset):
        identity_login (None | str | Unset):
        role (None | str | Unset):
        permission_set_ref (None | str | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        PaginatedResponsePermissionBindingResponse
    """

    return sync_detailed(
        client=client,
        page=page,
        page_size=page_size,
        identity_id=identity_id,
        identity_login=identity_login,
        role=role,
        permission_set_ref=permission_set_ref,
    ).parsed


async def asyncio_detailed(
    *,
    client: AuthenticatedClient,
    page: int | Unset = UNSET,
    page_size: int | Unset = UNSET,
    identity_id: int | None | Unset = UNSET,
    identity_login: None | str | Unset = UNSET,
    role: None | str | Unset = UNSET,
    permission_set_ref: None | str | Unset = UNSET,
) -> Response[PaginatedResponsePermissionBindingResponse]:
    """
    Args:
        page (int | Unset):
        page_size (int | Unset):
        identity_id (int | None | Unset):
        identity_login (None | str | Unset):
        role (None | str | Unset):
        permission_set_ref (None | str | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[PaginatedResponsePermissionBindingResponse]
    """

    kwargs = _get_kwargs(
        page=page,
        page_size=page_size,
        identity_id=identity_id,
        identity_login=identity_login,
        role=role,
        permission_set_ref=permission_set_ref,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    *,
    client: AuthenticatedClient,
    page: int | Unset = UNSET,
    page_size: int | Unset = UNSET,
    identity_id: int | None | Unset = UNSET,
    identity_login: None | str | Unset = UNSET,
    role: None | str | Unset = UNSET,
    permission_set_ref: None | str | Unset = UNSET,
) -> PaginatedResponsePermissionBindingResponse | None:
    """
    Args:
        page (int | Unset):
        page_size (int | Unset):
        identity_id (int | None | Unset):
        identity_login (None | str | Unset):
        role (None | str | Unset):
        permission_set_ref (None | str | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        PaginatedResponsePermissionBindingResponse
    """

    return (
        await asyncio_detailed(
            client=client,
            page=page,
            page_size=page_size,
            identity_id=identity_id,
            identity_login=identity_login,
            role=role,
            permission_set_ref=permission_set_ref,
        )
    ).parsed
