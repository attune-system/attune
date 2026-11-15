#!/usr/bin/env python3
"""Forward only bootstrap-owned structured fields, never arbitrary child logs."""
import re
import sys


def summarize(lines):
    count = 0
    for line in lines:
        line = line.rstrip("\n")
        if len(line) <= 2048 and re.fullmatch(
            r"ATTUNE_BOOTSTRAP_DIAGNOSTIC (?:[a-z_]+=[A-Za-z0-9_.-]+)(?: [a-z_]+=[A-Za-z0-9_.-]+)*", line
        ):
            # Restrict both keys and value alphabets; reject the entire line if
            # any unknown field appears rather than accidentally relaying it.
            fields = line.split()[1:]
            allowed = {"stage", "outcome", "exit", "error", "http", "api_stage", "code", "operation"}
            if all(field.split("=", 1)[0] in allowed for field in fields):
                print(line)
                count += 1
    if not count:
        print("No structured bootstrap diagnostic available; raw service logs withheld")


if __name__ == "__main__":
    summarize(sys.stdin)
