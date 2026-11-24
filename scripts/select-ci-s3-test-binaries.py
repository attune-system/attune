"""Export the two S3 correctness test executables from Cargo's build output."""

import json
import sys
from pathlib import Path


def main():
    targets = {
        ("attune_common", "lib"): "ATTUNE_S3_COMMON_TEST_BIN",
        ("runtime_log_replica_tests", "test"): "ATTUNE_S3_REPLICA_TEST_BIN",
    }
    binaries = {}
    for line in sys.stdin:
        message = json.loads(line)
        if message["reason"] != "compiler-artifact" or not message["profile"]["test"]:
            continue
        target = message["target"]
        variable = targets.get((target["name"], target["kind"][0]))
        executable = message["executable"]
        if variable and executable:
            if variable in binaries and binaries[variable] != executable:
                sys.exit(f"Multiple Cargo test executables found for {variable}")
            binaries[variable] = executable

    for variable in targets.values():
        executable = binaries.get(variable)
        if not executable or not Path(executable).is_file():
            sys.exit(f"Missing Cargo test executable for {variable}")
        print(f"{variable}={executable}")


if __name__ == "__main__":
    main()
