from __future__ import annotations

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any, TypeVar, cast

from attrs import define as _attrs_define
from typing_extensions import Self

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.install_source_type_0 import InstallSourceType0
    from ..models.install_source_type_1 import InstallSourceType1
    from ..models.pack_contents import PackContents
    from ..models.pack_dependencies import PackDependencies
    from ..models.pack_meta import PackMeta


T = TypeVar("T", bound="PackIndexEntry")


@_attrs_define
class PackIndexEntry:
    """Pack entry in a registry index

    Attributes:
        author (str): Pack author/maintainer name
        contents (PackContents): Pack contents summary
        description (str): Brief pack description
        install_sources (list[InstallSourceType0 | InstallSourceType1]): Available installation sources
        keywords (list[str]): Searchable keywords/tags
        label (str): Human-readable pack name
        license_ (str): SPDX license identifier
        ref (str): Unique pack identifier (matches pack.yaml ref)
        runtime_deps (list[str]): Required runtimes (python3, nodejs, shell)
        version (str): Semantic version (latest available)
        dependencies (None | PackDependencies | Unset):
        email (None | str | Unset): Contact email
        homepage (None | str | Unset): Pack homepage URL
        meta (None | PackMeta | Unset):
        repository (None | str | Unset): Source repository URL
        use_case (None | str | Unset): Brief use-case summary for browsing/install decisions
    """

    author: str
    contents: PackContents
    description: str
    install_sources: list[InstallSourceType0 | InstallSourceType1]
    keywords: list[str]
    label: str
    license_: str
    ref: str
    runtime_deps: list[str]
    version: str
    dependencies: None | PackDependencies | Unset = UNSET
    email: None | str | Unset = UNSET
    homepage: None | str | Unset = UNSET
    meta: None | PackMeta | Unset = UNSET
    repository: None | str | Unset = UNSET
    use_case: None | str | Unset = UNSET

    def to_dict(self) -> dict[str, Any]:
        from ..models.install_source_type_0 import InstallSourceType0
        from ..models.pack_dependencies import PackDependencies
        from ..models.pack_meta import PackMeta

        author = self.author

        contents = self.contents.to_dict()

        description = self.description

        install_sources = []
        for install_sources_item_data in self.install_sources:
            install_sources_item: dict[str, Any]
            if isinstance(install_sources_item_data, InstallSourceType0):
                install_sources_item = install_sources_item_data.to_dict()
            else:
                install_sources_item = install_sources_item_data.to_dict()

            install_sources.append(install_sources_item)

        keywords = self.keywords

        label = self.label

        license_ = self.license_

        ref = self.ref

        runtime_deps = self.runtime_deps

        version = self.version

        dependencies: dict[str, Any] | None | Unset
        if isinstance(self.dependencies, Unset):
            dependencies = UNSET
        elif isinstance(self.dependencies, PackDependencies):
            dependencies = self.dependencies.to_dict()
        else:
            dependencies = self.dependencies

        email: None | str | Unset
        if isinstance(self.email, Unset):
            email = UNSET
        else:
            email = self.email

        homepage: None | str | Unset
        if isinstance(self.homepage, Unset):
            homepage = UNSET
        else:
            homepage = self.homepage

        meta: dict[str, Any] | None | Unset
        if isinstance(self.meta, Unset):
            meta = UNSET
        elif isinstance(self.meta, PackMeta):
            meta = self.meta.to_dict()
        else:
            meta = self.meta

        repository: None | str | Unset
        if isinstance(self.repository, Unset):
            repository = UNSET
        else:
            repository = self.repository

        use_case: None | str | Unset
        if isinstance(self.use_case, Unset):
            use_case = UNSET
        else:
            use_case = self.use_case

        field_dict: dict[str, Any] = {}

        field_dict.update(
            {
                "author": author,
                "contents": contents,
                "description": description,
                "install_sources": install_sources,
                "keywords": keywords,
                "label": label,
                "license": license_,
                "ref": ref,
                "runtime_deps": runtime_deps,
                "version": version,
            }
        )
        if dependencies is not UNSET:
            field_dict["dependencies"] = dependencies
        if email is not UNSET:
            field_dict["email"] = email
        if homepage is not UNSET:
            field_dict["homepage"] = homepage
        if meta is not UNSET:
            field_dict["meta"] = meta
        if repository is not UNSET:
            field_dict["repository"] = repository
        if use_case is not UNSET:
            field_dict["use_case"] = use_case

        return field_dict

    @classmethod
    def from_dict(cls, src_dict: Mapping[str, Any]) -> Self:
        from ..models.install_source_type_0 import InstallSourceType0
        from ..models.install_source_type_1 import InstallSourceType1
        from ..models.pack_contents import PackContents
        from ..models.pack_dependencies import PackDependencies
        from ..models.pack_meta import PackMeta

        d = dict(src_dict)
        author = d.pop("author")

        contents = PackContents.from_dict(d.pop("contents"))

        description = d.pop("description")

        install_sources = []
        _install_sources = d.pop("install_sources")
        for install_sources_item_data in _install_sources:

            def _parse_install_sources_item(
                data: object,
            ) -> InstallSourceType0 | InstallSourceType1:
                try:
                    if not isinstance(data, dict):
                        raise TypeError()
                    componentsschemas_install_source_type_0 = (
                        InstallSourceType0.from_dict(data)
                    )

                    return componentsschemas_install_source_type_0
                except (TypeError, ValueError, AttributeError, KeyError):
                    pass
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_install_source_type_1 = InstallSourceType1.from_dict(
                    data
                )

                return componentsschemas_install_source_type_1

            install_sources_item = _parse_install_sources_item(
                install_sources_item_data
            )

            install_sources.append(install_sources_item)

        keywords = cast(list[str], d.pop("keywords"))

        label = d.pop("label")

        license_ = d.pop("license")

        ref = d.pop("ref")

        runtime_deps = cast(list[str], d.pop("runtime_deps"))

        version = d.pop("version")

        def _parse_dependencies(data: object) -> None | PackDependencies | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                dependencies_type_1 = PackDependencies.from_dict(data)

                return dependencies_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(None | PackDependencies | Unset, data)

        dependencies = _parse_dependencies(d.pop("dependencies", UNSET))

        def _parse_email(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        email = _parse_email(d.pop("email", UNSET))

        def _parse_homepage(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        homepage = _parse_homepage(d.pop("homepage", UNSET))

        def _parse_meta(data: object) -> None | PackMeta | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                meta_type_1 = PackMeta.from_dict(data)

                return meta_type_1
            except (TypeError, ValueError, AttributeError, KeyError):
                pass
            return cast(None | PackMeta | Unset, data)

        meta = _parse_meta(d.pop("meta", UNSET))

        def _parse_repository(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        repository = _parse_repository(d.pop("repository", UNSET))

        def _parse_use_case(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        use_case = _parse_use_case(d.pop("use_case", UNSET))

        pack_index_entry = cls(
            author=author,
            contents=contents,
            description=description,
            install_sources=install_sources,
            keywords=keywords,
            label=label,
            license_=license_,
            ref=ref,
            runtime_deps=runtime_deps,
            version=version,
            dependencies=dependencies,
            email=email,
            homepage=homepage,
            meta=meta,
            repository=repository,
            use_case=use_case,
        )

        return pack_index_entry
