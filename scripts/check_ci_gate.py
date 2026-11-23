"""Validate terminal CI job results using only Python's standard library."""

import json
import os
import sys


# Keep this policy aligned with ci.yml job conditions. The regression suite
# evaluates those conditions independently across every path/input combination.
JOBS = {
    "rust-fmt": ("rust", "run_rust_fmt"),
    "rust-clippy": ("rust", "run_rust_clippy"),
    "rust-test": ("rust", "run_rust_test"),
    "e2e-smoke": ("smoke", "run_e2e_smoke"),
    "rust-audit": ("rust", "run_rust_audit"),
    "web-blocking": ("web", "run_web_blocking"),
    "web-image": ("web_image", "run_web_image"),
    "web-advisory": ("web", "run_web_advisory"),
    "security-blocking": (None, "run_security_blocking"),
    "security-advisory": (None, "run_security_advisory"),
    "release-checks": ("release", None),
}
ADVISORY = {"web-advisory", "security-advisory"}
PATH_FLAGS = {"rust", "cargo_deny_inputs", "web", "web_image", "release", "smoke"}


def evaluate(needs, event, inputs):
    """Return blocking errors and advisory warnings; malformed input fails closed."""
    if event not in {"push", "pull_request", "workflow_dispatch"}:
        raise ValueError(f"Unsupported CI event: {event}")
    if not isinstance(needs, dict) or set(needs) != set(JOBS) | {"changes"}:
        raise ValueError("CI needs must contain every known job and no unknown jobs")
    if not isinstance(inputs, dict):
        raise ValueError("CI inputs must be an object")
    if any(not isinstance(job, dict) for job in needs.values()):
        raise ValueError("Each CI job result must be an object")
    if needs["changes"].get("result") != "success":
        return ["changes must succeed before job applicability can be trusted"], []

    flags = needs["changes"].get("outputs")
    if not isinstance(flags, dict) or any(
        flags.get(key) not in {"true", "false"} for key in PATH_FLAGS
    ):
        raise ValueError("Path detection must supply all required true/false flags")
    if event == "workflow_dispatch":
        keys = {key for _, key in JOBS.values() if key is not None}
        if set(inputs) != keys or any(type(value) is not bool for value in inputs.values()):
            raise ValueError("Manual runs must supply every declared boolean input")

    errors, warnings = [], []
    for name, (path_flag, input_flag) in JOBS.items():
        if event == "workflow_dispatch":
            expected = input_flag is None or inputs[input_flag]
        else:
            expected = path_flag is None or flags[path_flag] == "true"
        result = needs[name].get("result")
        if result == "success" or (result == "skipped" and not expected):
            continue
        if result == "failure" and name in ADVISORY:
            warnings.append(f"{name} failed (advisory; inspect its logs)")
        else:
            errors.append(f"{name}: {result!r}; expected {'success' if expected else 'success or skipped'}")
    return errors, warnings


def main():
    try:
        errors, warnings = evaluate(
            json.loads(os.environ["CI_NEEDS"]),
            os.environ["CI_EVENT"],
            json.loads(os.environ["CI_INPUTS"]),
        )
    except (KeyError, ValueError, TypeError) as error:
        print(f"CI gate rejected invalid input: {error}", file=sys.stderr)
        return 1
    for warning in warnings:
        print(f"::warning::{warning}")
    for error in errors:
        print(f"::error::{error}")
    if errors:
        return 1
    print("CI gate passed: applicable blocking jobs succeeded; skips match the selection policy.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
