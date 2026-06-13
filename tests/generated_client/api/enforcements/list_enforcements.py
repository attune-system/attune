from http import HTTPStatus
from typing import Any, cast

import httpx

from ... import errors
from ...client import AuthenticatedClient, Client
from ...models.enforcement_status import EnforcementStatus
from ...models.paginated_response_enforcement_summary import (
    PaginatedResponseEnforcementSummary,
)
from ...types import UNSET, Response, Unset


def _get_kwargs(
    *,
    rule: int | None | Unset = UNSET,
    event: int | None | Unset = UNSET,
    status: EnforcementStatus | None | Unset = UNSET,
    trigger_ref: None | str | Unset = UNSET,
    rule_ref: None | str | Unset = UNSET,
    include_total: bool | None | Unset = UNSET,
    page: int | Unset = UNSET,
    per_page: int | Unset = UNSET,
) -> dict[str, Any]:

    params: dict[str, Any] = {}

    json_rule: int | None | Unset
    if isinstance(rule, Unset):
        json_rule = UNSET
    else:
        json_rule = rule
    params["rule"] = json_rule

    json_event: int | None | Unset
    if isinstance(event, Unset):
        json_event = UNSET
    else:
        json_event = event
    params["event"] = json_event

    json_status: None | str | Unset
    if isinstance(status, Unset):
        json_status = UNSET
    elif isinstance(status, EnforcementStatus):
        json_status = status.value
    else:
        json_status = status
    params["status"] = json_status

    json_trigger_ref: None | str | Unset
    if isinstance(trigger_ref, Unset):
        json_trigger_ref = UNSET
    else:
        json_trigger_ref = trigger_ref
    params["trigger_ref"] = json_trigger_ref

    json_rule_ref: None | str | Unset
    if isinstance(rule_ref, Unset):
        json_rule_ref = UNSET
    else:
        json_rule_ref = rule_ref
    params["rule_ref"] = json_rule_ref

    json_include_total: bool | None | Unset
    if isinstance(include_total, Unset):
        json_include_total = UNSET
    else:
        json_include_total = include_total
    params["include_total"] = json_include_total

    params["page"] = page

    params["per_page"] = per_page

    params = {k: v for k, v in params.items() if v is not UNSET and v is not None}

    _kwargs: dict[str, Any] = {
        "method": "get",
        "url": "/api/v1/enforcements",
        "params": params,
    }

    return _kwargs


def _parse_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> Any | PaginatedResponseEnforcementSummary | None:
    if response.status_code == 200:
        response_200 = PaginatedResponseEnforcementSummary.from_dict(response.json())

        return response_200

    if response.status_code == 401:
        response_401 = cast(Any, None)
        return response_401

    if response.status_code == 500:
        response_500 = cast(Any, None)
        return response_500

    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> Response[Any | PaginatedResponseEnforcementSummary]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    *,
    client: AuthenticatedClient,
    rule: int | None | Unset = UNSET,
    event: int | None | Unset = UNSET,
    status: EnforcementStatus | None | Unset = UNSET,
    trigger_ref: None | str | Unset = UNSET,
    rule_ref: None | str | Unset = UNSET,
    include_total: bool | None | Unset = UNSET,
    page: int | Unset = UNSET,
    per_page: int | Unset = UNSET,
) -> Response[Any | PaginatedResponseEnforcementSummary]:
    """List all enforcements with pagination and optional filters

    Args:
        rule (int | None | Unset):
        event (int | None | Unset):
        status (EnforcementStatus | None | Unset):
        trigger_ref (None | str | Unset):
        rule_ref (None | str | Unset):
        include_total (bool | None | Unset):
        page (int | Unset):
        per_page (int | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Any | PaginatedResponseEnforcementSummary]
    """

    kwargs = _get_kwargs(
        rule=rule,
        event=event,
        status=status,
        trigger_ref=trigger_ref,
        rule_ref=rule_ref,
        include_total=include_total,
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
    rule: int | None | Unset = UNSET,
    event: int | None | Unset = UNSET,
    status: EnforcementStatus | None | Unset = UNSET,
    trigger_ref: None | str | Unset = UNSET,
    rule_ref: None | str | Unset = UNSET,
    include_total: bool | None | Unset = UNSET,
    page: int | Unset = UNSET,
    per_page: int | Unset = UNSET,
) -> Any | PaginatedResponseEnforcementSummary | None:
    """List all enforcements with pagination and optional filters

    Args:
        rule (int | None | Unset):
        event (int | None | Unset):
        status (EnforcementStatus | None | Unset):
        trigger_ref (None | str | Unset):
        rule_ref (None | str | Unset):
        include_total (bool | None | Unset):
        page (int | Unset):
        per_page (int | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Any | PaginatedResponseEnforcementSummary
    """

    return sync_detailed(
        client=client,
        rule=rule,
        event=event,
        status=status,
        trigger_ref=trigger_ref,
        rule_ref=rule_ref,
        include_total=include_total,
        page=page,
        per_page=per_page,
    ).parsed


async def asyncio_detailed(
    *,
    client: AuthenticatedClient,
    rule: int | None | Unset = UNSET,
    event: int | None | Unset = UNSET,
    status: EnforcementStatus | None | Unset = UNSET,
    trigger_ref: None | str | Unset = UNSET,
    rule_ref: None | str | Unset = UNSET,
    include_total: bool | None | Unset = UNSET,
    page: int | Unset = UNSET,
    per_page: int | Unset = UNSET,
) -> Response[Any | PaginatedResponseEnforcementSummary]:
    """List all enforcements with pagination and optional filters

    Args:
        rule (int | None | Unset):
        event (int | None | Unset):
        status (EnforcementStatus | None | Unset):
        trigger_ref (None | str | Unset):
        rule_ref (None | str | Unset):
        include_total (bool | None | Unset):
        page (int | Unset):
        per_page (int | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Any | PaginatedResponseEnforcementSummary]
    """

    kwargs = _get_kwargs(
        rule=rule,
        event=event,
        status=status,
        trigger_ref=trigger_ref,
        rule_ref=rule_ref,
        include_total=include_total,
        page=page,
        per_page=per_page,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    *,
    client: AuthenticatedClient,
    rule: int | None | Unset = UNSET,
    event: int | None | Unset = UNSET,
    status: EnforcementStatus | None | Unset = UNSET,
    trigger_ref: None | str | Unset = UNSET,
    rule_ref: None | str | Unset = UNSET,
    include_total: bool | None | Unset = UNSET,
    page: int | Unset = UNSET,
    per_page: int | Unset = UNSET,
) -> Any | PaginatedResponseEnforcementSummary | None:
    """List all enforcements with pagination and optional filters

    Args:
        rule (int | None | Unset):
        event (int | None | Unset):
        status (EnforcementStatus | None | Unset):
        trigger_ref (None | str | Unset):
        rule_ref (None | str | Unset):
        include_total (bool | None | Unset):
        page (int | Unset):
        per_page (int | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Any | PaginatedResponseEnforcementSummary
    """

    return (
        await asyncio_detailed(
            client=client,
            rule=rule,
            event=event,
            status=status,
            trigger_ref=trigger_ref,
            rule_ref=rule_ref,
            include_total=include_total,
            page=page,
            per_page=per_page,
        )
    ).parsed
