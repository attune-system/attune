from http import HTTPStatus
from typing import Any

import httpx

from ... import errors
from ...client import AuthenticatedClient, Client
from ...models.execution_status import ExecutionStatus
from ...models.paginated_response_execution_summary import (
    PaginatedResponseExecutionSummary,
)
from ...types import UNSET, Response, Unset


def _get_kwargs(
    *,
    status: ExecutionStatus | None | Unset = UNSET,
    action_ref: None | str | Unset = UNSET,
    pack_name: None | str | Unset = UNSET,
    rule_ref: None | str | Unset = UNSET,
    trigger_ref: None | str | Unset = UNSET,
    executor: int | None | Unset = UNSET,
    result_contains: None | str | Unset = UNSET,
    enforcement: int | None | Unset = UNSET,
    parent: int | None | Unset = UNSET,
    top_level_only: bool | None | Unset = UNSET,
    include_total: bool | None | Unset = UNSET,
    page: int | Unset = UNSET,
    per_page: int | Unset = UNSET,
) -> dict[str, Any]:

    params: dict[str, Any] = {}

    json_status: None | str | Unset
    if isinstance(status, Unset):
        json_status = UNSET
    elif isinstance(status, ExecutionStatus):
        json_status = status.value
    else:
        json_status = status
    params["status"] = json_status

    json_action_ref: None | str | Unset
    if isinstance(action_ref, Unset):
        json_action_ref = UNSET
    else:
        json_action_ref = action_ref
    params["action_ref"] = json_action_ref

    json_pack_name: None | str | Unset
    if isinstance(pack_name, Unset):
        json_pack_name = UNSET
    else:
        json_pack_name = pack_name
    params["pack_name"] = json_pack_name

    json_rule_ref: None | str | Unset
    if isinstance(rule_ref, Unset):
        json_rule_ref = UNSET
    else:
        json_rule_ref = rule_ref
    params["rule_ref"] = json_rule_ref

    json_trigger_ref: None | str | Unset
    if isinstance(trigger_ref, Unset):
        json_trigger_ref = UNSET
    else:
        json_trigger_ref = trigger_ref
    params["trigger_ref"] = json_trigger_ref

    json_executor: int | None | Unset
    if isinstance(executor, Unset):
        json_executor = UNSET
    else:
        json_executor = executor
    params["executor"] = json_executor

    json_result_contains: None | str | Unset
    if isinstance(result_contains, Unset):
        json_result_contains = UNSET
    else:
        json_result_contains = result_contains
    params["result_contains"] = json_result_contains

    json_enforcement: int | None | Unset
    if isinstance(enforcement, Unset):
        json_enforcement = UNSET
    else:
        json_enforcement = enforcement
    params["enforcement"] = json_enforcement

    json_parent: int | None | Unset
    if isinstance(parent, Unset):
        json_parent = UNSET
    else:
        json_parent = parent
    params["parent"] = json_parent

    json_top_level_only: bool | None | Unset
    if isinstance(top_level_only, Unset):
        json_top_level_only = UNSET
    else:
        json_top_level_only = top_level_only
    params["top_level_only"] = json_top_level_only

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
        "url": "/api/v1/executions",
        "params": params,
    }

    return _kwargs


def _parse_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> PaginatedResponseExecutionSummary | None:
    if response.status_code == 200:
        response_200 = PaginatedResponseExecutionSummary.from_dict(response.json())

        return response_200

    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> Response[PaginatedResponseExecutionSummary]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    *,
    client: AuthenticatedClient,
    status: ExecutionStatus | None | Unset = UNSET,
    action_ref: None | str | Unset = UNSET,
    pack_name: None | str | Unset = UNSET,
    rule_ref: None | str | Unset = UNSET,
    trigger_ref: None | str | Unset = UNSET,
    executor: int | None | Unset = UNSET,
    result_contains: None | str | Unset = UNSET,
    enforcement: int | None | Unset = UNSET,
    parent: int | None | Unset = UNSET,
    top_level_only: bool | None | Unset = UNSET,
    include_total: bool | None | Unset = UNSET,
    page: int | Unset = UNSET,
    per_page: int | Unset = UNSET,
) -> Response[PaginatedResponseExecutionSummary]:
    """List all executions with pagination and optional filters

    Args:
        status (ExecutionStatus | None | Unset):
        action_ref (None | str | Unset):
        pack_name (None | str | Unset):
        rule_ref (None | str | Unset):
        trigger_ref (None | str | Unset):
        executor (int | None | Unset):
        result_contains (None | str | Unset):
        enforcement (int | None | Unset):
        parent (int | None | Unset):
        top_level_only (bool | None | Unset):
        include_total (bool | None | Unset):
        page (int | Unset):
        per_page (int | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[PaginatedResponseExecutionSummary]
    """

    kwargs = _get_kwargs(
        status=status,
        action_ref=action_ref,
        pack_name=pack_name,
        rule_ref=rule_ref,
        trigger_ref=trigger_ref,
        executor=executor,
        result_contains=result_contains,
        enforcement=enforcement,
        parent=parent,
        top_level_only=top_level_only,
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
    status: ExecutionStatus | None | Unset = UNSET,
    action_ref: None | str | Unset = UNSET,
    pack_name: None | str | Unset = UNSET,
    rule_ref: None | str | Unset = UNSET,
    trigger_ref: None | str | Unset = UNSET,
    executor: int | None | Unset = UNSET,
    result_contains: None | str | Unset = UNSET,
    enforcement: int | None | Unset = UNSET,
    parent: int | None | Unset = UNSET,
    top_level_only: bool | None | Unset = UNSET,
    include_total: bool | None | Unset = UNSET,
    page: int | Unset = UNSET,
    per_page: int | Unset = UNSET,
) -> PaginatedResponseExecutionSummary | None:
    """List all executions with pagination and optional filters

    Args:
        status (ExecutionStatus | None | Unset):
        action_ref (None | str | Unset):
        pack_name (None | str | Unset):
        rule_ref (None | str | Unset):
        trigger_ref (None | str | Unset):
        executor (int | None | Unset):
        result_contains (None | str | Unset):
        enforcement (int | None | Unset):
        parent (int | None | Unset):
        top_level_only (bool | None | Unset):
        include_total (bool | None | Unset):
        page (int | Unset):
        per_page (int | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        PaginatedResponseExecutionSummary
    """

    return sync_detailed(
        client=client,
        status=status,
        action_ref=action_ref,
        pack_name=pack_name,
        rule_ref=rule_ref,
        trigger_ref=trigger_ref,
        executor=executor,
        result_contains=result_contains,
        enforcement=enforcement,
        parent=parent,
        top_level_only=top_level_only,
        include_total=include_total,
        page=page,
        per_page=per_page,
    ).parsed


async def asyncio_detailed(
    *,
    client: AuthenticatedClient,
    status: ExecutionStatus | None | Unset = UNSET,
    action_ref: None | str | Unset = UNSET,
    pack_name: None | str | Unset = UNSET,
    rule_ref: None | str | Unset = UNSET,
    trigger_ref: None | str | Unset = UNSET,
    executor: int | None | Unset = UNSET,
    result_contains: None | str | Unset = UNSET,
    enforcement: int | None | Unset = UNSET,
    parent: int | None | Unset = UNSET,
    top_level_only: bool | None | Unset = UNSET,
    include_total: bool | None | Unset = UNSET,
    page: int | Unset = UNSET,
    per_page: int | Unset = UNSET,
) -> Response[PaginatedResponseExecutionSummary]:
    """List all executions with pagination and optional filters

    Args:
        status (ExecutionStatus | None | Unset):
        action_ref (None | str | Unset):
        pack_name (None | str | Unset):
        rule_ref (None | str | Unset):
        trigger_ref (None | str | Unset):
        executor (int | None | Unset):
        result_contains (None | str | Unset):
        enforcement (int | None | Unset):
        parent (int | None | Unset):
        top_level_only (bool | None | Unset):
        include_total (bool | None | Unset):
        page (int | Unset):
        per_page (int | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[PaginatedResponseExecutionSummary]
    """

    kwargs = _get_kwargs(
        status=status,
        action_ref=action_ref,
        pack_name=pack_name,
        rule_ref=rule_ref,
        trigger_ref=trigger_ref,
        executor=executor,
        result_contains=result_contains,
        enforcement=enforcement,
        parent=parent,
        top_level_only=top_level_only,
        include_total=include_total,
        page=page,
        per_page=per_page,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    *,
    client: AuthenticatedClient,
    status: ExecutionStatus | None | Unset = UNSET,
    action_ref: None | str | Unset = UNSET,
    pack_name: None | str | Unset = UNSET,
    rule_ref: None | str | Unset = UNSET,
    trigger_ref: None | str | Unset = UNSET,
    executor: int | None | Unset = UNSET,
    result_contains: None | str | Unset = UNSET,
    enforcement: int | None | Unset = UNSET,
    parent: int | None | Unset = UNSET,
    top_level_only: bool | None | Unset = UNSET,
    include_total: bool | None | Unset = UNSET,
    page: int | Unset = UNSET,
    per_page: int | Unset = UNSET,
) -> PaginatedResponseExecutionSummary | None:
    """List all executions with pagination and optional filters

    Args:
        status (ExecutionStatus | None | Unset):
        action_ref (None | str | Unset):
        pack_name (None | str | Unset):
        rule_ref (None | str | Unset):
        trigger_ref (None | str | Unset):
        executor (int | None | Unset):
        result_contains (None | str | Unset):
        enforcement (int | None | Unset):
        parent (int | None | Unset):
        top_level_only (bool | None | Unset):
        include_total (bool | None | Unset):
        page (int | Unset):
        per_page (int | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        PaginatedResponseExecutionSummary
    """

    return (
        await asyncio_detailed(
            client=client,
            status=status,
            action_ref=action_ref,
            pack_name=pack_name,
            rule_ref=rule_ref,
            trigger_ref=trigger_ref,
            executor=executor,
            result_contains=result_contains,
            enforcement=enforcement,
            parent=parent,
            top_level_only=top_level_only,
            include_total=include_total,
            page=page,
            per_page=per_page,
        )
    ).parsed
