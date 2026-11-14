# Native core actions

`echo.yaml`, `sleep.yaml`, `noop.yaml` and `http_request.yaml` declare the four
native actions. Each binary entrypoint implements its own behavior and receives
flat stdin JSON; the shared Rust library only parses/validates parameters.
Echo/sleep/noop keep text output; HTTP returns structured JSON with native Rust TLS.
See the [core README](../README.md) for HTTP semantics and architecture packaging.

No shell/Python/curl wrappers, arbitrary command launchers, SSH key helpers,
pack-administration actions, or installation workflow are shipped here.
