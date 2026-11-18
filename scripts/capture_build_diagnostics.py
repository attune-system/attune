#!/usr/bin/env python3
"""Run one initial build, drain plain output, retain only allowlisted evidence.

Raw BuildKit output, source excerpts, URLs, environment and command arguments
are neither echoed nor written to disk. This wrapper never retries a build.
"""
import argparse
import json
import os
from pathlib import PurePosixPath
import re
import signal
import stat
import subprocess
import sys
import threading

LINE_LIMIT = 16384
INPUT_LIMIT = 16 * 1024 * 1024
EVENT_LIMIT = 4096
ERRORS = {"E0432": "unresolved_import", "E0433": "unresolved_module", "E0583": "missing_module_file",
          "E0425": "missing_symbol", "E0412": "missing_type", "E0308": "type_mismatch",
          "E0599": "missing_method", "E0277": "unsatisfied_trait", "E0063": "missing_fields"}


def events(line):
    text = re.sub(r"\x1b\[[0-9;]*m", "", line)
    result = []
    code = re.search(r"\berror\[(E[0-9]{4})\]:", text)
    if code:
        result.append({"kind": "rust_error", "code": code[1], "category": ERRORS.get(code[1], "compiler_error")})
    elif "error:" in text:
        category = "compiler_or_build_error"
        for phrase, known in [("could not compile", "cargo_compile_failed"), ("couldn't read", "missing_source_file"),
                              ("lock file", "lockfile_error"), ("failed to solve", "buildkit_failed")]:
            if phrase in text:
                category = known
                break
        result.append({"kind": "build_error", "category": category})
    for phrase, known in [("missing a lib target", "missing_library_target"),
                          ("signal: 9", "compiler_signal_9"), ("No space left on device", "disk_full"),
                          ("failed to receive status", "buildkit_status_lost")]:
        if phrase in text:
            result.append({"kind": "build_condition", "category": known})
    location = re.search(r"(?:-->|couldn't read|failed to read)\s+[`\"]?(?:/build/)?((?:crates|packs|migrations|docker)/[A-Za-z0-9_./-]+)(?::([0-9]{1,9})(?::([0-9]{1,9}))?)?", text)
    if location and len(location[1]) <= 512 and ".." not in PurePosixPath(location[1]).parts:
        value = {"kind": "source_location", "file": location[1]}
        if location[2]:
            value["line"] = int(location[2])
        if location[3]:
            value["column"] = int(location[3])
        result.append(value)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", required=True)
    parser.add_argument("--group", required=True)
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    command = args.command[1:] if args.command[:1] == ["--"] else args.command
    if not command or not re.fullmatch(r"[a-z0-9][a-z0-9_-]{0,63}", args.group):
        parser.error("one command and a bounded public build group are required")
    descriptor = os.open(args.output, os.O_WRONLY | os.O_APPEND | os.O_CREAT | os.O_NOFOLLOW, 0o600)
    metadata = os.fstat(descriptor)
    if (not stat.S_ISREG(metadata.st_mode) or metadata.st_uid != os.geteuid()
            or metadata.st_nlink != 1 or stat.S_IMODE(metadata.st_mode) & 0o077 or metadata.st_size):
        os.close(descriptor)
        raise SystemExit("invalid owned diagnostic output file")
    with os.fdopen(descriptor, "w", encoding="utf-8") as output:
        count = 0
        def emit(value):
            nonlocal count
            if count >= EVENT_LIMIT:
                return
            line = json.dumps({"group": args.group, **value}, sort_keys=True)
            output.write(line + "\n")
            output.flush()
            print(line, flush=True)
            count += 1
        child = subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, start_new_session=True)
        kill_timer = None
        interrupted = None
        def terminate(number, _frame):
            nonlocal kill_timer, interrupted
            interrupted = number
            try:
                os.killpg(child.pid, number)
            except ProcessLookupError:
                return
            def force_join():
                try:
                    os.killpg(child.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
            if kill_timer is None:
                kill_timer = threading.Timer(10, force_join)
                kill_timer.start()
        for number in [signal.SIGINT, signal.SIGTERM]:
            signal.signal(number, terminate)
        emit({"kind": "build_start"})
        total = 0
        buffer = bytearray()
        dropping = False
        truncated = False
        try:
            while True:
                chunk = child.stdout.read1(65536)
                if not chunk:
                    break
                total += len(chunk)
                if total > INPUT_LIMIT:
                    truncated = True
                    continue  # Keep draining; no hidden cancellation/retry.
                for piece in chunk.splitlines(keepends=True):
                    end = piece.endswith((b"\n", b"\r"))
                    if not dropping:
                        if len(buffer) + len(piece) > LINE_LIMIT:
                            dropping = True
                            truncated = True
                        else:
                            buffer.extend(piece)
                    if end:
                        if not dropping:
                            for value in events(buffer.decode("utf-8", errors="replace")):
                                emit(value)
                        buffer.clear()
                        dropping = False
            if buffer and not dropping:
                for value in events(buffer.decode("utf-8", errors="replace")):
                    emit(value)
            status = child.wait()
        finally:
            if child.poll() is None:
                terminate(signal.SIGTERM, None)
                child.wait()
            if kill_timer is not None:
                kill_timer.cancel()
                kill_timer.join()
        # Always reserve a terminal record, including when event capacity fills.
        if count >= EVENT_LIMIT:
            count = EVENT_LIMIT - 1
            truncated = True
        code = 128 + interrupted if interrupted is not None else (128 - status if status < 0 else status)
        emit({"kind": "build_end", "exit_code": code, "raw_bytes_examined": total,
              "diagnostics_truncated": truncated})
        output.flush()
        os.fsync(output.fileno())
    return min(code, 255)


if __name__ == "__main__":
    sys.exit(main())
