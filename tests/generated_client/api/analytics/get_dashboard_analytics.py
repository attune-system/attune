import datetime
from http import HTTPStatus
from typing import Any

import httpx

from ... import errors
from ...client import AuthenticatedClient, Client
from ...models.get_dashboard_analytics_response_200 import (
    GetDashboardAnalyticsResponse200,
)
from ...types import UNSET, Response, Unset


def _get_kwargs(
    *,
    since: datetime.datetime | None | Unset = UNSET,
    until: datetime.datetime | None | Unset = UNSET,
    hours: int | None | Unset = UNSET,
) -> dict[str, Any]:

    params: dict[str, Any] = {}

    json_since: None | str | Unset
    if isinstance(since, Unset):
        json_since = UNSET
    elif isinstance(since, datetime.datetime):
        json_since = since.isoformat()
    else:
        json_since = since
    params["since"] = json_since

    json_until: None | str | Unset
    if isinstance(until, Unset):
        json_until = UNSET
    elif isinstance(until, datetime.datetime):
        json_until = until.isoformat()
    else:
        json_until = until
    params["until"] = json_until

    json_hours: int | None | Unset
    if isinstance(hours, Unset):
        json_hours = UNSET
    else:
        json_hours = hours
    params["hours"] = json_hours

    params = {k: v for k, v in params.items() if v is not UNSET and v is not None}

    _kwargs: dict[str, Any] = {
        "method": "get",
        "url": "/api/v1/analytics/dashboard",
        "params": params,
    }

    return _kwargs


def _parse_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> GetDashboardAnalyticsResponse200 | None:
    if response.status_code == 200:
        response_200 = GetDashboardAnalyticsResponse200.from_dict(response.json())

        return response_200

    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> Response[GetDashboardAnalyticsResponse200]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    *,
    client: AuthenticatedClient,
    since: datetime.datetime | None | Unset = UNSET,
    until: datetime.datetime | None | Unset = UNSET,
    hours: int | None | Unset = UNSET,
) -> Response[GetDashboardAnalyticsResponse200]:
    """Get a combined dashboard analytics payload.

     Returns all key metrics in a single response to avoid multiple round-trips
    from the dashboard page. Includes execution throughput, status transitions,
    event volume, enforcement volume, worker status, and failure rate.

    Args:
        since (datetime.datetime | None | Unset):
        until (datetime.datetime | None | Unset):
        hours (int | None | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[GetDashboardAnalyticsResponse200]
    """

    kwargs = _get_kwargs(
        since=since,
        until=until,
        hours=hours,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    *,
    client: AuthenticatedClient,
    since: datetime.datetime | None | Unset = UNSET,
    until: datetime.datetime | None | Unset = UNSET,
    hours: int | None | Unset = UNSET,
) -> GetDashboardAnalyticsResponse200 | None:
    """Get a combined dashboard analytics payload.

     Returns all key metrics in a single response to avoid multiple round-trips
    from the dashboard page. Includes execution throughput, status transitions,
    event volume, enforcement volume, worker status, and failure rate.

    Args:
        since (datetime.datetime | None | Unset):
        until (datetime.datetime | None | Unset):
        hours (int | None | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        GetDashboardAnalyticsResponse200
    """

    return sync_detailed(
        client=client,
        since=since,
        until=until,
        hours=hours,
    ).parsed


async def asyncio_detailed(
    *,
    client: AuthenticatedClient,
    since: datetime.datetime | None | Unset = UNSET,
    until: datetime.datetime | None | Unset = UNSET,
    hours: int | None | Unset = UNSET,
) -> Response[GetDashboardAnalyticsResponse200]:
    """Get a combined dashboard analytics payload.

     Returns all key metrics in a single response to avoid multiple round-trips
    from the dashboard page. Includes execution throughput, status transitions,
    event volume, enforcement volume, worker status, and failure rate.

    Args:
        since (datetime.datetime | None | Unset):
        until (datetime.datetime | None | Unset):
        hours (int | None | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[GetDashboardAnalyticsResponse200]
    """

    kwargs = _get_kwargs(
        since=since,
        until=until,
        hours=hours,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    *,
    client: AuthenticatedClient,
    since: datetime.datetime | None | Unset = UNSET,
    until: datetime.datetime | None | Unset = UNSET,
    hours: int | None | Unset = UNSET,
) -> GetDashboardAnalyticsResponse200 | None:
    """Get a combined dashboard analytics payload.

     Returns all key metrics in a single response to avoid multiple round-trips
    from the dashboard page. Includes execution throughput, status transitions,
    event volume, enforcement volume, worker status, and failure rate.

    Args:
        since (datetime.datetime | None | Unset):
        until (datetime.datetime | None | Unset):
        hours (int | None | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        GetDashboardAnalyticsResponse200
    """

    return (
        await asyncio_detailed(
            client=client,
            since=since,
            until=until,
            hours=hours,
        )
    ).parsed
