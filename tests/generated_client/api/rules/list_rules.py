from http import HTTPStatus
from typing import Any, cast

import httpx

from ... import errors
from ...client import AuthenticatedClient, Client
from ...models.paginated_response_rule_summary import PaginatedResponseRuleSummary
from ...types import UNSET, Response, Unset


def _get_kwargs(
    *,
    page: int | Unset = UNSET,
    page_size: int | Unset = UNSET,
    pack_ref: None | str | Unset = UNSET,
    action_ref: None | str | Unset = UNSET,
    trigger_ref: None | str | Unset = UNSET,
    enabled: bool | None | Unset = UNSET,
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

    json_trigger_ref: None | str | Unset
    if isinstance(trigger_ref, Unset):
        json_trigger_ref = UNSET
    else:
        json_trigger_ref = trigger_ref
    params["trigger_ref"] = json_trigger_ref

    json_enabled: bool | None | Unset
    if isinstance(enabled, Unset):
        json_enabled = UNSET
    else:
        json_enabled = enabled
    params["enabled"] = json_enabled

    params = {k: v for k, v in params.items() if v is not UNSET and v is not None}

    _kwargs: dict[str, Any] = {
        "method": "get",
        "url": "/api/v1/rules",
        "params": params,
    }

    return _kwargs


def _parse_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> Any | PaginatedResponseRuleSummary | None:
    if response.status_code == 200:
        response_200 = PaginatedResponseRuleSummary.from_dict(response.json())

        return response_200

    if response.status_code == 500:
        response_500 = cast(Any, None)
        return response_500

    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> Response[Any | PaginatedResponseRuleSummary]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    *,
    client: AuthenticatedClient | Client,
    page: int | Unset = UNSET,
    page_size: int | Unset = UNSET,
    pack_ref: None | str | Unset = UNSET,
    action_ref: None | str | Unset = UNSET,
    trigger_ref: None | str | Unset = UNSET,
    enabled: bool | None | Unset = UNSET,
) -> Response[Any | PaginatedResponseRuleSummary]:
    """List all rules with pagination

    Args:
        page (int | Unset):
        page_size (int | Unset):
        pack_ref (None | str | Unset):
        action_ref (None | str | Unset):
        trigger_ref (None | str | Unset):
        enabled (bool | None | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Any | PaginatedResponseRuleSummary]
    """

    kwargs = _get_kwargs(
        page=page,
        page_size=page_size,
        pack_ref=pack_ref,
        action_ref=action_ref,
        trigger_ref=trigger_ref,
        enabled=enabled,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    *,
    client: AuthenticatedClient | Client,
    page: int | Unset = UNSET,
    page_size: int | Unset = UNSET,
    pack_ref: None | str | Unset = UNSET,
    action_ref: None | str | Unset = UNSET,
    trigger_ref: None | str | Unset = UNSET,
    enabled: bool | None | Unset = UNSET,
) -> Any | PaginatedResponseRuleSummary | None:
    """List all rules with pagination

    Args:
        page (int | Unset):
        page_size (int | Unset):
        pack_ref (None | str | Unset):
        action_ref (None | str | Unset):
        trigger_ref (None | str | Unset):
        enabled (bool | None | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Any | PaginatedResponseRuleSummary
    """

    return sync_detailed(
        client=client,
        page=page,
        page_size=page_size,
        pack_ref=pack_ref,
        action_ref=action_ref,
        trigger_ref=trigger_ref,
        enabled=enabled,
    ).parsed


async def asyncio_detailed(
    *,
    client: AuthenticatedClient | Client,
    page: int | Unset = UNSET,
    page_size: int | Unset = UNSET,
    pack_ref: None | str | Unset = UNSET,
    action_ref: None | str | Unset = UNSET,
    trigger_ref: None | str | Unset = UNSET,
    enabled: bool | None | Unset = UNSET,
) -> Response[Any | PaginatedResponseRuleSummary]:
    """List all rules with pagination

    Args:
        page (int | Unset):
        page_size (int | Unset):
        pack_ref (None | str | Unset):
        action_ref (None | str | Unset):
        trigger_ref (None | str | Unset):
        enabled (bool | None | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Any | PaginatedResponseRuleSummary]
    """

    kwargs = _get_kwargs(
        page=page,
        page_size=page_size,
        pack_ref=pack_ref,
        action_ref=action_ref,
        trigger_ref=trigger_ref,
        enabled=enabled,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    *,
    client: AuthenticatedClient | Client,
    page: int | Unset = UNSET,
    page_size: int | Unset = UNSET,
    pack_ref: None | str | Unset = UNSET,
    action_ref: None | str | Unset = UNSET,
    trigger_ref: None | str | Unset = UNSET,
    enabled: bool | None | Unset = UNSET,
) -> Any | PaginatedResponseRuleSummary | None:
    """List all rules with pagination

    Args:
        page (int | Unset):
        page_size (int | Unset):
        pack_ref (None | str | Unset):
        action_ref (None | str | Unset):
        trigger_ref (None | str | Unset):
        enabled (bool | None | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Any | PaginatedResponseRuleSummary
    """

    return (
        await asyncio_detailed(
            client=client,
            page=page,
            page_size=page_size,
            pack_ref=pack_ref,
            action_ref=action_ref,
            trigger_ref=trigger_ref,
            enabled=enabled,
        )
    ).parsed
