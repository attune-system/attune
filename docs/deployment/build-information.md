# Build information

`GET /api/v1/info` reports the build identity of the API process that handles the request. The endpoint is public, does not query the database, and sends `Cache-Control: no-store`.

```json
{
  "data": {
    "version": "0.7.3",
    "git_sha": "0123456789abcdef0123456789abcdef01234567"
  }
}
```

`version` is the workspace semantic version. `git_sha` is the full source commit SHA compiled into the binary. The value is `unknown` when neither Git metadata nor an explicit build SHA was available. A SHA identifies the source commit, not uncommitted modifications to a development checkout.

## Client commands

```bash
attune info
attune --profile production info
attune --api-url https://attune.example.com --json info
attune info --local

attune-mcp --profile production --info
attune-mcp --info --local
```

The default report separates `local` and `server`:

- `local` identifies the `attune` or `attune-mcp` binary that runs the command.
- `server` identifies the API reached through the selected profile or explicit API URL. It includes the profile name, API origin, connection status, version, and Git SHA.

Server lookup failures retain local information, report the error, and return a nonzero command exit status. Lookups have a ten-second deadline. `--local` avoids profile loading and network requests.

`attune info` supports table, JSON, and YAML output. `attune-mcp --info` emits JSON and exits instead of starting the MCP transport. The MCP `info_get` tool returns the same local and server distinction for the running MCP process. Its `server.status` is `unavailable` if the API lookup fails.

## Web UI

The **System info** navigation link opens `/info`. The page queries the server endpoint and displays its semantic version and full Git SHA. **Refresh** issues another request. During a rolling deployment, requests may reach different API replicas and report different builds.

## Build-time revision metadata

Native Cargo builds discover the commit from Git. CI sets `ATTUNE_BUILD_GIT_SHA` to the release commit before compilation. Git-less source builds can supply the same variable:

```bash
ATTUNE_BUILD_GIT_SHA="$SOURCE_COMMIT_SHA" cargo build --release --locked
```

Docker compiler stages accept `ATTUNE_BUILD_GIT_SHA` as a build argument. The Make targets supply the current commit. For direct Compose builds:

```bash
export ATTUNE_BUILD_GIT_SHA="$(git rev-parse HEAD)"
docker compose build
```

Changing an environment variable after compilation does not change the reported build. Packaging images retain the metadata already compiled into their binaries.

`bash scripts/test-build-info.sh` verifies Git-less fallback, invalid SHA rejection, warm-cache rebuilds with different SHAs, and resistance to runtime overrides.
