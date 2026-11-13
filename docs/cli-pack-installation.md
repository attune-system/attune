# Pack administration

Core 1.0.7 no longer ships pack-administration actions or the installation
workflow. Install and administer packs through the normal authenticated CLI/API.

```sh
attune pack upload ./packs/my_pack
attune pack install https://example.invalid/my_pack.tar.gz
attune pack list
```

Use `pack upload` for local files when the API runs in Docker. `pack register`
requires an API-visible server path. Installation still performs ordinary
admission, runtime verification, and candidate testing; core actions provide no
administrative shortcut. An optional utility pack is deferred.

See [CLI reference](cli/cli.md), [API pack installation](api/api-pack-installation.md),
and [core contract](../packs/core/README.md).
