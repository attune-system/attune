# Core dependencies

The three core actions are static musl binaries built from `crates/core-actions`.
They require no interpreter or shared libraries. Rust/Cargo and cargo-zigbuild are
build-time dependencies only. The timer sensor remains separately built from
`crates/core-timer-sensor` and retains its existing launcher and runtime contract.

Python remains part of the bootstrap publisher image, not an action prerequisite.
Other runtime definitions in core describe optional capabilities; they do not make
those runtimes prerequisites for echo, noop, or sleep.
