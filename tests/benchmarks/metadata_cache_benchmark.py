#!/usr/bin/env python3
"""Metadata-cache benchmark harness for Attune."""

from __future__ import annotations

import argparse
import concurrent.futures
import json
import math
import os
import platform
import random
import socket
import statistics
import subprocess
import sys
import threading
import time
import uuid
from collections import Counter
from dataclasses import dataclass, asdict
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Callable, Iterable, Sequence
from urllib.parse import quote

import requests

TESTS_DIR = Path(__file__).resolve().parents[1]
if str(TESTS_DIR) not in sys.path:
    sys.path.insert(0, str(TESTS_DIR))

TERMINAL_STATUSES = {"completed", "failed", "timeout", "cancelled", "abandoned"}
DEFAULT_CONTAINERS = [
    "attune-api",
    "attune-executor-1",
    "attune-executor-2",
    "attune-worker-shell",
    "attune-worker-python",
    "attune-worker-node",
    "attune-worker-full",
    "attune-sensor",
    "attune-notifier",
    "attune-supervisor",
    "attune-postgres",
    "attune-rabbitmq",
    "attune-valkey",
]
CACHE_WORKLOADS = (
    "metadata_api",
    "execution_throughput",
    "hot_path_cache",
    "automation_latency",
    "steady_state_hot_path",
)


@dataclass
class BenchmarkSettings:
    profile: str = "smoke"
    base_url: str = "http://localhost:8080"
    metadata_warmup_seconds: float = 3.0
    metadata_duration_seconds: float = 12.0
    metadata_concurrency: int = 12
    execution_warmup_count: int = 8
    execution_count: int = 60
    execution_concurrency: int = 8
    execution_workflow_parent_count: int = 1
    execution_measurement_rounds: int = 3
    e2e_warmup_iterations: int = 1
    e2e_pipelines: int = 4
    e2e_iterations: int = 6
    e2e_measurement_rounds: int = 3
    poll_interval_seconds: float = 0.25
    e2e_poll_interval_seconds: float = 0.05
    execution_timeout_seconds: int = 45
    sampler_interval_seconds: float = 1.0
    metadata_seed_count: int = 12
    hotpath_warmup_iterations: int = 8
    hotpath_iterations: int = 40
    hotpath_concurrency: int = 8
    queue_poll_settle_seconds: float = 3.0
    steady_state_windows: int = 0
    steady_state_iterations_per_window: int = 0
    steady_state_pause_seconds: float = 5.0
    steady_state_concurrency: int = 8


PROFILE_OVERRIDES: dict[str, dict[str, Any]] = {
    "smoke": {},
    "metadata-heavy": {
        "metadata_warmup_seconds": 5.0,
        "metadata_duration_seconds": 30.0,
        "metadata_concurrency": 24,
        "metadata_seed_count": 60,
        "hotpath_iterations": 80,
    },
    "automation-hotpath": {
        "metadata_seed_count": 30,
        "hotpath_warmup_iterations": 24,
        "hotpath_iterations": 400,
        "hotpath_concurrency": 24,
        "e2e_pipelines": 8,
        "e2e_iterations": 25,
        "e2e_measurement_rounds": 3,
        "queue_poll_settle_seconds": 5.0,
    },
    "execution-throughput": {
        "metadata_seed_count": 24,
        "execution_warmup_count": 32,
        "execution_count": 600,
        "execution_concurrency": 16,
        "execution_workflow_parent_count": 4,
        "execution_measurement_rounds": 3,
        "execution_timeout_seconds": 120,
        "metadata_duration_seconds": 8.0,
        "hotpath_iterations": 80,
    },
    "runtime-queue": {
        "metadata_seed_count": 30,
        "metadata_duration_seconds": 20.0,
        "metadata_concurrency": 16,
        "hotpath_warmup_iterations": 24,
        "hotpath_iterations": 160,
        "hotpath_concurrency": 16,
        "queue_poll_settle_seconds": 10.0,
    },
    "stable": {
        "metadata_warmup_seconds": 8.0,
        "metadata_duration_seconds": 45.0,
        "metadata_concurrency": 24,
        "metadata_seed_count": 60,
        "execution_warmup_count": 32,
        "execution_count": 800,
        "execution_concurrency": 16,
        "execution_workflow_parent_count": 4,
        "execution_measurement_rounds": 5,
        "execution_timeout_seconds": 150,
        "hotpath_warmup_iterations": 32,
        "hotpath_iterations": 500,
        "hotpath_concurrency": 24,
        "e2e_pipelines": 8,
        "e2e_iterations": 30,
        "e2e_measurement_rounds": 5,
        "queue_poll_settle_seconds": 8.0,
    },
    "soak": {
        "metadata_warmup_seconds": 15.0,
        "metadata_duration_seconds": 60.0,
        "metadata_concurrency": 24,
        "metadata_seed_count": 80,
        "execution_warmup_count": 64,
        "execution_count": 1200,
        "execution_concurrency": 16,
        "execution_workflow_parent_count": 6,
        "execution_measurement_rounds": 6,
        "execution_timeout_seconds": 240,
        "hotpath_warmup_iterations": 64,
        "hotpath_iterations": 800,
        "hotpath_concurrency": 24,
        "e2e_pipelines": 10,
        "e2e_iterations": 50,
        "e2e_measurement_rounds": 6,
        "queue_poll_settle_seconds": 10.0,
        "steady_state_windows": 6,
        "steady_state_iterations_per_window": 160,
        "steady_state_pause_seconds": 10.0,
        "steady_state_concurrency": 16,
    },
}


PROFILE_CONTROLLED_FIELDS = tuple(field for field in BenchmarkSettings.__dataclass_fields__ if field != "profile")


def build_settings_from_args(args: argparse.Namespace) -> BenchmarkSettings:
    profile = args.profile
    if profile not in PROFILE_OVERRIDES:
        raise ValueError(f"unknown benchmark profile: {profile}")
    values = asdict(BenchmarkSettings())
    values.update(PROFILE_OVERRIDES[profile])
    values["profile"] = profile
    for field in PROFILE_CONTROLLED_FIELDS:
        override = getattr(args, field, None)
        if override is not None:
            values[field] = override
    return BenchmarkSettings(**values)


@dataclass
class E2EPipeline:
    trigger_ref: str
    webhook_key: str
    marker: str


def utc_now() -> datetime:
    return datetime.now(timezone.utc)


def utc_now_iso() -> str:
    return utc_now().isoformat().replace("+00:00", "Z")


def parse_timestamp(value: str | None) -> datetime | None:
    if not value:
        return None
    return datetime.fromisoformat(value.replace("Z", "+00:00"))


def quantile(samples: Sequence[float], percentile: float) -> float | None:
    if not samples:
        return None
    if len(samples) == 1:
        return float(samples[0])
    ordered = sorted(float(x) for x in samples)
    position = (len(ordered) - 1) * percentile
    lower = math.floor(position)
    upper = math.ceil(position)
    if lower == upper:
        return ordered[int(position)]
    lower_value = ordered[lower]
    upper_value = ordered[upper]
    return lower_value + (upper_value - lower_value) * (position - lower)


def summarize_samples(samples: Sequence[float]) -> dict[str, Any]:
    values = [float(value) for value in samples]
    if not values:
        return {
            "count": 0,
            "min": None,
            "max": None,
            "mean": None,
            "stdev": None,
            "p50": None,
            "p95": None,
            "p99": None,
        }
    return {
        "count": len(values),
        "min": min(values),
        "max": max(values),
        "mean": statistics.fmean(values),
        "stdev": statistics.stdev(values) if len(values) > 1 else 0.0,
        "p50": quantile(values, 0.50),
        "p95": quantile(values, 0.95),
        "p99": quantile(values, 0.99),
    }


def safe_div(numerator: float, denominator: float) -> float | None:
    if denominator == 0:
        return None
    return numerator / denominator


def parse_percent(value: str | None) -> float | None:
    if not value:
        return None
    try:
        return float(value.replace("%", "").strip())
    except ValueError:
        return None


def parse_mem_usage(value: str | None) -> dict[str, Any] | None:
    if not value or " / " not in value:
        return None
    used, limit = value.split(" / ", 1)
    return {"used": used.strip(), "limit": limit.strip()}


def deep_get(payload: dict[str, Any], path: Sequence[str]) -> Any:
    current: Any = payload
    for part in path:
        if not isinstance(current, dict):
            return None
        current = current.get(part)
    return current


def valkey_delta(payload: dict[str, Any], key: str) -> int | float:
    value = deep_get(payload, ["valkey", "delta", key])
    return value if isinstance(value, (int, float)) else 0


def metadata_cache_delta(payload: dict[str, Any], key: str) -> int | float:
    value = deep_get(payload, ["metadata_cache", "delta", key])
    return value if isinstance(value, (int, float)) else 0


def metadata_cache_engagement(payload: dict[str, Any]) -> int | float:
    return sum(
        metadata_cache_delta(payload, key)
        for key in (
            "l1_json_hits",
            "l1_index_hits",
            "l2_json_hits",
            "l2_index_hits",
        )
    )


def percentile_sample_warnings(payload: dict[str, Any], min_count: int = 100) -> list[dict[str, Any]]:
    warnings: list[dict[str, Any]] = []
    for workload in CACHE_WORKLOADS:
        workload_payload = payload.get(workload, {})
        if not isinstance(workload_payload, dict):
            continue
        for metric_name, metric_payload in workload_payload.items():
            if not isinstance(metric_payload, dict):
                continue
            if "p95" not in metric_payload and "p99" not in metric_payload:
                continue
            count = metric_payload.get("count")
            if isinstance(count, int) and 0 < count < min_count:
                warnings.append(
                    {
                        "workload": workload,
                        "metric": metric_name,
                        "count": count,
                        "message": f"{workload}.{metric_name} p95/p99 are based on only {count} samples",
                    }
                )
    return warnings


def profile_interpretation_notes(payload: dict[str, Any]) -> list[str]:
    settings = payload.get("settings", {})
    if not isinstance(settings, dict):
        return []
    notes: list[str] = []
    profile = settings.get("profile")
    if profile == "smoke":
        notes.append("The smoke profile is intended for quick regression checks, not stable throughput conclusions.")
    execution_count = settings.get("execution_count")
    if isinstance(execution_count, int) and execution_count < 300:
        notes.append(
            f"Execution throughput uses only {execution_count} child executions; RabbitMQ/worker timing variance can dominate percentages."
        )
    execution_rounds = settings.get("execution_measurement_rounds")
    if isinstance(execution_rounds, int) and execution_rounds < 3:
        notes.append(
            "Execution throughput uses fewer than 3 measurement rounds; a single scheduler/worker outlier can dominate p95/p99 comparisons."
        )
    e2e_samples = None
    pipelines = settings.get("e2e_pipelines")
    iterations = settings.get("e2e_iterations")
    if isinstance(pipelines, int) and isinstance(iterations, int):
        e2e_samples = pipelines * iterations
    if isinstance(e2e_samples, int) and e2e_samples < 100:
        notes.append(f"Automation latency p95/p99 are based on only {e2e_samples} end-to-end samples.")
    e2e_rounds = settings.get("e2e_measurement_rounds")
    if isinstance(e2e_rounds, int) and e2e_rounds < 3:
        notes.append(
            "Automation latency uses fewer than 3 measurement rounds; webhook, executor, and worker timing variance can dominate p95/p99 comparisons."
        )
    return notes


def unwrap_data(payload: Any) -> Any:
    if hasattr(payload, "json"):
        payload = payload.json()
    if "data" in payload:
        return payload["data"]
    return payload


def unwrap_items(payload: dict[str, Any]) -> list[dict[str, Any]]:
    if "data" in payload and isinstance(payload["data"], list):
        return payload["data"]
    if "data" in payload and isinstance(payload["data"], dict):
        data = payload["data"]
        if isinstance(data.get("items"), list):
            return data["items"]
    if "items" in payload and isinstance(payload["items"], list):
        return payload["items"]
    return []


def compare_metric(cache_on: float | None, cache_off: float | None, lower_is_better: bool) -> dict[str, Any]:
    if cache_on is None or cache_off is None:
        return {
            "cache_on": cache_on,
            "cache_off": cache_off,
            "delta": None,
            "delta_percent": None,
            "winner": None,
        }
    delta = cache_on - cache_off
    delta_percent = safe_div(delta * 100.0, cache_off)
    if lower_is_better:
        winner = "cache-on" if cache_on < cache_off else "cache-off"
    else:
        winner = "cache-on" if cache_on > cache_off else "cache-off"
    return {
        "cache_on": cache_on,
        "cache_off": cache_off,
        "delta": delta,
        "delta_percent": delta_percent,
        "winner": winner,
    }


def run_command(command: Sequence[str], cwd: Path | None = None, check: bool = True) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        command,
        cwd=str(cwd) if cwd else None,
        check=check,
        capture_output=True,
        text=True,
    )


def collect_docker_stats(containers: Sequence[str]) -> list[dict[str, Any]]:
    try:
        result = run_command(
            ["docker", "stats", "--no-stream", "--format", "{{json .}}", *containers],
            check=False,
        )
    except FileNotFoundError:
        return []
    if result.returncode != 0:
        return []
    rows: list[dict[str, Any]] = []
    for line in result.stdout.splitlines():
        line = line.strip()
        if not line:
            continue
        try:
            entry = json.loads(line)
        except json.JSONDecodeError:
            continue
        rows.append(
            {
                "container": entry.get("Container"),
                "name": entry.get("Name"),
                "cpu_percent": parse_percent(entry.get("CPUPerc")),
                "memory_percent": parse_percent(entry.get("MemPerc")),
                "memory_usage": parse_mem_usage(entry.get("MemUsage")),
                "net_io": entry.get("NetIO"),
                "block_io": entry.get("BlockIO"),
                "pids": entry.get("PIDs"),
                "raw": entry,
            }
        )
    return rows


class DockerSampler:
    def __init__(self, containers: Sequence[str], interval_seconds: float) -> None:
        self.containers = list(containers)
        self.interval_seconds = interval_seconds
        self.samples: list[dict[str, Any]] = []
        self._stop = threading.Event()
        self._thread: threading.Thread | None = None

    def start(self) -> None:
        self._stop.clear()
        self._thread = threading.Thread(target=self._run, daemon=True)
        self._thread.start()

    def stop(self) -> list[dict[str, Any]]:
        self._stop.set()
        if self._thread:
            self._thread.join(timeout=self.interval_seconds + 1.0)
        return self.samples

    def _run(self) -> None:
        while not self._stop.is_set():
            self.samples.append(
                {
                    "timestamp": utc_now_iso(),
                    "containers": collect_docker_stats(self.containers),
                }
            )
            self._stop.wait(self.interval_seconds)


def summarize_docker_samples(samples: Sequence[dict[str, Any]]) -> dict[str, Any]:
    by_container: dict[str, dict[str, list[float]]] = {}
    for sample in samples:
        for container in sample.get("containers", []):
            name = container.get("container") or container.get("name")
            if not name:
                continue
            bucket = by_container.setdefault(name, {"cpu_percent": [], "memory_percent": []})
            cpu = container.get("cpu_percent")
            mem = container.get("memory_percent")
            if isinstance(cpu, (int, float)):
                bucket["cpu_percent"].append(float(cpu))
            if isinstance(mem, (int, float)):
                bucket["memory_percent"].append(float(mem))
    summary: dict[str, Any] = {}
    for name, metrics in by_container.items():
        summary[name] = {
            "cpu_percent": summarize_samples(metrics["cpu_percent"]),
            "memory_percent": summarize_samples(metrics["memory_percent"]),
        }
    return summary


def collect_valkey_info() -> dict[str, Any]:
    try:
        result = run_command(["docker", "exec", "attune-valkey", "valkey-cli", "INFO", "stats"], check=False)
    except FileNotFoundError:
        return {}
    if result.returncode != 0:
        return {}
    info: dict[str, Any] = {}
    for line in result.stdout.splitlines():
        line = line.strip()
        if not line or line.startswith("#") or ":" not in line:
            continue
        key, value = line.split(":", 1)
        raw = value.strip()
        if raw.isdigit():
            info[key] = int(raw)
            continue
        try:
            info[key] = float(raw)
        except ValueError:
            info[key] = raw
    return info


def subtract_numeric_maps(after: dict[str, Any], before: dict[str, Any]) -> dict[str, Any]:
    keys = set(after) | set(before)
    delta: dict[str, Any] = {}
    for key in keys:
        after_value = after.get(key)
        before_value = before.get(key)
        if isinstance(after_value, (int, float)) and isinstance(before_value, (int, float)):
            delta[key] = after_value - before_value
    return delta


class BenchmarkRunner:
    def __init__(self, mode: str, settings: BenchmarkSettings) -> None:
        from helpers.client_wrapper import AttuneClient

        self.mode = mode
        self.settings = settings
        self.client = AttuneClient(base_url=settings.base_url, timeout=30, auto_login=False)
        self.pack_ref = f"bench_{mode.replace('-', '_')}_{uuid.uuid4().hex[:10]}"
        self.seed_summary: dict[str, Any] = {}
        self.metadata_endpoints: list[dict[str, str]] = []
        self.pipelines: list[E2EPipeline] = []
        self.workflow_ref = f"{self.pack_ref}.fanout_noop"
        self.queue_ref = f"{self.pack_ref}.cache_poll_queue"
        self.sanity_checks: list[dict[str, Any]] = []
        self.suite_started_at = utc_now()

    @property
    def auth_header(self) -> dict[str, str]:
        token = getattr(self.client, "access_token", None) or getattr(self.client, "token", None)
        if not token:
            raise RuntimeError("client is not authenticated")
        return {"Authorization": f"Bearer {token}"}

    def build_session(self) -> requests.Session:
        session = requests.Session()
        session.headers.update(self.auth_header)
        return session

    def wait_for_api(self, timeout_seconds: int = 300) -> None:
        deadline = time.monotonic() + timeout_seconds
        last_error: Exception | None = None
        while time.monotonic() < deadline:
            try:
                response = requests.get(f"{self.settings.base_url.rstrip('/')}/health", timeout=5)
                if response.ok:
                    self.client.login()
                    return
            except Exception as exc:  # noqa: BLE001
                last_error = exc
            time.sleep(2)
        raise RuntimeError(f"Attune API did not become ready within {timeout_seconds}s") from last_error

    def get_json(self, path: str, **kwargs: Any) -> dict[str, Any]:
        return self.client.get(path, **kwargs)

    def post_json(self, path: str, **kwargs: Any) -> dict[str, Any]:
        return self.client.post(path, **kwargs)

    def collect_metadata_cache_stats(self) -> dict[str, Any]:
        try:
            return unwrap_data(self.client.get("/api/v1/diagnostics/metadata-cache"))
        except Exception as exc:  # noqa: BLE001
            return {"error": str(exc)}

    def seed(self) -> None:
        pack = self.client.create_pack(
            ref=self.pack_ref,
            label=f"Benchmark {self.mode}",
            description="Metadata cache benchmark pack",
        )
        trigger_refs: list[str] = []
        rule_refs: list[str] = []
        for index in range(self.settings.metadata_seed_count):
            trigger_ref = f"{self.pack_ref}.meta_trigger_{index}"
            rule_ref = f"{self.pack_ref}.meta_rule_{index}"
            trigger = self.client.create_trigger(
                ref=trigger_ref,
                label=f"Metadata Trigger {index}",
                pack_ref=self.pack_ref,
                description="Benchmark metadata trigger",
            )
            self.client.create_rule(
                ref=rule_ref,
                pack_ref=self.pack_ref,
                label=f"Metadata Rule {index}",
                description="Benchmark metadata rule",
                trigger_ref=trigger["ref"],
                action_ref="core.noop",
                enabled=True,
                action_params={"message": f"metadata-rule-{index}"},
            )
            trigger_refs.append(trigger_ref)
            rule_refs.append(rule_ref)

        workflow = self.create_execution_benchmark_workflow()
        queue = self.create_queue_poll_benchmark_queue()

        pipelines: list[E2EPipeline] = []
        for index in range(self.settings.e2e_pipelines):
            trigger_ref = f"{self.pack_ref}.latency_trigger_{index}"
            trigger = self.client.create_trigger(
                ref=trigger_ref,
                label=f"Latency Trigger {index}",
                pack_ref=self.pack_ref,
                description="Benchmark latency trigger",
            )
            enabled = self.client.post(
                f"/api/v1/triggers/{quote(trigger['ref'], safe='')}/webhooks/enable"
            )["data"]
            marker = f"{self.pack_ref}:pipeline:{index}"
            self.client.create_rule(
                ref=f"{self.pack_ref}.latency_rule_{index}",
                pack_ref=self.pack_ref,
                label=f"Latency Rule {index}",
                description="Benchmark latency rule",
                trigger_ref=trigger["ref"],
                action_ref="core.echo",
                enabled=True,
                action_params={"message": marker},
            )
            pipelines.append(
                E2EPipeline(
                    trigger_ref=trigger_ref,
                    webhook_key=enabled["webhook_key"],
                    marker=marker,
                )
            )

        self.pipelines = pipelines
        self.seed_summary = {
            "pack": pack,
            "workflow": workflow,
            "queue": queue,
            "metadata_trigger_refs": trigger_refs,
            "metadata_rule_refs": rule_refs,
            "pipelines": [asdict(pipeline) for pipeline in pipelines],
        }
        self.metadata_endpoints = self.discover_metadata_endpoints()
        self.record_sanity_check("seed-pack-created", bool(pack.get("id")), f"pack_ref={self.pack_ref}")
        self.record_sanity_check(
            "execution-workflow-created",
            bool(workflow.get("id")),
            f"workflow_ref={self.workflow_ref}",
        )
        self.record_sanity_check(
            "queue-poll-definition-created",
            bool(queue.get("id")),
            f"queue_ref={self.queue_ref}",
        )
        self.record_sanity_check(
            "metadata-endpoints-discovered",
            len(self.metadata_endpoints) >= 12,
            f"count={len(self.metadata_endpoints)}",
        )
        self.record_sanity_check(
            "e2e-pipelines-created",
            len(self.pipelines) == self.settings.e2e_pipelines,
            f"count={len(self.pipelines)}",
        )

    def create_execution_benchmark_workflow(self) -> dict[str, Any]:
        workflow_name = self.workflow_ref.split(".", 1)[1]
        tasks = [
            {
                "name": "noop_each",
                "action": "core.noop",
                "with_items": "{{ parameters.items }}",
                "concurrency": self.settings.execution_concurrency,
                "input": {
                    "message": "{{ parameters.message_prefix }}:{{ index }}:{{ item }}",
                },
            }
        ]
        return self.client.create_workflow(
            pack_ref=self.pack_ref,
            name=workflow_name,
            label="Fan-out No-Op Benchmark",
            description="Benchmark workflow that creates child executions inside Attune",
            version="1.0.0",
            param_schema={
                "items": {
                    "type": "array",
                    "required": True,
                    "description": "Items expanded into core.noop child executions",
                },
                "message_prefix": {
                    "type": "string",
                    "required": True,
                    "description": "Marker prefix for child execution messages",
                },
            },
            out_schema={},
            tags=["benchmark", "metadata-cache", "fanout"],
            tasks=tasks,
        )

    def create_queue_poll_benchmark_queue(self) -> dict[str, Any]:
        return unwrap_data(
            self.client.post(
                "/api/v1/queues",
                json={
                    "ref": self.queue_ref,
                    "pack_ref": self.pack_ref,
                    "label": "Cache Poll Queue Benchmark",
                    "description": "Enabled queue definition used to exercise dispatcher queue metadata polling",
                    "enabled": True,
                    "accepting_new_items": False,
                    "dispatch_action_ref": "core.noop",
                    "default_priority": 0,
                    "allow_pending_update": False,
                    "batch_mode": "single",
                    "item_schema": {},
                    "action_params": {"message": "{{ item.message }}"},
                    "config": {
                        "dispatch": {
                            "concurrency": {"source": "literal", "value": 1},
                            "batch_size": {"source": "literal", "value": 1},
                        }
                    },
                },
            )
        )

    def discover_metadata_endpoints(self) -> list[dict[str, str]]:
        endpoints: list[dict[str, str]] = [
            {"name": "packs-list", "path": "/api/v1/packs"},
            {"name": "actions-list", "path": "/api/v1/actions"},
            {"name": "triggers-list", "path": "/api/v1/triggers"},
            {"name": "rules-list", "path": "/api/v1/rules"},
            {"name": "runtimes-list", "path": "/api/v1/runtimes"},
            {"name": "benchmark-pack-runtimes-list", "path": f"/api/v1/packs/{quote(self.pack_ref, safe='')}/runtimes"},
            {"name": "workflows-list", "path": "/api/v1/workflows"},
            {"name": "queues-list", "path": "/api/v1/queues"},
            {"name": "policies-list", "path": "/api/v1/policies"},
        ]
        seeded_detail_endpoints = [
            ("benchmark-pack-detail", f"/api/v1/packs/{quote(self.pack_ref, safe='')}"),
            ("workflow-action-detail", f"/api/v1/actions/{quote(self.workflow_ref, safe='')}"),
            ("workflow-detail", f"/api/v1/workflows/{quote(self.workflow_ref, safe='')}"),
        ]
        if self.seed_summary.get("metadata_trigger_refs"):
            trigger_ref = self.seed_summary["metadata_trigger_refs"][0]
            seeded_detail_endpoints.append(
                ("benchmark-trigger-detail", f"/api/v1/triggers/{quote(trigger_ref, safe='')}")
            )
        if self.seed_summary.get("metadata_rule_refs"):
            rule_ref = self.seed_summary["metadata_rule_refs"][0]
            seeded_detail_endpoints.append(
                ("benchmark-rule-detail", f"/api/v1/rules/{quote(rule_ref, safe='')}")
            )
        endpoints.extend({"name": name, "path": path} for name, path in seeded_detail_endpoints)

        detail_candidates = [
            ("packs", "/api/v1/packs", lambda item: quote(item.get("ref", ""), safe="")),
            ("actions", "/api/v1/actions", lambda item: quote(item.get("ref", ""), safe="")),
            ("triggers", "/api/v1/triggers", lambda item: quote(item.get("ref", ""), safe="")),
            ("rules", "/api/v1/rules", lambda item: quote(item.get("ref", ""), safe="")),
            ("runtimes", "/api/v1/runtimes", lambda item: quote(item.get("ref", ""), safe="")),
            ("workflows", "/api/v1/workflows", lambda item: quote(item.get("ref", ""), safe="")),
            ("queues", "/api/v1/queues", lambda item: quote(item.get("ref", ""), safe="")),
            ("policies", "/api/v1/policies", lambda item: quote(item.get("ref", ""), safe="")),
        ]
        seen_paths = {endpoint["path"] for endpoint in endpoints}
        for name, path, key_builder in detail_candidates:
            try:
                response = self.client.get(path)
                items = unwrap_items(response)
                if items:
                    key = key_builder(items[0])
                    detail_path = f"{path}/{key}"
                    if key and key != "None" and detail_path not in seen_paths:
                        endpoints.append({"name": f"{name}-detail", "path": detail_path})
                        seen_paths.add(detail_path)
            except Exception:  # noqa: BLE001
                continue
        return endpoints

    def record_sanity_check(self, name: str, passed: bool, detail: str) -> None:
        self.sanity_checks.append({"name": name, "passed": passed, "detail": detail})

    def capture_workload(self, workload_name: str, fn: Callable[[], dict[str, Any]]) -> dict[str, Any]:
        before_metadata_cache = self.collect_metadata_cache_stats()
        before_valkey = collect_valkey_info()
        before_docker = collect_docker_stats(DEFAULT_CONTAINERS)
        sampler = DockerSampler(DEFAULT_CONTAINERS, self.settings.sampler_interval_seconds)
        started_at = utc_now_iso()
        sampler.start()
        try:
            payload = fn()
        finally:
            samples = sampler.stop()
        completed_at = utc_now_iso()
        after_metadata_cache = self.collect_metadata_cache_stats()
        after_valkey = collect_valkey_info()
        after_docker = collect_docker_stats(DEFAULT_CONTAINERS)
        payload.update(
            {
                "started_at": started_at,
                "completed_at": completed_at,
                "docker": {
                    "before": before_docker,
                    "after": after_docker,
                    "samples": samples,
                    "summary": summarize_docker_samples(samples),
                },
                "metadata_cache": {
                    "before": before_metadata_cache,
                    "after": after_metadata_cache,
                    "delta": subtract_numeric_maps(after_metadata_cache, before_metadata_cache),
                },
                "valkey": {
                    "before": before_valkey,
                    "after": after_valkey,
                    "delta": subtract_numeric_maps(after_valkey, before_valkey),
                },
            }
        )
        return payload

    def run_metadata_warmup(self) -> None:
        deadline = time.monotonic() + self.settings.metadata_warmup_seconds
        session = self.build_session()
        index = 0
        while time.monotonic() < deadline:
            endpoint = self.metadata_endpoints[index % len(self.metadata_endpoints)]
            session.get(f"{self.settings.base_url.rstrip('/')}{endpoint['path']}", timeout=10)
            index += 1

    def run_cross_service_cache_prewarm(self) -> dict[str, Any]:
        before_metadata_cache = self.collect_metadata_cache_stats()
        before_valkey = collect_valkey_info()
        started = time.perf_counter()
        session = self.build_session()
        api_requests = 0
        api_failures = 0
        for endpoint in self.metadata_endpoints:
            try:
                response = session.get(f"{self.settings.base_url.rstrip('/')}{endpoint['path']}", timeout=10)
                api_requests += 1
                if not response.ok:
                    api_failures += 1
            except Exception:  # noqa: BLE001
                api_requests += 1
                api_failures += 1

        self.run_execution_warmup()
        self.run_hotpath_warmup()
        if self.settings.queue_poll_settle_seconds > 0:
            time.sleep(min(self.settings.queue_poll_settle_seconds, 3.0))

        elapsed = time.perf_counter() - started
        after_metadata_cache = self.collect_metadata_cache_stats()
        after_valkey = collect_valkey_info()
        return {
            "duration_seconds": elapsed,
            "api_requests": api_requests,
            "api_failures": api_failures,
            "execution_warmup_count": self.settings.execution_warmup_count,
            "hotpath_warmup_iterations": self.settings.hotpath_warmup_iterations,
            "metadata_cache": {
                "before": before_metadata_cache,
                "after": after_metadata_cache,
                "delta": subtract_numeric_maps(after_metadata_cache, before_metadata_cache),
            },
            "valkey": {
                "before": before_valkey,
                "after": after_valkey,
                "delta": subtract_numeric_maps(after_valkey, before_valkey),
            },
            "paths": [
                "api_metadata_read_through",
                "executor_workflow_fanout_metadata",
                "worker_action_runtime_metadata",
                "webhook_trigger_rule_action_metadata",
                "queue_enabled_definition_polling",
            ],
        }

    def run_metadata_measurement(self) -> dict[str, Any]:
        deadline = time.monotonic() + self.settings.metadata_duration_seconds

        def worker(worker_index: int) -> dict[str, Any]:
            session = self.build_session()
            latencies: list[float] = []
            status_codes: Counter[int] = Counter()
            failures = 0
            index = worker_index
            while time.monotonic() < deadline:
                endpoint = self.metadata_endpoints[index % len(self.metadata_endpoints)]
                started = time.perf_counter()
                try:
                    response = session.get(
                        f"{self.settings.base_url.rstrip('/')}{endpoint['path']}",
                        timeout=10,
                    )
                    latency_ms = (time.perf_counter() - started) * 1000.0
                    latencies.append(latency_ms)
                    status_codes[response.status_code] += 1
                    if not response.ok:
                        failures += 1
                except requests.RequestException:
                    failures += 1
                index += 1
            return {
                "latencies": latencies,
                "status_codes": dict(status_codes),
                "failures": failures,
            }

        with concurrent.futures.ThreadPoolExecutor(max_workers=self.settings.metadata_concurrency) as pool:
            results = list(pool.map(worker, range(self.settings.metadata_concurrency)))

        all_latencies = [latency for result in results for latency in result["latencies"]]
        all_statuses: Counter[int] = Counter()
        failures = 0
        for result in results:
            all_statuses.update(result["status_codes"])
            failures += result["failures"]

        duration_seconds = self.settings.metadata_duration_seconds
        requests_completed = len(all_latencies)
        success_count = sum(count for code, count in all_statuses.items() if 200 <= int(code) < 300)
        return {
            "requests_completed": requests_completed,
            "success_count": success_count,
            "failure_count": failures,
            "success_rate": safe_div(success_count, requests_completed),
            "requests_per_second": safe_div(requests_completed, duration_seconds),
            "latency_ms": summarize_samples(all_latencies),
            "status_codes": dict(all_statuses),
            "endpoint_count": len(self.metadata_endpoints),
            "warmup_seconds": self.settings.metadata_warmup_seconds,
            "duration_seconds": duration_seconds,
        }

    def run_execution_warmup(self) -> None:
        parent_count = 1
        child_counts = self.distribute_child_counts(self.settings.execution_warmup_count, parent_count)
        parents = self.submit_workflow_executions(child_counts, parent_count, warmup=True)
        self.wait_for_workflow_batch(parents)

    @staticmethod
    def distribute_child_counts(total_count: int, parent_count: int) -> list[int]:
        parent_count = max(1, parent_count)
        base = total_count // parent_count
        remainder = total_count % parent_count
        return [base + (1 if index < remainder else 0) for index in range(parent_count) if base + (1 if index < remainder else 0) > 0]

    def submit_workflow_executions(
        self,
        child_counts: Sequence[int],
        concurrency: int,
        warmup: bool = False,
    ) -> list[dict[str, Any]]:
        def submit(index_and_count: tuple[int, int]) -> dict[str, Any]:
            index, child_count = index_and_count
            marker = f"{self.pack_ref}-{'warmup' if warmup else 'bench'}-workflow-{index}-{uuid.uuid4().hex[:6]}"
            session = self.build_session()
            started = time.perf_counter()
            response = session.post(
                f"{self.settings.base_url.rstrip('/')}/api/v1/executions/execute",
                json={
                    "action_ref": self.workflow_ref,
                    "parameters": {
                        "items": list(range(child_count)),
                        "message_prefix": marker,
                    },
                },
                timeout=15,
            )
            response.raise_for_status()
            payload = unwrap_data(response.json())
            return {
                "id": payload["id"],
                "marker": marker,
                "expected_children": child_count,
                "submit_latency_ms": (time.perf_counter() - started) * 1000.0,
            }

        with concurrent.futures.ThreadPoolExecutor(max_workers=concurrency) as pool:
            return list(pool.map(submit, enumerate(child_counts)))

    def wait_for_terminal_execution(self, execution_id: int, poll_interval_seconds: float | None = None) -> dict[str, Any]:
        session = self.build_session()
        deadline = time.monotonic() + self.settings.execution_timeout_seconds
        poll_interval = poll_interval_seconds if poll_interval_seconds is not None else self.settings.poll_interval_seconds
        while time.monotonic() < deadline:
            response = session.get(
                f"{self.settings.base_url.rstrip('/')}/api/v1/executions/{execution_id}",
                timeout=10,
            )
            response.raise_for_status()
            execution = response.json()["data"]
            if execution["status"] in TERMINAL_STATUSES:
                return execution
            time.sleep(poll_interval)
        raise TimeoutError(f"Execution {execution_id} did not reach terminal state")

    def wait_for_terminal_executions(self, executions: Sequence[dict[str, Any]]) -> list[dict[str, Any]]:
        with concurrent.futures.ThreadPoolExecutor(max_workers=min(16, max(1, len(executions)))) as pool:
            return list(pool.map(lambda item: self.wait_for_terminal_execution(item["id"]), executions))

    def list_child_executions(self, session: requests.Session, parent_id: int) -> list[dict[str, Any]]:
        children: list[dict[str, Any]] = []
        page = 1
        while True:
            response = session.get(
                f"{self.settings.base_url.rstrip('/')}/api/v1/executions",
                params={"parent": parent_id, "page": page, "per_page": 100},
                timeout=10,
            )
            response.raise_for_status()
            page_items = unwrap_items(response.json())
            children.extend(page_items)
            if len(page_items) < 100:
                return children
            page += 1

    def wait_for_workflow_children(self, parent: dict[str, Any]) -> list[dict[str, Any]]:
        session = self.build_session()
        deadline = time.monotonic() + self.settings.execution_timeout_seconds
        expected_children = int(parent["expected_children"])
        last_children: list[dict[str, Any]] = []
        while time.monotonic() < deadline:
            children = self.list_child_executions(session, int(parent["id"]))
            last_children = children
            if len(children) >= expected_children and all(
                child.get("status") in TERMINAL_STATUSES for child in children
            ):
                return children
            time.sleep(self.settings.poll_interval_seconds)
        status_counts = Counter(child.get("status") for child in last_children)
        raise TimeoutError(
            f"Workflow execution {parent['id']} produced {len(last_children)}/{expected_children} terminal children; statuses={dict(status_counts)}"
        )

    def wait_for_workflow_batch(self, parents: Sequence[dict[str, Any]]) -> tuple[list[dict[str, Any]], list[dict[str, Any]]]:
        with concurrent.futures.ThreadPoolExecutor(max_workers=min(16, max(1, len(parents)))) as pool:
            child_batches = list(pool.map(self.wait_for_workflow_children, parents))
        terminal_parents = self.wait_for_terminal_executions(parents)
        children = [child for batch in child_batches for child in batch]
        return terminal_parents, children

    @staticmethod
    def select_median_throughput_round(rounds: Sequence[dict[str, Any]]) -> dict[str, Any]:
        if not rounds:
            return {}
        sorted_rounds = sorted(
            rounds,
            key=lambda item: (
                float(item.get("completed_per_second") or 0.0),
                -float(item.get("schedule_latency_ms", {}).get("p95") or 0.0),
            ),
        )
        return sorted_rounds[len(sorted_rounds) // 2]

    def run_execution_measurement_round(self, round_index: int) -> dict[str, Any]:
        parent_count = max(1, self.settings.execution_workflow_parent_count)
        child_counts = self.distribute_child_counts(self.settings.execution_count, parent_count)
        started = time.perf_counter()
        submissions = self.submit_workflow_executions(child_counts, parent_count)
        terminal_parents, terminals = self.wait_for_workflow_batch(submissions)
        wall_elapsed = time.perf_counter() - started
        submit_latencies = [item["submit_latency_ms"] for item in submissions]
        schedule_latencies: list[float] = []
        run_latencies: list[float] = []
        end_to_end_latencies: list[float] = []
        child_created_times: list[datetime] = []
        child_updated_times: list[datetime] = []
        status_counts: Counter[str] = Counter()
        for execution in terminals:
            status_counts[execution["status"]] += 1
            created = parse_timestamp(execution.get("created"))
            started_at = parse_timestamp(execution.get("started_at"))
            updated = parse_timestamp(execution.get("updated"))
            if created:
                child_created_times.append(created)
            if updated:
                child_updated_times.append(updated)
            if created and started_at:
                schedule_latencies.append((started_at - created).total_seconds() * 1000.0)
            if started_at and updated:
                run_latencies.append((updated - started_at).total_seconds() * 1000.0)
            if created and updated:
                end_to_end_latencies.append((updated - created).total_seconds() * 1000.0)

        completed = sum(1 for execution in terminals if execution["status"] == "completed")
        internal_elapsed = None
        if child_created_times and child_updated_times:
            internal_elapsed = (max(child_updated_times) - min(child_created_times)).total_seconds()
        parent_status_counts: Counter[str] = Counter(parent["status"] for parent in terminal_parents)
        return {
            "round_index": round_index,
            "submitted": sum(child_counts),
            "completed": completed,
            "parent_submitted": len(submissions),
            "parent_status_counts": dict(parent_status_counts),
            "status_counts": dict(status_counts),
            "submit_latency_ms": summarize_samples(submit_latencies),
            "schedule_latency_ms": summarize_samples(schedule_latencies),
            "run_latency_ms": summarize_samples(run_latencies),
            "end_to_end_latency_ms": summarize_samples(end_to_end_latencies),
            "completed_per_second": safe_div(completed, internal_elapsed) if internal_elapsed else None,
            "internal_elapsed_seconds": internal_elapsed,
            "wall_elapsed_seconds": wall_elapsed,
            "warmup_count": self.settings.execution_warmup_count,
            "count": self.settings.execution_count,
            "workflow_parent_count": len(submissions),
            "children_per_parent": child_counts,
            "workflow_child_concurrency": self.settings.execution_concurrency,
        }

    def run_execution_measurement(self) -> dict[str, Any]:
        round_count = max(1, self.settings.execution_measurement_rounds)
        rounds = [
            self.run_execution_measurement_round(round_index)
            for round_index in range(round_count)
        ]
        selected = dict(self.select_median_throughput_round(rounds))
        selected["measurement_rounds"] = round_count
        selected["selection_strategy"] = "median_completed_per_second"
        selected["all_rounds"] = rounds
        selected["round_completed_per_second"] = summarize_samples(
            [
                float(item["completed_per_second"])
                for item in rounds
                if item.get("completed_per_second") is not None
            ]
        )
        selected["round_schedule_p95_ms"] = summarize_samples(
            [
                float(item["schedule_latency_ms"]["p95"])
                for item in rounds
                if item.get("schedule_latency_ms", {}).get("p95") is not None
            ]
        )
        return selected

    def run_hotpath_warmup(self) -> None:
        self.run_hotpath_probes(self.settings.hotpath_warmup_iterations, warmup=True)

    def run_hotpath_probes(
        self,
        iterations: int,
        warmup: bool = False,
        concurrency: int | None = None,
    ) -> list[dict[str, Any]]:
        if iterations <= 0:
            return []

        def probe(index: int) -> dict[str, Any]:
            session = self.build_session()
            pipeline = self.pipelines[index % len(self.pipelines)]
            started = time.perf_counter()
            response = session.post(
                f"{self.settings.base_url.rstrip('/')}/api/v1/webhooks/{pipeline.webhook_key}",
                json={
                    "payload": {
                        "run_id": uuid.uuid4().hex,
                        "pipeline": pipeline.marker,
                        "hotpath": True,
                        "warmup": warmup,
                    }
                },
                timeout=15,
            )
            latency_ms = (time.perf_counter() - started) * 1000.0
            return {
                "status_code": response.status_code,
                "ok": response.ok,
                "latency_ms": latency_ms,
            }

        max_workers = concurrency or self.settings.hotpath_concurrency
        with concurrent.futures.ThreadPoolExecutor(max_workers=max_workers) as pool:
            return list(pool.map(probe, range(iterations)))

    @staticmethod
    def summarize_hotpath_probe_results(results: list[dict[str, Any]], elapsed: float) -> dict[str, Any]:
        status_counts: Counter[int] = Counter(item["status_code"] for item in results)
        success_count = sum(1 for item in results if item["ok"])
        latencies = [item["latency_ms"] for item in results]
        return {
            "requests_completed": len(results),
            "success_count": success_count,
            "failure_count": len(results) - success_count,
            "success_rate": safe_div(success_count, len(results)),
            "requests_per_second": safe_div(len(results), elapsed),
            "latency_ms": summarize_samples(latencies),
            "status_codes": dict(status_counts),
        }

    def run_hotpath_measurement(self) -> dict[str, Any]:
        started = time.perf_counter()
        results = self.run_hotpath_probes(self.settings.hotpath_iterations)
        if self.settings.queue_poll_settle_seconds > 0:
            time.sleep(self.settings.queue_poll_settle_seconds)
        elapsed = time.perf_counter() - started
        payload = self.summarize_hotpath_probe_results(results, elapsed)
        payload.update(
            {
                "warmup_iterations": self.settings.hotpath_warmup_iterations,
                "iterations": self.settings.hotpath_iterations,
                "concurrency": self.settings.hotpath_concurrency,
                "queue_poll_settle_seconds": self.settings.queue_poll_settle_seconds,
                "probed_paths": [
                    "webhook_key_trigger_lookup",
                    "rule_action_metadata_lookup",
                    "queue_enabled_definition_polling",
                ],
            }
        )
        return payload

    def run_steady_state_hotpath_measurement(self) -> dict[str, Any]:
        windows: list[dict[str, Any]] = []
        all_results: list[dict[str, Any]] = []
        total_started = time.perf_counter()

        for window_index in range(self.settings.steady_state_windows):
            before_cache = self.collect_metadata_cache_stats()
            before_valkey = collect_valkey_info()
            started = time.perf_counter()
            results = self.run_hotpath_probes(
                self.settings.steady_state_iterations_per_window,
                concurrency=self.settings.steady_state_concurrency,
            )
            elapsed = time.perf_counter() - started
            after_cache = self.collect_metadata_cache_stats()
            after_valkey = collect_valkey_info()

            window = self.summarize_hotpath_probe_results(results, elapsed)
            window.update(
                {
                    "window_index": window_index,
                    "metadata_cache": {
                        "before": before_cache,
                        "after": after_cache,
                        "delta": subtract_numeric_maps(after_cache, before_cache),
                    },
                    "valkey": {
                        "before": before_valkey,
                        "after": after_valkey,
                        "delta": subtract_numeric_maps(after_valkey, before_valkey),
                    },
                }
            )
            windows.append(window)
            all_results.extend(results)

            if (
                self.settings.steady_state_pause_seconds > 0
                and window_index < self.settings.steady_state_windows - 1
            ):
                time.sleep(self.settings.steady_state_pause_seconds)

        total_elapsed = time.perf_counter() - total_started
        payload = self.summarize_hotpath_probe_results(all_results, total_elapsed)
        first_p95 = deep_get(windows[0], ["latency_ms", "p95"]) if windows else None
        last_p95 = deep_get(windows[-1], ["latency_ms", "p95"]) if windows else None
        payload.update(
            {
                "windows": windows,
                "window_count": self.settings.steady_state_windows,
                "iterations_per_window": self.settings.steady_state_iterations_per_window,
                "concurrency": self.settings.steady_state_concurrency,
                "pause_seconds": self.settings.steady_state_pause_seconds,
                "first_window_latency_ms": windows[0]["latency_ms"] if windows else summarize_samples([]),
                "last_window_latency_ms": windows[-1]["latency_ms"] if windows else summarize_samples([]),
                "p95_drift_percent": (
                    safe_div((last_p95 - first_p95) * 100.0, first_p95)
                    if isinstance(first_p95, (int, float)) and isinstance(last_p95, (int, float))
                    else None
                ),
                "probed_paths": [
                    "long_running_webhook_key_trigger_lookup",
                    "long_running_rule_action_metadata_lookup",
                    "long_running_queue_enabled_definition_polling",
                    "warm_cache_l1_l2_stability",
                ],
            }
        )
        return payload

    def run_e2e_warmup(self) -> None:
        for pipeline in self.pipelines:
            seen: set[int] = set()
            for _ in range(self.settings.e2e_warmup_iterations):
                self.fire_pipeline_once(pipeline, seen)

    def list_echo_executions(
        self, session: requests.Session, trigger_ref: str | None = None
    ) -> list[dict[str, Any]]:
        params: dict[str, Any] = {"action_ref": "core.echo", "limit": 200, "offset": 0}
        if trigger_ref:
            params["trigger_ref"] = trigger_ref
        response = session.get(
            f"{self.settings.base_url.rstrip('/')}/api/v1/executions",
            params=params,
            timeout=10,
        )
        response.raise_for_status()
        return unwrap_items(response.json())

    def fire_pipeline_once(self, pipeline: E2EPipeline, seen_ids: set[int]) -> dict[str, Any]:
        session = self.build_session()
        known_ids = {
            execution["id"]
            for execution in self.list_echo_executions(session, trigger_ref=pipeline.trigger_ref)
        }
        started = time.perf_counter()
        response = session.post(
            f"{self.settings.base_url.rstrip('/')}/api/v1/webhooks/{pipeline.webhook_key}",
            json={"payload": {"run_id": uuid.uuid4().hex, "pipeline": pipeline.marker}},
            timeout=15,
        )
        post_latency_ms = (time.perf_counter() - started) * 1000.0
        response.raise_for_status()
        webhook_response = unwrap_data(response.json())
        event_created = parse_timestamp(webhook_response.get("received_at"))
        deadline = time.monotonic() + self.settings.execution_timeout_seconds
        execution_id: int | None = None
        while time.monotonic() < deadline and execution_id is None:
            for execution in self.list_echo_executions(session, trigger_ref=pipeline.trigger_ref):
                if (
                    execution["id"] not in seen_ids
                    and execution["id"] not in known_ids
                ):
                    execution_id = execution["id"]
                    seen_ids.add(execution_id)
                    break
            if execution_id is None:
                time.sleep(self.settings.e2e_poll_interval_seconds)
        if execution_id is None:
            raise TimeoutError(f"No execution observed for pipeline {pipeline.marker}")
        discovery_latency_ms = (time.perf_counter() - started) * 1000.0
        execution = self.wait_for_terminal_execution(
            execution_id,
            poll_interval_seconds=self.settings.e2e_poll_interval_seconds,
        )
        latency_ms = (time.perf_counter() - started) * 1000.0
        execution_created = parse_timestamp(execution.get("created"))
        execution_updated = parse_timestamp(execution.get("updated"))
        event_to_execution_created_ms = None
        execution_internal_ms = None
        if event_created and execution_created:
            event_to_execution_created_ms = (execution_created - event_created).total_seconds() * 1000.0
        if execution_created and execution_updated:
            execution_internal_ms = (execution_updated - execution_created).total_seconds() * 1000.0
        return {
            "execution_id": execution_id,
            "status": execution["status"],
            "latency_ms": latency_ms,
            "post_latency_ms": post_latency_ms,
            "discovery_latency_ms": discovery_latency_ms,
            "terminal_wait_latency_ms": latency_ms - discovery_latency_ms,
            "event_to_execution_created_ms": event_to_execution_created_ms,
            "execution_internal_ms": execution_internal_ms,
            "created": execution.get("created"),
            "updated": execution.get("updated"),
        }

    @staticmethod
    def select_median_latency_round(rounds: Sequence[dict[str, Any]]) -> dict[str, Any]:
        if not rounds:
            return {}
        successful_rounds = [
            item
            for item in rounds
            if item.get("latency_ms", {}).get("p95") is not None
            and (item.get("success_rate") or 0.0) > 0.0
        ]
        candidates = successful_rounds or list(rounds)
        sorted_rounds = sorted(
            candidates,
            key=lambda item: (
                float(item.get("latency_ms", {}).get("p95") or float("inf")),
                float(item.get("latency_ms", {}).get("p99") or float("inf")),
            ),
        )
        return sorted_rounds[len(sorted_rounds) // 2]

    def run_e2e_measurement_round(self, round_index: int) -> dict[str, Any]:
        def run_pipeline(pipeline: E2EPipeline) -> list[dict[str, Any]]:
            seen: set[int] = set()
            results: list[dict[str, Any]] = []
            for iteration in range(self.settings.e2e_iterations):
                started = time.perf_counter()
                try:
                    results.append(self.fire_pipeline_once(pipeline, seen))
                except TimeoutError as exc:
                    results.append(
                        {
                            "status": "timeout",
                            "pipeline": pipeline.marker,
                            "iteration": iteration,
                            "error": str(exc),
                            "latency_ms": (time.perf_counter() - started) * 1000.0,
                        }
                    )
                except requests.RequestException as exc:
                    results.append(
                        {
                            "status": "error",
                            "pipeline": pipeline.marker,
                            "iteration": iteration,
                            "error": str(exc),
                            "latency_ms": (time.perf_counter() - started) * 1000.0,
                        }
                    )
            return results

        with concurrent.futures.ThreadPoolExecutor(max_workers=self.settings.e2e_pipelines) as pool:
            all_results = [result for batch in pool.map(run_pipeline, self.pipelines) for result in batch]

        completed_results = [item for item in all_results if item.get("status") == "completed"]
        failed_results = [item for item in all_results if item.get("status") != "completed"]
        latencies = [item["latency_ms"] for item in completed_results]
        post_latencies = [item["post_latency_ms"] for item in completed_results if item.get("post_latency_ms") is not None]
        discovery_latencies = [
            item["discovery_latency_ms"]
            for item in completed_results
            if item.get("discovery_latency_ms") is not None
        ]
        terminal_wait_latencies = [
            item["terminal_wait_latency_ms"]
            for item in completed_results
            if item.get("terminal_wait_latency_ms") is not None
        ]
        event_to_execution_created = [
            item["event_to_execution_created_ms"]
            for item in completed_results
            if item.get("event_to_execution_created_ms") is not None
        ]
        execution_internal = [
            item["execution_internal_ms"]
            for item in completed_results
            if item.get("execution_internal_ms") is not None
        ]
        status_counts: Counter[str] = Counter(item["status"] for item in all_results)
        completed = status_counts.get("completed", 0)
        return {
            "round_index": round_index,
            "requests_completed": completed,
            "requests_attempted": len(all_results),
            "failure_count": len(failed_results),
            "status_counts": dict(status_counts),
            "success_rate": safe_div(completed, len(all_results)),
            "latency_ms": summarize_samples(latencies),
            "post_latency_ms": summarize_samples(post_latencies),
            "discovery_latency_ms": summarize_samples(discovery_latencies),
            "terminal_wait_latency_ms": summarize_samples(terminal_wait_latencies),
            "event_to_execution_created_ms": summarize_samples(event_to_execution_created),
            "execution_internal_ms": summarize_samples(execution_internal),
            "pipelines": self.settings.e2e_pipelines,
            "iterations_per_pipeline": self.settings.e2e_iterations,
            "warmup_iterations": self.settings.e2e_warmup_iterations,
            "poll_interval_seconds": self.settings.e2e_poll_interval_seconds,
            "failures": failed_results[:10],
        }

    def run_e2e_measurement(self) -> dict[str, Any]:
        round_count = max(1, self.settings.e2e_measurement_rounds)
        rounds = [
            self.run_e2e_measurement_round(round_index)
            for round_index in range(round_count)
        ]
        selected = dict(self.select_median_latency_round(rounds))
        selected["measurement_rounds"] = round_count
        selected["selection_strategy"] = "median_latency_p95"
        selected["all_rounds"] = rounds
        selected["round_latency_p95_ms"] = summarize_samples(
            [
                float(item["latency_ms"]["p95"])
                for item in rounds
                if item.get("latency_ms", {}).get("p95") is not None
            ]
        )
        selected["round_execution_internal_p99_ms"] = summarize_samples(
            [
                float(item["execution_internal_ms"]["p99"])
                for item in rounds
                if item.get("execution_internal_ms", {}).get("p99") is not None
            ]
        )
        return selected

    def collect_analytics(self) -> dict[str, Any]:
        since = self.suite_started_at.isoformat().replace("+00:00", "Z")
        until = utc_now_iso()
        try:
            response = self.client.get(
                "/api/v1/analytics/dashboard",
                params={"since": since, "until": until},
            )
            response.raise_for_status()
            return {"since": since, "until": until, "payload": response.json().get("data")}
        except Exception as exc:  # noqa: BLE001
            return {"since": since, "until": until, "error": str(exc)}

    def run(self) -> dict[str, Any]:
        self.wait_for_api()
        self.seed()
        prewarm = self.run_cross_service_cache_prewarm()
        self.record_sanity_check(
            "cache-prewarm-api-requests",
            prewarm.get("api_failures") == 0,
            f"api_requests={prewarm.get('api_requests')} api_failures={prewarm.get('api_failures')}",
        )

        self.run_metadata_warmup()
        metadata = self.capture_workload("metadata_api", self.run_metadata_measurement)
        self.record_sanity_check(
            "metadata-workload-success-rate",
            (metadata.get("success_rate") or 0.0) >= 0.99,
            f"success_rate={metadata.get('success_rate')}",
        )
        if self.mode == "cache-on":
            metadata_hits = valkey_delta(metadata, "keyspace_hits")
            metadata_cache_hits = metadata_cache_engagement(metadata)
            self.record_sanity_check(
                "metadata-cache-engagement",
                metadata_hits > 0 or metadata_cache_hits > 0,
                f"keyspace_hits_delta={metadata_hits} metadata_cache_hits={metadata_cache_hits}",
            )

        self.run_execution_warmup()
        executions = self.capture_workload("execution_throughput", self.run_execution_measurement)
        self.record_sanity_check(
            "execution-workload-completions",
            executions.get("completed") == executions.get("submitted"),
            f"completed={executions.get('completed')} submitted={executions.get('submitted')}",
        )
        if self.mode == "cache-on":
            execution_hits = valkey_delta(executions, "keyspace_hits")
            execution_cache_hits = metadata_cache_engagement(executions)
            self.record_sanity_check(
                "execution-cache-engagement",
                execution_hits > 0 or execution_cache_hits > 0,
                f"keyspace_hits_delta={execution_hits} metadata_cache_hits={execution_cache_hits}",
            )

        self.run_hotpath_warmup()
        hotpath = self.capture_workload("hot_path_cache", self.run_hotpath_measurement)
        self.record_sanity_check(
            "hotpath-workload-success-rate",
            (hotpath.get("success_rate") or 0.0) >= 0.99,
            f"success_rate={hotpath.get('success_rate')}",
        )
        if self.mode == "cache-on":
            hotpath_hits = valkey_delta(hotpath, "keyspace_hits")
            hotpath_cache_hits = metadata_cache_engagement(hotpath)
            self.record_sanity_check(
                "hotpath-cache-engagement",
                hotpath_hits > 0 or hotpath_cache_hits > 0,
                f"keyspace_hits_delta={hotpath_hits} metadata_cache_hits={hotpath_cache_hits}",
            )

        self.run_e2e_warmup()
        automation = self.capture_workload("automation_latency", self.run_e2e_measurement)
        self.record_sanity_check(
            "automation-workload-success-rate",
            (automation.get("success_rate") or 0.0) >= 0.99,
            f"success_rate={automation.get('success_rate')}",
        )

        steady_state = None
        if self.settings.steady_state_windows > 0 and self.settings.steady_state_iterations_per_window > 0:
            steady_state = self.capture_workload(
                "steady_state_hot_path",
                self.run_steady_state_hotpath_measurement,
            )
            self.record_sanity_check(
                "steady-state-hotpath-success-rate",
                (steady_state.get("success_rate") or 0.0) >= 0.99,
                f"success_rate={steady_state.get('success_rate')} windows={steady_state.get('window_count')}",
            )

        result = {
            "suite": "metadata-cache-blended-benchmark",
            "mode": self.mode,
            "started_at": self.suite_started_at.isoformat().replace("+00:00", "Z"),
            "completed_at": utc_now_iso(),
            "settings": asdict(self.settings),
            "environment": collect_environment(self.settings.base_url),
            "seed": self.seed_summary,
            "cache_prewarm": prewarm,
            "metadata_api": metadata,
            "execution_throughput": executions,
            "hot_path_cache": hotpath,
            "automation_latency": automation,
            "analytics_dashboard": self.collect_analytics(),
            "sanity_checks": self.sanity_checks,
        }
        if steady_state is not None:
            result["steady_state_hot_path"] = steady_state
        return result


def collect_environment(base_url: str) -> dict[str, Any]:
    def read_stdout(command: Sequence[str]) -> str | None:
        try:
            result = run_command(command, check=False)
        except FileNotFoundError:
            return None
        if result.returncode != 0:
            return None
        return result.stdout.strip() or None

    return {
        "git_revision": read_stdout(["git", "rev-parse", "HEAD"]),
        "git_branch": read_stdout(["git", "rev-parse", "--abbrev-ref", "HEAD"]),
        "hostname": socket.gethostname(),
        "platform": platform.platform(),
        "python": sys.version,
        "docker_version": read_stdout(["docker", "version", "--format", "{{.Server.Version}}"]),
        "docker_compose_version": read_stdout(["docker", "compose", "version", "--short"]),
        "base_url": base_url,
    }


def build_comparison(cache_on: dict[str, Any], cache_off: dict[str, Any]) -> dict[str, Any]:
    metric_specs = [
        ("metadata_api.requests_per_second", ["metadata_api", "requests_per_second"], False),
        ("metadata_api.latency_ms.p95", ["metadata_api", "latency_ms", "p95"], True),
        ("metadata_api.latency_ms.p99", ["metadata_api", "latency_ms", "p99"], True),
        ("execution_throughput.completed_per_second", ["execution_throughput", "completed_per_second"], False),
        ("execution_throughput.schedule_latency_ms.p95", ["execution_throughput", "schedule_latency_ms", "p95"], True),
        ("execution_throughput.end_to_end_latency_ms.p95", ["execution_throughput", "end_to_end_latency_ms", "p95"], True),
        ("hot_path_cache.requests_per_second", ["hot_path_cache", "requests_per_second"], False),
        ("hot_path_cache.latency_ms.p95", ["hot_path_cache", "latency_ms", "p95"], True),
        ("hot_path_cache.latency_ms.p99", ["hot_path_cache", "latency_ms", "p99"], True),
        ("automation_latency.latency_ms.p95", ["automation_latency", "latency_ms", "p95"], True),
        ("automation_latency.latency_ms.p99", ["automation_latency", "latency_ms", "p99"], True),
        ("automation_latency.post_latency_ms.p99", ["automation_latency", "post_latency_ms", "p99"], True),
        ("automation_latency.discovery_latency_ms.p99", ["automation_latency", "discovery_latency_ms", "p99"], True),
        ("automation_latency.terminal_wait_latency_ms.p99", ["automation_latency", "terminal_wait_latency_ms", "p99"], True),
        ("automation_latency.event_to_execution_created_ms.p99", ["automation_latency", "event_to_execution_created_ms", "p99"], True),
        ("automation_latency.execution_internal_ms.p99", ["automation_latency", "execution_internal_ms", "p99"], True),
        ("automation_latency.success_rate", ["automation_latency", "success_rate"], False),
    ]
    if cache_on.get("steady_state_hot_path") or cache_off.get("steady_state_hot_path"):
        metric_specs[9:9] = [
            ("steady_state_hot_path.requests_per_second", ["steady_state_hot_path", "requests_per_second"], False),
            ("steady_state_hot_path.latency_ms.p95", ["steady_state_hot_path", "latency_ms", "p95"], True),
            ("steady_state_hot_path.latency_ms.p99", ["steady_state_hot_path", "latency_ms", "p99"], True),
            (
                "steady_state_hot_path.last_window_latency_ms.p95",
                ["steady_state_hot_path", "last_window_latency_ms", "p95"],
                True,
            ),
            ("steady_state_hot_path.p95_drift_percent", ["steady_state_hot_path", "p95_drift_percent"], True),
        ]
    metrics: dict[str, Any] = {}
    for name, path, lower_is_better in metric_specs:
        metrics[name] = compare_metric(
            deep_get(cache_on, path),
            deep_get(cache_off, path),
            lower_is_better=lower_is_better,
        )
    return {
        "cache_on_mode": cache_on.get("mode"),
        "cache_off_mode": cache_off.get("mode"),
        "generated_at": utc_now_iso(),
        "metrics": metrics,
    }


def format_value(value: Any) -> str:
    if value is None:
        return "n/a"
    if isinstance(value, float):
        return f"{value:.3f}"
    return str(value)


def render_comparison_markdown(cache_on: dict[str, Any], cache_off: dict[str, Any], comparison: dict[str, Any]) -> str:
    lines = [
        "# Metadata cache benchmark comparison",
        "",
        f"Generated: {comparison['generated_at']}",
        "",
        "## Modes",
        "",
        f"- Cache on: `{cache_on.get('mode')}`",
        f"- Cache off: `{cache_off.get('mode')}`",
        f"- Git revision: `{cache_on.get('environment', {}).get('git_revision') or cache_off.get('environment', {}).get('git_revision')}`",
        "",
        "## Workload settings",
        "",
        "```json",
        json.dumps(cache_on.get("settings", {}), indent=2, sort_keys=True),
        "```",
        "",
        "## Metric comparison",
        "",
        "| Metric | Cache on | Cache off | Delta | Delta % | Winner |",
        "| --- | ---: | ---: | ---: | ---: | --- |",
    ]
    for name, values in comparison["metrics"].items():
        delta_percent = values.get("delta_percent")
        delta_percent_text = "n/a" if delta_percent is None else f"{delta_percent:.2f}%"
        lines.append(
            f"| `{name}` | {format_value(values.get('cache_on'))} | {format_value(values.get('cache_off'))} | {format_value(values.get('delta'))} | {delta_percent_text} | {values.get('winner') or 'n/a'} |"
        )

    if deep_get(cache_on, ["execution_throughput", "all_rounds"]) or deep_get(cache_off, ["execution_throughput", "all_rounds"]):
        lines.extend([
            "",
            "## Execution throughput rounds",
            "",
            "Execution throughput comparison uses the median `completed_per_second` round for p95/p99 metrics; all rounds are retained below to expose scheduler/worker outliers.",
            "",
            "| Mode | Selected | Round | Completed/s | Schedule p95 ms | E2E p95 ms | Internal elapsed s | Completed |",
            "| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |",
        ])
        for mode, payload in (("cache-on", cache_on), ("cache-off", cache_off)):
            selected_round = deep_get(payload, ["execution_throughput", "round_index"])
            rounds = deep_get(payload, ["execution_throughput", "all_rounds"]) or []
            for round_payload in rounds:
                round_index = round_payload.get("round_index")
                selected = "yes" if round_index == selected_round else ""
                lines.append(
                    f"| `{mode}` | {selected} | {round_index} | "
                    f"{format_value(round_payload.get('completed_per_second'))} | "
                    f"{format_value(deep_get(round_payload, ['schedule_latency_ms', 'p95']))} | "
                    f"{format_value(deep_get(round_payload, ['end_to_end_latency_ms', 'p95']))} | "
                    f"{format_value(round_payload.get('internal_elapsed_seconds'))} | "
                    f"{format_value(round_payload.get('completed'))} |"
                )

    if deep_get(cache_on, ["automation_latency", "all_rounds"]) or deep_get(cache_off, ["automation_latency", "all_rounds"]):
        lines.extend([
            "",
            "## Automation latency rounds",
            "",
            "Automation latency comparison uses the median latency p95 round for p95/p99 metrics; all rounds are retained below to expose webhook/executor/worker outliers.",
            "",
            "| Mode | Selected | Round | Latency p95 ms | Latency p99 ms | Terminal wait p99 ms | Execution internal p99 ms | Success rate | Samples |",
            "| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |",
        ])
        for mode, payload in (("cache-on", cache_on), ("cache-off", cache_off)):
            selected_round = deep_get(payload, ["automation_latency", "round_index"])
            rounds = deep_get(payload, ["automation_latency", "all_rounds"]) or []
            for round_payload in rounds:
                round_index = round_payload.get("round_index")
                selected = "yes" if round_index == selected_round else ""
                lines.append(
                    f"| `{mode}` | {selected} | {round_index} | "
                    f"{format_value(deep_get(round_payload, ['latency_ms', 'p95']))} | "
                    f"{format_value(deep_get(round_payload, ['latency_ms', 'p99']))} | "
                    f"{format_value(deep_get(round_payload, ['terminal_wait_latency_ms', 'p99']))} | "
                    f"{format_value(deep_get(round_payload, ['execution_internal_ms', 'p99']))} | "
                    f"{format_value(round_payload.get('success_rate'))} | "
                    f"{format_value(round_payload.get('requests_completed'))} |"
                )

    lines.extend([
        "",
        "## Cache engagement",
        "",
        "### Process-local L1",
        "",
        "| Workload | Cache-on JSON hits | Cache-on JSON misses | Cache-on index hits | Cache-on index misses | Cache-off JSON hits | Cache-off JSON misses | Cache-off index hits | Cache-off index misses |",
        "| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |",
    ])
    for workload in CACHE_WORKLOADS:
        cache_on_payload = cache_on.get(workload, {})
        cache_off_payload = cache_off.get(workload, {})
        lines.append(
            f"| `{workload}` | "
            f"{format_value(metadata_cache_delta(cache_on_payload, 'l1_json_hits'))} | "
            f"{format_value(metadata_cache_delta(cache_on_payload, 'l1_json_misses'))} | "
            f"{format_value(metadata_cache_delta(cache_on_payload, 'l1_index_hits'))} | "
            f"{format_value(metadata_cache_delta(cache_on_payload, 'l1_index_misses'))} | "
            f"{format_value(metadata_cache_delta(cache_off_payload, 'l1_json_hits'))} | "
            f"{format_value(metadata_cache_delta(cache_off_payload, 'l1_json_misses'))} | "
            f"{format_value(metadata_cache_delta(cache_off_payload, 'l1_index_hits'))} | "
            f"{format_value(metadata_cache_delta(cache_off_payload, 'l1_index_misses'))} |"
        )

    lines.extend([
        "",
        "### Valkey L2 client counters",
        "",
        "| Workload | Cache-on JSON hits | Cache-on JSON misses | Cache-on index hits | Cache-on index misses | Cache-on fallbacks | Cache-on writes | Cache-on evictions | Cache-on errors | Cache-off fallbacks | Cache-off writes | Cache-off evictions |",
        "| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |",
    ])
    for workload in CACHE_WORKLOADS:
        cache_on_payload = cache_on.get(workload, {})
        cache_off_payload = cache_off.get(workload, {})
        lines.append(
            f"| `{workload}` | "
            f"{format_value(metadata_cache_delta(cache_on_payload, 'l2_json_hits'))} | "
            f"{format_value(metadata_cache_delta(cache_on_payload, 'l2_json_misses'))} | "
            f"{format_value(metadata_cache_delta(cache_on_payload, 'l2_index_hits'))} | "
            f"{format_value(metadata_cache_delta(cache_on_payload, 'l2_index_misses'))} | "
            f"{format_value(metadata_cache_delta(cache_on_payload, 'local_only_fallbacks'))} | "
            f"{format_value(metadata_cache_delta(cache_on_payload, 'writes'))} | "
            f"{format_value(metadata_cache_delta(cache_on_payload, 'evictions'))} | "
            f"{format_value(metadata_cache_delta(cache_on_payload, 'errors'))} | "
            f"{format_value(metadata_cache_delta(cache_off_payload, 'local_only_fallbacks'))} | "
            f"{format_value(metadata_cache_delta(cache_off_payload, 'writes'))} | "
            f"{format_value(metadata_cache_delta(cache_off_payload, 'evictions'))} |"
        )

    lines.extend([
        "",
        "### Redis/Valkey server counters",
        "",
        "| Workload | Cache-on keyspace hits | Cache-on commands | Cache-off keyspace hits | Cache-off commands |",
        "| --- | ---: | ---: | ---: | ---: |",
    ])
    for workload in CACHE_WORKLOADS:
        cache_on_payload = cache_on.get(workload, {})
        cache_off_payload = cache_off.get(workload, {})
        lines.append(
            f"| `{workload}` | {format_value(valkey_delta(cache_on_payload, 'keyspace_hits'))} | "
            f"{format_value(valkey_delta(cache_on_payload, 'total_commands_processed'))} | "
            f"{format_value(valkey_delta(cache_off_payload, 'keyspace_hits'))} | "
            f"{format_value(valkey_delta(cache_off_payload, 'total_commands_processed'))} |"
        )

    lines.extend([
        "",
        "Cache-off is `Postgres + process-local L1`; cache-on adds Valkey as L2. Low Valkey command deltas with non-zero L1 hits usually mean repeated reads were served in-process rather than that the cache was unused.",
    ])

    if cache_on.get("steady_state_hot_path") or cache_off.get("steady_state_hot_path"):
        lines.extend([
            "",
            "## Steady-state hot path windows",
            "",
            "| Mode | Window | RPS | Mean ms | p95 ms | p99 ms | L1 hits | L2 hits | Valkey commands |",
            "| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |",
        ])
        for mode, payload in (("cache-on", cache_on), ("cache-off", cache_off)):
            windows = deep_get(payload, ["steady_state_hot_path", "windows"]) or []
            for window in windows:
                cache_delta = window.get("metadata_cache", {}).get("delta", {})
                valkey = window.get("valkey", {}).get("delta", {})
                l1_hits = sum(
                    cache_delta.get(key, 0)
                    for key in ("l1_json_hits", "l1_index_hits")
                    if isinstance(cache_delta.get(key, 0), (int, float))
                )
                l2_hits = sum(
                    cache_delta.get(key, 0)
                    for key in ("l2_json_hits", "l2_index_hits")
                    if isinstance(cache_delta.get(key, 0), (int, float))
                )
                lines.append(
                    f"| `{mode}` | {window.get('window_index')} | "
                    f"{format_value(window.get('requests_per_second'))} | "
                    f"{format_value(deep_get(window, ['latency_ms', 'mean']))} | "
                    f"{format_value(deep_get(window, ['latency_ms', 'p95']))} | "
                    f"{format_value(deep_get(window, ['latency_ms', 'p99']))} | "
                    f"{format_value(l1_hits)} | {format_value(l2_hits)} | "
                    f"{format_value(valkey.get('total_commands_processed'))} |"
                )

    warnings = [
        ("cache-on", warning)
        for warning in percentile_sample_warnings(cache_on)
    ] + [
        ("cache-off", warning)
        for warning in percentile_sample_warnings(cache_off)
    ]
    if warnings:
        lines.extend([
            "",
            "## Percentile sample-size warnings",
            "",
            "| Mode | Workload metric | Samples | Note |",
            "| --- | --- | ---: | --- |",
        ])
        for mode, warning in warnings:
            lines.append(
                f"| `{mode}` | `{warning['workload']}.{warning['metric']}` | {warning['count']} | {warning['message']} |"
            )

    interpretation_notes = []
    for mode, payload in (("cache-on", cache_on), ("cache-off", cache_off)):
        for note in profile_interpretation_notes(payload):
            row = (mode, note)
            if row not in interpretation_notes:
                interpretation_notes.append(row)
    if interpretation_notes:
        lines.extend([
            "",
            "## Benchmark interpretation notes",
            "",
            "| Mode | Note |",
            "| --- | --- |",
        ])
        for mode, note in interpretation_notes:
            lines.append(f"| `{mode}` | {note} |")

    def render_sanity_table(label: str, payload: dict[str, Any]) -> None:
        lines.extend([
            "",
            f"## {label} sanity checks",
            "",
            "| Check | Passed | Detail |",
            "| --- | --- | --- |",
        ])
        for check in payload.get("sanity_checks", []):
            lines.append(
                f"| `{check['name']}` | {'yes' if check['passed'] else 'no'} | {check['detail']} |"
            )

    render_sanity_table("Cache on", cache_on)
    render_sanity_table("Cache off", cache_off)
    return "\n".join(lines) + "\n"


def command_run(args: argparse.Namespace) -> int:
    settings = build_settings_from_args(args)
    runner = BenchmarkRunner(args.mode, settings)
    result = runner.run()
    output_path = Path(args.output)
    output_path.parent.mkdir(parents=True, exist_ok=True)
    output_path.write_text(json.dumps(result, indent=2, sort_keys=True) + "\n")
    print(f"Wrote benchmark result to {output_path}")
    return 0


def command_compare(args: argparse.Namespace) -> int:
    cache_on = json.loads(Path(args.cache_on).read_text())
    cache_off = json.loads(Path(args.cache_off).read_text())
    comparison = build_comparison(cache_on, cache_off)
    output_path = Path(args.output)
    output_path.parent.mkdir(parents=True, exist_ok=True)
    output_path.write_text(render_comparison_markdown(cache_on, cache_off, comparison))
    if args.json_output:
        json_path = Path(args.json_output)
        json_path.parent.mkdir(parents=True, exist_ok=True)
        json_path.write_text(json.dumps(comparison, indent=2, sort_keys=True) + "\n")
    print(f"Wrote comparison report to {output_path}")
    return 0


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description="Run Attune metadata-cache benchmarks")
    subparsers = parser.add_subparsers(dest="command", required=True)

    run_parser = subparsers.add_parser("run", help="Run a single benchmark mode")
    run_parser.add_argument("--mode", required=True, choices=["cache-on", "cache-off"])
    run_parser.add_argument("--output", required=True)
    run_parser.add_argument("--profile", choices=sorted(PROFILE_OVERRIDES), default="smoke")
    run_parser.add_argument("--base-url")
    run_parser.add_argument("--metadata-warmup-seconds", type=float)
    run_parser.add_argument("--metadata-duration-seconds", type=float)
    run_parser.add_argument("--metadata-concurrency", type=int)
    run_parser.add_argument("--metadata-seed-count", type=int)
    run_parser.add_argument("--execution-warmup-count", type=int)
    run_parser.add_argument("--execution-count", type=int)
    run_parser.add_argument("--execution-concurrency", type=int)
    run_parser.add_argument(
        "--execution-workflow-parent-count",
        type=int,
        help="Number of parent workflow executions to submit for the execution throughput workload",
    )
    run_parser.add_argument(
        "--execution-measurement-rounds",
        type=int,
        help="Number of execution-throughput rounds; the median-throughput round is used for p95/p99 comparisons",
    )
    run_parser.add_argument("--e2e-warmup-iterations", type=int)
    run_parser.add_argument("--e2e-pipelines", type=int)
    run_parser.add_argument("--e2e-iterations", type=int)
    run_parser.add_argument(
        "--e2e-measurement-rounds",
        type=int,
        help="Number of automation-latency rounds; the median latency p95 round is used for p95/p99 comparisons",
    )
    run_parser.add_argument("--poll-interval-seconds", type=float)
    run_parser.add_argument("--e2e-poll-interval-seconds", type=float)
    run_parser.add_argument("--execution-timeout-seconds", type=int)
    run_parser.add_argument("--sampler-interval-seconds", type=float)
    run_parser.add_argument("--hotpath-warmup-iterations", type=int)
    run_parser.add_argument("--hotpath-iterations", type=int)
    run_parser.add_argument("--hotpath-concurrency", type=int)
    run_parser.add_argument("--queue-poll-settle-seconds", type=float)
    run_parser.add_argument("--steady-state-windows", type=int)
    run_parser.add_argument("--steady-state-iterations-per-window", type=int)
    run_parser.add_argument("--steady-state-pause-seconds", type=float)
    run_parser.add_argument("--steady-state-concurrency", type=int)
    run_parser.set_defaults(func=command_run)

    compare_parser = subparsers.add_parser("compare", help="Compare cache-on and cache-off results")
    compare_parser.add_argument("--cache-on", required=True)
    compare_parser.add_argument("--cache-off", required=True)
    compare_parser.add_argument("--output", required=True)
    compare_parser.add_argument("--json-output")
    compare_parser.set_defaults(func=command_compare)
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)
    return args.func(args)


if __name__ == "__main__":
    raise SystemExit(main())
