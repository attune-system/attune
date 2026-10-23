from http import HTTPStatus
from typing import Any, cast
from urllib.parse import quote

import httpx

from ... import errors
from ...client import AuthenticatedClient, Client
from ...models.api_response_sensor_response import ApiResponseSensorResponse
from ...types import UNSET, Response, Unset


def _get_kwargs(
    ref: str,
    *,
    include_secret_values: bool | Unset = UNSET,
) -> dict[str, Any]:

    params: dict[str, Any] = {}

    params["include_secret_values"] = include_secret_values

    params = {k: v for k, v in params.items() if v is not UNSET and v is not None}

    _kwargs: dict[str, Any] = {
        "method": "get",
        "url": "/api/v1/sensors/{ref}".format(
            ref=quote(str(ref), safe=""),
        ),
        "params": params,
    }

    return _kwargs


def _parse_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> Any | ApiResponseSensorResponse | None:
    if response.status_code == 200:
        response_200 = ApiResponseSensorResponse.from_dict(response.json())

        return response_200

    if response.status_code == 404:
        response_404 = cast(Any, None)
        return response_404

    if response.status_code == 500:
        response_500 = cast(Any, None)
        return response_500

    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> Response[Any | ApiResponseSensorResponse]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    ref: str,
    *,
    client: AuthenticatedClient | Client,
    include_secret_values: bool | Unset = UNSET,
) -> Response[Any | ApiResponseSensorResponse]:
    """Get a single sensor by reference

    Args:
        ref (str):
        include_secret_values (bool | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Any | ApiResponseSensorResponse]
    """

    kwargs = _get_kwargs(
        ref=ref,
        include_secret_values=include_secret_values,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    ref: str,
    *,
    client: AuthenticatedClient | Client,
    include_secret_values: bool | Unset = UNSET,
) -> Any | ApiResponseSensorResponse | None:
    """Get a single sensor by reference

    Args:
        ref (str):
        include_secret_values (bool | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Any | ApiResponseSensorResponse
    """

    return sync_detailed(
        ref=ref,
        client=client,
        include_secret_values=include_secret_values,
    ).parsed


async def asyncio_detailed(
    ref: str,
    *,
    client: AuthenticatedClient | Client,
    include_secret_values: bool | Unset = UNSET,
) -> Response[Any | ApiResponseSensorResponse]:
    """Get a single sensor by reference

    Args:
        ref (str):
        include_secret_values (bool | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Any | ApiResponseSensorResponse]
    """

    kwargs = _get_kwargs(
        ref=ref,
        include_secret_values=include_secret_values,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    ref: str,
    *,
    client: AuthenticatedClient | Client,
    include_secret_values: bool | Unset = UNSET,
) -> Any | ApiResponseSensorResponse | None:
    """Get a single sensor by reference

    Args:
        ref (str):
        include_secret_values (bool | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Any | ApiResponseSensorResponse
    """

    return (
        await asyncio_detailed(
            ref=ref,
            client=client,
            include_secret_values=include_secret_values,
        )
    ).parsed
