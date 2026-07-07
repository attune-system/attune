from http import HTTPStatus
from typing import Any, cast
from urllib.parse import quote

import httpx

from ...client import AuthenticatedClient, Client
from ...types import Response, UNSET
from ... import errors

from ...models.api_response_trace_report_response import ApiResponseTraceReportResponse
from typing import cast



def _get_kwargs(
    trace_tag: str,

) -> dict[str, Any]:
    

    

    

    _kwargs: dict[str, Any] = {
        "method": "get",
        "url": "/api/v1/traces/{trace_tag}".format(trace_tag=quote(str(trace_tag), safe=""),),
    }


    return _kwargs



def _parse_response(*, client: AuthenticatedClient | Client, response: httpx.Response) -> Any | ApiResponseTraceReportResponse | None:
    if response.status_code == 200:
        response_200 = ApiResponseTraceReportResponse.from_dict(response.json())



        return response_200

    if response.status_code == 400:
        response_400 = cast(Any, None)
        return response_400

    if response.status_code == 401:
        response_401 = cast(Any, None)
        return response_401

    if response.status_code == 403:
        response_403 = cast(Any, None)
        return response_403

    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(*, client: AuthenticatedClient | Client, response: httpx.Response) -> Response[Any | ApiResponseTraceReportResponse]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    trace_tag: str,
    *,
    client: AuthenticatedClient,

) -> Response[Any | ApiResponseTraceReportResponse]:
    """ 
    Args:
        trace_tag (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Any | ApiResponseTraceReportResponse]
     """


    kwargs = _get_kwargs(
        trace_tag=trace_tag,

    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)

def sync(
    trace_tag: str,
    *,
    client: AuthenticatedClient,

) -> Any | ApiResponseTraceReportResponse | None:
    """ 
    Args:
        trace_tag (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Any | ApiResponseTraceReportResponse
     """


    return sync_detailed(
        trace_tag=trace_tag,
client=client,

    ).parsed

async def asyncio_detailed(
    trace_tag: str,
    *,
    client: AuthenticatedClient,

) -> Response[Any | ApiResponseTraceReportResponse]:
    """ 
    Args:
        trace_tag (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Any | ApiResponseTraceReportResponse]
     """


    kwargs = _get_kwargs(
        trace_tag=trace_tag,

    )

    response = await client.get_async_httpx_client().request(
        **kwargs
    )

    return _build_response(client=client, response=response)

async def asyncio(
    trace_tag: str,
    *,
    client: AuthenticatedClient,

) -> Any | ApiResponseTraceReportResponse | None:
    """ 
    Args:
        trace_tag (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Any | ApiResponseTraceReportResponse
     """


    return (await asyncio_detailed(
        trace_tag=trace_tag,
client=client,

    )).parsed
