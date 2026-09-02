from http import HTTPStatus
from typing import Any, cast
from urllib.parse import quote

import httpx

from ... import errors
from ...client import AuthenticatedClient, Client
from ...models.api_response_trigger_response import ApiResponseTriggerResponse
from ...types import UNSET, Response, Unset


def _get_kwargs(
    ref: str,
    *,
    referencing_pack_ref: None | str | Unset = UNSET,
) -> dict[str, Any]:

    params: dict[str, Any] = {}

    json_referencing_pack_ref: None | str | Unset
    if isinstance(referencing_pack_ref, Unset):
        json_referencing_pack_ref = UNSET
    else:
        json_referencing_pack_ref = referencing_pack_ref
    params["referencing_pack_ref"] = json_referencing_pack_ref

    params = {k: v for k, v in params.items() if v is not UNSET and v is not None}

    _kwargs: dict[str, Any] = {
        "method": "get",
        "url": "/api/v1/triggers/{ref}".format(
            ref=quote(str(ref), safe=""),
        ),
        "params": params,
    }

    return _kwargs


def _parse_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> Any | ApiResponseTriggerResponse | None:
    if response.status_code == 200:
        response_200 = ApiResponseTriggerResponse.from_dict(response.json())

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
) -> Response[Any | ApiResponseTriggerResponse]:
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
    referencing_pack_ref: None | str | Unset = UNSET,
) -> Response[Any | ApiResponseTriggerResponse]:
    """Get a single trigger by reference

    Args:
        ref (str):
        referencing_pack_ref (None | str | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Any | ApiResponseTriggerResponse]
    """

    kwargs = _get_kwargs(
        ref=ref,
        referencing_pack_ref=referencing_pack_ref,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    ref: str,
    *,
    client: AuthenticatedClient | Client,
    referencing_pack_ref: None | str | Unset = UNSET,
) -> Any | ApiResponseTriggerResponse | None:
    """Get a single trigger by reference

    Args:
        ref (str):
        referencing_pack_ref (None | str | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Any | ApiResponseTriggerResponse
    """

    return sync_detailed(
        ref=ref,
        client=client,
        referencing_pack_ref=referencing_pack_ref,
    ).parsed


async def asyncio_detailed(
    ref: str,
    *,
    client: AuthenticatedClient | Client,
    referencing_pack_ref: None | str | Unset = UNSET,
) -> Response[Any | ApiResponseTriggerResponse]:
    """Get a single trigger by reference

    Args:
        ref (str):
        referencing_pack_ref (None | str | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Any | ApiResponseTriggerResponse]
    """

    kwargs = _get_kwargs(
        ref=ref,
        referencing_pack_ref=referencing_pack_ref,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    ref: str,
    *,
    client: AuthenticatedClient | Client,
    referencing_pack_ref: None | str | Unset = UNSET,
) -> Any | ApiResponseTriggerResponse | None:
    """Get a single trigger by reference

    Args:
        ref (str):
        referencing_pack_ref (None | str | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Any | ApiResponseTriggerResponse
    """

    return (
        await asyncio_detailed(
            ref=ref,
            client=client,
            referencing_pack_ref=referencing_pack_ref,
        )
    ).parsed
