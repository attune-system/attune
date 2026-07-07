from http import HTTPStatus
from typing import Any, cast
from urllib.parse import quote

import httpx

from ...client import AuthenticatedClient, Client
from ...types import Response, UNSET
from ... import errors

from ...models.paginated_response_policy_summary import PaginatedResponsePolicySummary
from ...models.policy_scope_type import PolicyScopeType
from ...types import UNSET, Unset
from typing import cast



def _get_kwargs(
    *,
    page: int | Unset = UNSET,
    page_size: int | Unset = UNSET,
    pack_ref: None | str | Unset = UNSET,
    action_ref: None | str | Unset = UNSET,
    scope: None | PolicyScopeType | Unset = UNSET,
    enabled: bool | None | Unset = UNSET,
    tag: None | str | Unset = UNSET,

) -> dict[str, Any]:
    

    

    params: dict[str, Any] = {}

    params["page"] = page

    params["page_size"] = page_size

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

    json_scope: None | str | Unset
    if isinstance(scope, Unset):
        json_scope = UNSET
    elif isinstance(scope, PolicyScopeType):
        json_scope = scope.value
    else:
        json_scope = scope
    params["scope"] = json_scope

    json_enabled: bool | None | Unset
    if isinstance(enabled, Unset):
        json_enabled = UNSET
    else:
        json_enabled = enabled
    params["enabled"] = json_enabled

    json_tag: None | str | Unset
    if isinstance(tag, Unset):
        json_tag = UNSET
    else:
        json_tag = tag
    params["tag"] = json_tag


    params = {k: v for k, v in params.items() if v is not UNSET and v is not None}


    _kwargs: dict[str, Any] = {
        "method": "get",
        "url": "/api/v1/policies",
        "params": params,
    }


    return _kwargs



def _parse_response(*, client: AuthenticatedClient | Client, response: httpx.Response) -> PaginatedResponsePolicySummary | None:
    if response.status_code == 200:
        response_200 = PaginatedResponsePolicySummary.from_dict(response.json())



        return response_200

    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(*, client: AuthenticatedClient | Client, response: httpx.Response) -> Response[PaginatedResponsePolicySummary]:
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
    pack_ref: None | str | Unset = UNSET,
    action_ref: None | str | Unset = UNSET,
    scope: None | PolicyScopeType | Unset = UNSET,
    enabled: bool | None | Unset = UNSET,
    tag: None | str | Unset = UNSET,

) -> Response[PaginatedResponsePolicySummary]:
    """ 
    Args:
        page (int | Unset):
        page_size (int | Unset):
        pack_ref (None | str | Unset):
        action_ref (None | str | Unset):
        scope (None | PolicyScopeType | Unset):
        enabled (bool | None | Unset):
        tag (None | str | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[PaginatedResponsePolicySummary]
     """


    kwargs = _get_kwargs(
        page=page,
page_size=page_size,
pack_ref=pack_ref,
action_ref=action_ref,
scope=scope,
enabled=enabled,
tag=tag,

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
    pack_ref: None | str | Unset = UNSET,
    action_ref: None | str | Unset = UNSET,
    scope: None | PolicyScopeType | Unset = UNSET,
    enabled: bool | None | Unset = UNSET,
    tag: None | str | Unset = UNSET,

) -> PaginatedResponsePolicySummary | None:
    """ 
    Args:
        page (int | Unset):
        page_size (int | Unset):
        pack_ref (None | str | Unset):
        action_ref (None | str | Unset):
        scope (None | PolicyScopeType | Unset):
        enabled (bool | None | Unset):
        tag (None | str | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        PaginatedResponsePolicySummary
     """


    return sync_detailed(
        client=client,
page=page,
page_size=page_size,
pack_ref=pack_ref,
action_ref=action_ref,
scope=scope,
enabled=enabled,
tag=tag,

    ).parsed

async def asyncio_detailed(
    *,
    client: AuthenticatedClient,
    page: int | Unset = UNSET,
    page_size: int | Unset = UNSET,
    pack_ref: None | str | Unset = UNSET,
    action_ref: None | str | Unset = UNSET,
    scope: None | PolicyScopeType | Unset = UNSET,
    enabled: bool | None | Unset = UNSET,
    tag: None | str | Unset = UNSET,

) -> Response[PaginatedResponsePolicySummary]:
    """ 
    Args:
        page (int | Unset):
        page_size (int | Unset):
        pack_ref (None | str | Unset):
        action_ref (None | str | Unset):
        scope (None | PolicyScopeType | Unset):
        enabled (bool | None | Unset):
        tag (None | str | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[PaginatedResponsePolicySummary]
     """


    kwargs = _get_kwargs(
        page=page,
page_size=page_size,
pack_ref=pack_ref,
action_ref=action_ref,
scope=scope,
enabled=enabled,
tag=tag,

    )

    response = await client.get_async_httpx_client().request(
        **kwargs
    )

    return _build_response(client=client, response=response)

async def asyncio(
    *,
    client: AuthenticatedClient,
    page: int | Unset = UNSET,
    page_size: int | Unset = UNSET,
    pack_ref: None | str | Unset = UNSET,
    action_ref: None | str | Unset = UNSET,
    scope: None | PolicyScopeType | Unset = UNSET,
    enabled: bool | None | Unset = UNSET,
    tag: None | str | Unset = UNSET,

) -> PaginatedResponsePolicySummary | None:
    """ 
    Args:
        page (int | Unset):
        page_size (int | Unset):
        pack_ref (None | str | Unset):
        action_ref (None | str | Unset):
        scope (None | PolicyScopeType | Unset):
        enabled (bool | None | Unset):
        tag (None | str | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        PaginatedResponsePolicySummary
     """


    return (await asyncio_detailed(
        client=client,
page=page,
page_size=page_size,
pack_ref=pack_ref,
action_ref=action_ref,
scope=scope,
enabled=enabled,
tag=tag,

    )).parsed
