"""Inventory Timescale DDL in Attune's canonical migrations, without a database.

Run from any directory with: python3 docs/research/audit_timescaledb.py
This is a source inventory, not an inspection of deployed database state.
"""

import json
import re
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
PATTERNS = {
    "hypertables": r"SELECT\s+create_hypertable\(\s*'([^']+)'\s*,\s*'([^']+)'\s*,\s*chunk_time_interval\s*=>\s*INTERVAL\s*'([^']+)'",
    "compression_policies": r"SELECT\s+add_compression_policy\(\s*'([^']+)'\s*,\s*INTERVAL\s*'([^']+)'",
    "continuous_aggregates": r"CREATE\s+MATERIALIZED\s+VIEW\s+(\w+)\s+WITH\s*\(timescaledb\.continuous\)",
    "aggregate_refresh_policies": r"SELECT\s+add_continuous_aggregate_policy\(\s*'([^']+)'\s*,\s*start_offset\s*=>\s*INTERVAL\s*'([^']+)'\s*,\s*end_offset\s*=>\s*INTERVAL\s*'([^']+)'\s*,\s*schedule_interval\s*=>\s*INTERVAL\s*'([^']+)'",
    "ordinary_hourly_views": r"CREATE\s+VIEW\s+(\w+_hourly)\s+AS",
    "timescale_retention_policies": r"SELECT\s+add_retention_policy\(\s*'([^']+)'",
}


def inventory():
    results = {name: [] for name in PATTERNS}
    for path in sorted((ROOT / "migrations").glob("*.sql")):
        # Keep line numbers stable. Current migration DDL uses -- comments.
        sql = re.sub(r"--[^\n]*", "", path.read_text())
        for name, pattern in PATTERNS.items():
            for match in re.finditer(pattern, sql, re.IGNORECASE):
                results[name].append(
                    {
                        "values": list(match.groups()),
                        "source": f"{path.relative_to(ROOT)}:{sql.count(chr(10), 0, match.start()) + 1}",
                    }
                )
    return {
        "counts": {name: len(items) for name, items in results.items()},
        "objects": results,
    }


if __name__ == "__main__":
    print(json.dumps(inventory(), indent=2))
