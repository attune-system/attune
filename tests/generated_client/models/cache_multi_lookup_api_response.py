from __future__ import annotations

from collections.abc import Mapping
from typing import Any, TypeVar, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..types import UNSET, Unset
from typing import cast

if TYPE_CHECKING:
  from ..models.cache_multi_lookup_response import CacheMultiLookupResponse





T = TypeVar("T", bound="CacheMultiLookupApiResponse")



@_attrs_define
class CacheMultiLookupApiResponse:
    """ 
        Attributes:
            data (CacheMultiLookupResponse): Bounded multi-ID lookup response. Missing IDs are reported explicitly.
            message (None | str | Unset):
     """

    data: CacheMultiLookupResponse
    message: None | str | Unset = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)





    def to_dict(self) -> dict[str, Any]:
        from ..models.cache_multi_lookup_response import CacheMultiLookupResponse
        data = self.data.to_dict()

        message: None | str | Unset
        if isinstance(self.message, Unset):
            message = UNSET
        else:
            message = self.message


        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update({
            "data": data,
        })
        if message is not UNSET:
            field_dict["message"] = message

        return field_dict



    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.cache_multi_lookup_response import CacheMultiLookupResponse
        d = dict(src_dict)
        data = CacheMultiLookupResponse.from_dict(d.pop("data"))




        def _parse_message(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        message = _parse_message(d.pop("message", UNSET))


        cache_multi_lookup_api_response = cls(
            data=data,
            message=message,
        )


        cache_multi_lookup_api_response.additional_properties = d
        return cache_multi_lookup_api_response

    @property
    def additional_keys(self) -> list[str]:
        return list(self.additional_properties.keys())

    def __getitem__(self, key: str) -> Any:
        return self.additional_properties[key]

    def __setitem__(self, key: str, value: Any) -> None:
        self.additional_properties[key] = value

    def __delitem__(self, key: str) -> None:
        del self.additional_properties[key]

    def __contains__(self, key: str) -> bool:
        return key in self.additional_properties
