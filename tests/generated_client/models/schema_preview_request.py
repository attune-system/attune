from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar

from attrs import define as _attrs_define
from typing_extensions import Self

from ..models.schema_preview_context import SchemaPreviewContext

T = TypeVar("T", bound="SchemaPreviewRequest")


@_attrs_define
class SchemaPreviewRequest:
    """
    Attributes:
        context (SchemaPreviewContext):
        schema (Any):
    """

    context: SchemaPreviewContext
    schema: Any

    def to_dict(self) -> dict[str, Any]:
        context = self.context.value

        schema = self.schema

        field_dict: dict[str, Any] = {}

        field_dict.update(
            {
                "context": context,
                "schema": schema,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        d = dict(src_dict)
        context = SchemaPreviewContext(d.pop("context"))

        schema = d.pop("schema")

        schema_preview_request = cls(
            context=context,
            schema=schema,
        )

        return schema_preview_request
