"""Record workspace Cargo test executables and run the opt-in broker cases."""

import argparse
import json
import os
import re
import subprocess
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parent.parent
TESTS = {
    ("common", "lib", "attune_common"): (
        "mq::consumer::tests::keyed_consumer_orders_related_deliveries_and_recovers_unsettled_work",
    ),
    ("executor", "lib", "attune_executor"): (
        "service::tests::test_service_drains_completion_before_closing_resources",
        "service::tests::test_service_can_stop_while_reconnecting_during_startup",
        "service::tests::test_manager_completion_retry_holds_key_in_real_listener",
        "service::tests::test_worker_completion_retry_holds_key_in_real_listener",
    ),
    ("api", "test", "manager_session_tests"): (
        "work::completion_survives_broker_return_and_reuses_one_event_id",
    ),
}
CARGO_MESSAGES = {"build-finished", "build-script-executed", "compiler-artifact", "compiler-message"}


def record_artifacts(artifact_path, output):
    with artifact_path.open("w") as artifacts:
        for line in output:
            try:
                message = json.loads(line) if line.startswith("{") else None
            except json.JSONDecodeError:
                message = None
            if not isinstance(message, dict) or message.get("reason") not in CARGO_MESSAGES:
                sys.stdout.write(line)
            elif (message["reason"] == "compiler-artifact"
                  and message.get("executable") and message["profile"]["test"]):
                artifacts.write(line)


def find_executable(artifacts, package_path, kind, target):
    paths = {
        entry["executable"] for entry in artifacts
        if Path(entry["manifest_path"]) == package_path / "Cargo.toml"
        and entry["target"]["kind"] == [kind] and entry["target"]["name"] == target
    }
    if len(paths) != 1:
        raise SystemExit(f"Expected one workspace test executable for {target} ({kind}), found {len(paths)}")
    return paths.pop()


def run_tests(artifact_path):
    for variable in ("ATTUNE_TEST_RABBITMQ_URL", "ATTUNE_MANAGER_TEST_AMQP_URL"):
        if not os.environ.get(variable):
            raise SystemExit(f"Set {variable} to the owned RabbitMQ vhost URL")

    artifacts = [json.loads(line) for line in artifact_path.read_text().splitlines()]
    for (package, kind, target), tests in TESTS.items():
        package_path = ROOT / "crates" / package
        executable = find_executable(artifacts, package_path, kind, target)
        for test in tests:
            print(f"Running {test} from {executable}", flush=True)
            result = subprocess.run(
                [executable, test, "--ignored", "--exact", "--nocapture", "--test-threads=4"],
                cwd=package_path, stdout=subprocess.PIPE, text=True,
            )
            sys.stdout.write(result.stdout)
            if result.returncode or not re.search(r"^test result: ok\. 1 passed;", result.stdout, re.MULTILINE):
                raise SystemExit(f"Expected exactly one passing broker test for {test}")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=("record", "run"))
    parser.add_argument("artifact_path", type=Path)
    args = parser.parse_args()
    if args.command == "record":
        record_artifacts(args.artifact_path, sys.stdin)
    else:
        run_tests(args.artifact_path)
