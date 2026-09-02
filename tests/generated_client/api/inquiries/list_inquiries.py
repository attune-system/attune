from http import HTTPStatus
from typing import Any, cast

import httpx

from ... import errors
from ...client import AuthenticatedClient, Client
from ...models.inquiry_status import InquiryStatus
from ...models.paginated_response_inquiry_summary import PaginatedResponseInquirySummary
from ...types import UNSET, Response, Unset


def _get_kwargs(
    *,
    status: InquiryStatus | None | Unset = UNSET,
    execution: int | None | Unset = UNSET,
    assigned_to: int | None | Unset = UNSET,
    offset: int | None | Unset = UNSET,
    limit: int | None | Unset = UNSET,
) -> dict[str, Any]:

    params: dict[str, Any] = {}

    json_status: None | str | Unset
    if isinstance(status, Unset):
        json_status = UNSET
    elif isinstance(status, InquiryStatus):
        json_status = status.value
    else:
        json_status = status
    params["status"] = json_status

    json_execution: int | None | Unset
    if isinstance(execution, Unset):
        json_execution = UNSET
    else:
        json_execution = execution
    params["execution"] = json_execution

    json_assigned_to: int | None | Unset
    if isinstance(assigned_to, Unset):
        json_assigned_to = UNSET
    else:
        json_assigned_to = assigned_to
    params["assigned_to"] = json_assigned_to

    json_offset: int | None | Unset
    if isinstance(offset, Unset):
        json_offset = UNSET
    else:
        json_offset = offset
    params["offset"] = json_offset

    json_limit: int | None | Unset
    if isinstance(limit, Unset):
        json_limit = UNSET
    else:
        json_limit = limit
    params["limit"] = json_limit

    params = {k: v for k, v in params.items() if v is not UNSET and v is not None}

    _kwargs: dict[str, Any] = {
        "method": "get",
        "url": "/api/v1/inquiries",
        "params": params,
    }

    return _kwargs


def _parse_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> Any | PaginatedResponseInquirySummary | None:
    if response.status_code == 200:
        response_200 = PaginatedResponseInquirySummary.from_dict(response.json())

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
) -> Response[Any | PaginatedResponseInquirySummary]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    *,
    client: AuthenticatedClient,
    status: InquiryStatus | None | Unset = UNSET,
    execution: int | None | Unset = UNSET,
    assigned_to: int | None | Unset = UNSET,
    offset: int | None | Unset = UNSET,
    limit: int | None | Unset = UNSET,
) -> Response[Any | PaginatedResponseInquirySummary]:
    """List all inquiries with pagination and optional filters

    Args:
        status (InquiryStatus | None | Unset):
        execution (int | None | Unset):
        assigned_to (int | None | Unset):
        offset (int | None | Unset):
        limit (int | None | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Any | PaginatedResponseInquirySummary]
    """

    kwargs = _get_kwargs(
        status=status,
        execution=execution,
        assigned_to=assigned_to,
        offset=offset,
        limit=limit,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    *,
    client: AuthenticatedClient,
    status: InquiryStatus | None | Unset = UNSET,
    execution: int | None | Unset = UNSET,
    assigned_to: int | None | Unset = UNSET,
    offset: int | None | Unset = UNSET,
    limit: int | None | Unset = UNSET,
) -> Any | PaginatedResponseInquirySummary | None:
    """List all inquiries with pagination and optional filters

    Args:
        status (InquiryStatus | None | Unset):
        execution (int | None | Unset):
        assigned_to (int | None | Unset):
        offset (int | None | Unset):
        limit (int | None | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Any | PaginatedResponseInquirySummary
    """

    return sync_detailed(
        client=client,
        status=status,
        execution=execution,
        assigned_to=assigned_to,
        offset=offset,
        limit=limit,
    ).parsed


async def asyncio_detailed(
    *,
    client: AuthenticatedClient,
    status: InquiryStatus | None | Unset = UNSET,
    execution: int | None | Unset = UNSET,
    assigned_to: int | None | Unset = UNSET,
    offset: int | None | Unset = UNSET,
    limit: int | None | Unset = UNSET,
) -> Response[Any | PaginatedResponseInquirySummary]:
    """List all inquiries with pagination and optional filters

    Args:
        status (InquiryStatus | None | Unset):
        execution (int | None | Unset):
        assigned_to (int | None | Unset):
        offset (int | None | Unset):
        limit (int | None | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Any | PaginatedResponseInquirySummary]
    """

    kwargs = _get_kwargs(
        status=status,
        execution=execution,
        assigned_to=assigned_to,
        offset=offset,
        limit=limit,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    *,
    client: AuthenticatedClient,
    status: InquiryStatus | None | Unset = UNSET,
    execution: int | None | Unset = UNSET,
    assigned_to: int | None | Unset = UNSET,
    offset: int | None | Unset = UNSET,
    limit: int | None | Unset = UNSET,
) -> Any | PaginatedResponseInquirySummary | None:
    """List all inquiries with pagination and optional filters

    Args:
        status (InquiryStatus | None | Unset):
        execution (int | None | Unset):
        assigned_to (int | None | Unset):
        offset (int | None | Unset):
        limit (int | None | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Any | PaginatedResponseInquirySummary
    """

    return (
        await asyncio_detailed(
            client=client,
            status=status,
            execution=execution,
            assigned_to=assigned_to,
            offset=offset,
            limit=limit,
        )
    ).parsed
