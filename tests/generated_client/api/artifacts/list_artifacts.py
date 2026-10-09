from http import HTTPStatus
from typing import Any

import httpx

from ... import errors
from ...client import AuthenticatedClient, Client
from ...models.artifact_classification import ArtifactClassification
from ...models.artifact_type import ArtifactType
from ...models.artifact_visibility import ArtifactVisibility
from ...models.owner_type import OwnerType
from ...models.paginated_response_artifact_summary import (
    PaginatedResponseArtifactSummary,
)
from ...types import UNSET, Response, Unset


def _get_kwargs(
    *,
    scope: None | OwnerType | Unset = UNSET,
    owner: None | str | Unset = UNSET,
    type_: ArtifactType | None | Unset = UNSET,
    visibility: ArtifactVisibility | None | Unset = UNSET,
    classification: ArtifactClassification | None | Unset = UNSET,
    execution: int | None | Unset = UNSET,
    name: None | str | Unset = UNSET,
    page: int | Unset = UNSET,
    per_page: int | Unset = UNSET,
) -> dict[str, Any]:

    params: dict[str, Any] = {}

    json_scope: None | str | Unset
    if isinstance(scope, Unset):
        json_scope = UNSET
    elif isinstance(scope, OwnerType):
        json_scope = scope.value
    else:
        json_scope = scope
    params["scope"] = json_scope

    json_owner: None | str | Unset
    if isinstance(owner, Unset):
        json_owner = UNSET
    else:
        json_owner = owner
    params["owner"] = json_owner

    json_type_: None | str | Unset
    if isinstance(type_, Unset):
        json_type_ = UNSET
    elif isinstance(type_, ArtifactType):
        json_type_ = type_.value
    else:
        json_type_ = type_
    params["type"] = json_type_

    json_visibility: None | str | Unset
    if isinstance(visibility, Unset):
        json_visibility = UNSET
    elif isinstance(visibility, ArtifactVisibility):
        json_visibility = visibility.value
    else:
        json_visibility = visibility
    params["visibility"] = json_visibility

    json_classification: None | str | Unset
    if isinstance(classification, Unset):
        json_classification = UNSET
    elif isinstance(classification, ArtifactClassification):
        json_classification = classification.value
    else:
        json_classification = classification
    params["classification"] = json_classification

    json_execution: int | None | Unset
    if isinstance(execution, Unset):
        json_execution = UNSET
    else:
        json_execution = execution
    params["execution"] = json_execution

    json_name: None | str | Unset
    if isinstance(name, Unset):
        json_name = UNSET
    else:
        json_name = name
    params["name"] = json_name

    params["page"] = page

    params["per_page"] = per_page

    params = {k: v for k, v in params.items() if v is not UNSET and v is not None}

    _kwargs: dict[str, Any] = {
        "method": "get",
        "url": "/api/v1/artifacts",
        "params": params,
    }

    return _kwargs


def _parse_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> PaginatedResponseArtifactSummary | None:
    if response.status_code == 200:
        response_200 = PaginatedResponseArtifactSummary.from_dict(response.json())

        return response_200

    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: AuthenticatedClient | Client, response: httpx.Response
) -> Response[PaginatedResponseArtifactSummary]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    *,
    client: AuthenticatedClient,
    scope: None | OwnerType | Unset = UNSET,
    owner: None | str | Unset = UNSET,
    type_: ArtifactType | None | Unset = UNSET,
    visibility: ArtifactVisibility | None | Unset = UNSET,
    classification: ArtifactClassification | None | Unset = UNSET,
    execution: int | None | Unset = UNSET,
    name: None | str | Unset = UNSET,
    page: int | Unset = UNSET,
    per_page: int | Unset = UNSET,
) -> Response[PaginatedResponseArtifactSummary]:
    """List artifacts with pagination and optional filters

    Args:
        scope (None | OwnerType | Unset):
        owner (None | str | Unset):
        type_ (ArtifactType | None | Unset):
        visibility (ArtifactVisibility | None | Unset):
        classification (ArtifactClassification | None | Unset):
        execution (int | None | Unset):
        name (None | str | Unset):
        page (int | Unset):
        per_page (int | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[PaginatedResponseArtifactSummary]
    """

    kwargs = _get_kwargs(
        scope=scope,
        owner=owner,
        type_=type_,
        visibility=visibility,
        classification=classification,
        execution=execution,
        name=name,
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
    scope: None | OwnerType | Unset = UNSET,
    owner: None | str | Unset = UNSET,
    type_: ArtifactType | None | Unset = UNSET,
    visibility: ArtifactVisibility | None | Unset = UNSET,
    classification: ArtifactClassification | None | Unset = UNSET,
    execution: int | None | Unset = UNSET,
    name: None | str | Unset = UNSET,
    page: int | Unset = UNSET,
    per_page: int | Unset = UNSET,
) -> PaginatedResponseArtifactSummary | None:
    """List artifacts with pagination and optional filters

    Args:
        scope (None | OwnerType | Unset):
        owner (None | str | Unset):
        type_ (ArtifactType | None | Unset):
        visibility (ArtifactVisibility | None | Unset):
        classification (ArtifactClassification | None | Unset):
        execution (int | None | Unset):
        name (None | str | Unset):
        page (int | Unset):
        per_page (int | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        PaginatedResponseArtifactSummary
    """

    return sync_detailed(
        client=client,
        scope=scope,
        owner=owner,
        type_=type_,
        visibility=visibility,
        classification=classification,
        execution=execution,
        name=name,
        page=page,
        per_page=per_page,
    ).parsed


async def asyncio_detailed(
    *,
    client: AuthenticatedClient,
    scope: None | OwnerType | Unset = UNSET,
    owner: None | str | Unset = UNSET,
    type_: ArtifactType | None | Unset = UNSET,
    visibility: ArtifactVisibility | None | Unset = UNSET,
    classification: ArtifactClassification | None | Unset = UNSET,
    execution: int | None | Unset = UNSET,
    name: None | str | Unset = UNSET,
    page: int | Unset = UNSET,
    per_page: int | Unset = UNSET,
) -> Response[PaginatedResponseArtifactSummary]:
    """List artifacts with pagination and optional filters

    Args:
        scope (None | OwnerType | Unset):
        owner (None | str | Unset):
        type_ (ArtifactType | None | Unset):
        visibility (ArtifactVisibility | None | Unset):
        classification (ArtifactClassification | None | Unset):
        execution (int | None | Unset):
        name (None | str | Unset):
        page (int | Unset):
        per_page (int | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[PaginatedResponseArtifactSummary]
    """

    kwargs = _get_kwargs(
        scope=scope,
        owner=owner,
        type_=type_,
        visibility=visibility,
        classification=classification,
        execution=execution,
        name=name,
        page=page,
        per_page=per_page,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    *,
    client: AuthenticatedClient,
    scope: None | OwnerType | Unset = UNSET,
    owner: None | str | Unset = UNSET,
    type_: ArtifactType | None | Unset = UNSET,
    visibility: ArtifactVisibility | None | Unset = UNSET,
    classification: ArtifactClassification | None | Unset = UNSET,
    execution: int | None | Unset = UNSET,
    name: None | str | Unset = UNSET,
    page: int | Unset = UNSET,
    per_page: int | Unset = UNSET,
) -> PaginatedResponseArtifactSummary | None:
    """List artifacts with pagination and optional filters

    Args:
        scope (None | OwnerType | Unset):
        owner (None | str | Unset):
        type_ (ArtifactType | None | Unset):
        visibility (ArtifactVisibility | None | Unset):
        classification (ArtifactClassification | None | Unset):
        execution (int | None | Unset):
        name (None | str | Unset):
        page (int | Unset):
        per_page (int | Unset):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        PaginatedResponseArtifactSummary
    """

    return (
        await asyncio_detailed(
            client=client,
            scope=scope,
            owner=owner,
            type_=type_,
            visibility=visibility,
            classification=classification,
            execution=execution,
            name=name,
            page=page,
            per_page=per_page,
        )
    ).parsed
