from http import HTTPStatus
from typing import Any

import httpx

from ... import errors
from ...client import AuthenticatedClient, Client
from ...models.paginated_response_policy_response import PaginatedResponsePolicyResponse
from ...models.policy_scope_kind import PolicyScopeKind
from ...types import UNSET, Response, Unset


def _get_kwargs(
    *,
    page: int | Unset = UNSET,
    page_size: int | Unset = UNSET,
    scope: None | PolicyScopeKind | Unset = UNSET,
    pack_ref: None | str | Unset = UNSET,
    action_ref: None | str | Unset = UNSET,
) -> dict[str, Any]:

    params: dict[str, Any] = {}

    params["page"] = page

    params["page_size"] = page_size

    json_scope: None | str | Unset
    if isinstance(scope, Unset):
        json_scope = UNSET
    elif isinstance(scope, PolicyScopeKind):
        json_scope = scope.value
    else:
        json_scope = scope
    params["scope"] = json_scope

    json_pack_ref: None | str | Unset
    if isinstance(pack_ref, Unset):
        json_pack_ref = UNSET
    else:
        json_pack_ref = pack_ref
    params["pack_ref"] = json_pack_ref

    json_action_ref: None | str | Unset
    if isinstance(action_ref, Unset):
        json_action_ref = UNSET
    else:
        json_action_ref = action_ref
    params["action_ref"] = json_action_ref

    params = {k: v for k, v in params.items() if v is not UNSET and v is not None}

    _kwargs: dict[str, Any] = {
        "method": "get",
        "url": "/api/v1/policies",
        "params": params,
    }

    return _kwargs


def _parse_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> PaginatedResponsePolicyResponse | None:
    if response.status_code == 200:
        response_200 = PaginatedResponsePolicyResponse.from_dict(response.json())

        return response_200

    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> Response[PaginatedResponsePolicyResponse]:
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
    scope: None | PolicyScopeKind | Unset = UNSET,
    pack_ref: None | str | Unset = UNSET,
    action_ref: None | str | Unset = UNSET,
) -> Response[PaginatedResponsePolicyResponse]:
    """
    Args:
        page (int | Unset):
        page_size (int | Unset):
        scope (None | PolicyScopeKind | Unset):
        pack_ref (None | str | Unset):
        action_ref (None | str | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[PaginatedResponsePolicyResponse]
    """

    kwargs = _get_kwargs(
        page=page,
        page_size=page_size,
        scope=scope,
        pack_ref=pack_ref,
        action_ref=action_ref,
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
    scope: None | PolicyScopeKind | Unset = UNSET,
    pack_ref: None | str | Unset = UNSET,
    action_ref: None | str | Unset = UNSET,
) -> PaginatedResponsePolicyResponse | None:
    """
    Args:
        page (int | Unset):
        page_size (int | Unset):
        scope (None | PolicyScopeKind | Unset):
        pack_ref (None | str | Unset):
        action_ref (None | str | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        PaginatedResponsePolicyResponse
    """

    return sync_detailed(
        client=client,
        page=page,
        page_size=page_size,
        scope=scope,
        pack_ref=pack_ref,
        action_ref=action_ref,
    ).parsed


async def asyncio_detailed(
    *,
    client: AuthenticatedClient,
    page: int | Unset = UNSET,
    page_size: int | Unset = UNSET,
    scope: None | PolicyScopeKind | Unset = UNSET,
    pack_ref: None | str | Unset = UNSET,
    action_ref: None | str | Unset = UNSET,
) -> Response[PaginatedResponsePolicyResponse]:
    """
    Args:
        page (int | Unset):
        page_size (int | Unset):
        scope (None | PolicyScopeKind | Unset):
        pack_ref (None | str | Unset):
        action_ref (None | str | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[PaginatedResponsePolicyResponse]
    """

    kwargs = _get_kwargs(
        page=page,
        page_size=page_size,
        scope=scope,
        pack_ref=pack_ref,
        action_ref=action_ref,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    *,
    client: AuthenticatedClient,
    page: int | Unset = UNSET,
    page_size: int | Unset = UNSET,
    scope: None | PolicyScopeKind | Unset = UNSET,
    pack_ref: None | str | Unset = UNSET,
    action_ref: None | str | Unset = UNSET,
) -> PaginatedResponsePolicyResponse | None:
    """
    Args:
        page (int | Unset):
        page_size (int | Unset):
        scope (None | PolicyScopeKind | Unset):
        pack_ref (None | str | Unset):
        action_ref (None | str | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        PaginatedResponsePolicyResponse
    """

    return (
        await asyncio_detailed(
            client=client,
            page=page,
            page_size=page_size,
            scope=scope,
            pack_ref=pack_ref,
            action_ref=action_ref,
        )
    ).parsed
