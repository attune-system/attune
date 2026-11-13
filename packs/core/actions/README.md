# Native core actions

Only `echo.yaml`, `sleep.yaml`, and `noop.yaml` declare core actions. Their native
binaries receive one flat JSON object on stdin and produce text output. See the
[core README](../README.md) for parameter behavior and architecture packaging.

No shell/Python wrappers, HTTP clients, arbitrary command launchers, SSH key
helpers, pack-administration actions, or installation workflows are shipped here.
