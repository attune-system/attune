from http import HTTPStatus
from typing import Any, cast

import httpx

from ... import errors
from ...client import AuthenticatedClient, Client
from ...models.pack_upload_form import PackUploadForm
from ...models.upload_pack_response_201 import UploadPackResponse201
from ...types import Response


def _get_kwargs(
    *,
    body: PackUploadForm,
) -> dict[str, Any]:
    headers: dict[str, Any] = {}

    _kwargs: dict[str, Any] = {
        "method": "post",
        "url": "/api/v1/packs/upload",
    }

    _kwargs["files"] = body.to_multipart()

    headers["Content-Type"] = "multipart/form-data; boundary=+++"

    _kwargs["headers"] = headers
    return _kwargs


def _parse_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> Any | UploadPackResponse201 | None:
    if response.status_code == 201:
        response_201 = UploadPackResponse201.from_dict(response.json())

        return response_201

    if response.status_code == 400:
        response_400 = cast(Any, None)
        return response_400

    if response.status_code == 409:
        response_409 = cast(Any, None)
        return response_409

    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> Response[Any | UploadPackResponse201]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    *,
    client: AuthenticatedClient,
    body: PackUploadForm,
) -> Response[Any | UploadPackResponse201]:
    """Upload and register a pack from a tar.gz archive (multipart/form-data)

     The archive should be a gzipped tar containing the pack directory at its root
    (i.e. the archive should unpack to files like `pack.yaml`, `actions/`, etc.).
    The multipart field name must be `pack`.

    Optional form fields:
    - `force`: `"true"` to overwrite an existing pack with the same ref
    - `skip_tests`: `"true"` to skip test execution after registration

    Args:
        body (PackUploadForm):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Any | UploadPackResponse201]
    """

    kwargs = _get_kwargs(
        body=body,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    *,
    client: AuthenticatedClient,
    body: PackUploadForm,
) -> Any | UploadPackResponse201 | None:
    """Upload and register a pack from a tar.gz archive (multipart/form-data)

     The archive should be a gzipped tar containing the pack directory at its root
    (i.e. the archive should unpack to files like `pack.yaml`, `actions/`, etc.).
    The multipart field name must be `pack`.

    Optional form fields:
    - `force`: `"true"` to overwrite an existing pack with the same ref
    - `skip_tests`: `"true"` to skip test execution after registration

    Args:
        body (PackUploadForm):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Any | UploadPackResponse201
    """

    return sync_detailed(
        client=client,
        body=body,
    ).parsed


async def asyncio_detailed(
    *,
    client: AuthenticatedClient,
    body: PackUploadForm,
) -> Response[Any | UploadPackResponse201]:
    """Upload and register a pack from a tar.gz archive (multipart/form-data)

     The archive should be a gzipped tar containing the pack directory at its root
    (i.e. the archive should unpack to files like `pack.yaml`, `actions/`, etc.).
    The multipart field name must be `pack`.

    Optional form fields:
    - `force`: `"true"` to overwrite an existing pack with the same ref
    - `skip_tests`: `"true"` to skip test execution after registration

    Args:
        body (PackUploadForm):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Any | UploadPackResponse201]
    """

    kwargs = _get_kwargs(
        body=body,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    *,
    client: AuthenticatedClient,
    body: PackUploadForm,
) -> Any | UploadPackResponse201 | None:
    """Upload and register a pack from a tar.gz archive (multipart/form-data)

     The archive should be a gzipped tar containing the pack directory at its root
    (i.e. the archive should unpack to files like `pack.yaml`, `actions/`, etc.).
    The multipart field name must be `pack`.

    Optional form fields:
    - `force`: `"true"` to overwrite an existing pack with the same ref
    - `skip_tests`: `"true"` to skip test execution after registration

    Args:
        body (PackUploadForm):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Any | UploadPackResponse201
    """

    return (
        await asyncio_detailed(
            client=client,
            body=body,
        )
    ).parsed
