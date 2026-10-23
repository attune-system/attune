#!/usr/bin/env python3
"""Forecast retention and backlog bytes from explicit sanitized observations.

Examples:
  python3 scripts/artifact_read_capacity.py --input observations.json --output forecast.json
  cat observations.json | python3 scripts/artifact_read_capacity.py --input -

Read-only arithmetic: no database connection or live configuration changes.
Exit 0 means the modeled retention plus backlog fits the supplied reserve policy;
1 means it does not fit; 2 means input or output validation failed. Neither result
certifies that the measurements represent a deployment.
"""
import argparse
from fractions import Fraction
import json
import math
from pathlib import Path
import sys


def integer(record, field, minimum=0):
    value = record.get(field)
    if type(value) is not int or not minimum <= value <= 2**63 - 1:
        raise ValueError(f'{field} must be an integer between {minimum} and BIGINT maximum')
    return value


def ratio(record, field, minimum, maximum=None):
    value = record.get(field)
    if type(value) not in (int, float) or not math.isfinite(value) or value < minimum:
        raise ValueError(f'{field} must be finite and at least {minimum}')
    if maximum is not None and value >= maximum:
        raise ValueError(f'{field} must be less than {maximum}')
    return Fraction(str(value))


def forecast(observations):
    if not isinstance(observations, dict):
        raise ValueError('observations must be a JSON object')
    capacity = integer(observations, 'volume_capacity_bytes', 1)
    other = integer(observations, 'unmodeled_used_bytes')
    interval = integer(observations, 'sample_seconds', 1)
    reserve = ratio(observations, 'reserve_fraction', 0, 1)
    safety = ratio(observations, 'storage_safety_factor', 1)
    cohorts = observations.get('cohorts')
    if not isinstance(cohorts, list) or not 1 <= len(cohorts) <= 1000:
        raise ValueError('cohorts must contain between 1 and 1000 nonoverlapping storage cohorts')
    seen = set()
    result = []
    for cohort in cohorts:
        if not isinstance(cohort, dict):
            raise ValueError('each cohort must be a JSON object')
        name = cohort.get('name')
        if not isinstance(name, str) or not name.strip() or name in seen:
            raise ValueError('cohort names must be nonempty and unique')
        seen.add(name)
        inserted = integer(cohort, 'inserted_rows')
        sampled = integer(cohort, 'sampled_rows', 1)
        storage = integer(cohort, 'sampled_total_bytes', 1)
        retention = integer(cohort, 'retention_seconds', 1)
        backlog = integer(cohort, 'backlog_seconds')
        rate = Fraction(inserted, interval)
        rows = math.ceil(rate * retention)
        backlog_rows = math.ceil(rate * (retention + backlog))
        per_row = Fraction(storage, sampled)
        result.append(dict(name=name, observed_rows_per_second=float(rate),
            sampled_bytes_per_row=float(per_row), retention_rows=rows,
            retention_and_backlog_rows=backlog_rows,
            retention_bytes=math.ceil(rows * per_row * safety),
            retention_and_backlog_bytes=math.ceil(backlog_rows * per_row * safety)))
    retained = sum(cohort['retention_bytes'] for cohort in result) + other
    backlogged = sum(cohort['retention_and_backlog_bytes'] for cohort in result) + other
    reserve_bytes = math.ceil(capacity * reserve)
    usable = capacity - reserve_bytes
    return dict(status='forecast-only-not-deployment-certification', cohorts=result,
        volume_capacity_bytes=capacity, unmodeled_used_bytes=other, reserve_bytes=reserve_bytes,
        modeled_retention_used_bytes=retained, modeled_retention_and_backlog_used_bytes=backlogged,
        growth_headroom_after_reserve_bytes=usable - backlogged,
        required_capacity_at_supplied_reserve_bytes=math.ceil(Fraction(backlogged, 1) / (1 - reserve)),
        fits_without_backlog=retained <= usable, fits_with_backlog=backlogged <= usable,
        assumptions=[
            'Observed insertion rate and storage sample represent future workload and row-size distribution.',
            'Each cohort includes table, index, and TOAST bytes exactly once; cohorts do not overlap.',
            'Unmodeled bytes include WAL, other databases, backups, and all other uses of this volume.',
            'Retention is modeled as time at the supplied insertion rate; version-count policies need an equivalent measured horizon.',
            'Safety and reserve fractions are supplied policy, not inferred deployment recommendations.',
        ])


def main(argv=None):
    cli = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    cli.add_argument('--input', required=True, help='sanitized observations JSON, or - for stdin')
    cli.add_argument('--output', type=Path, help='unused output file; defaults to stdout')
    args = cli.parse_args(argv)
    try:
        if args.output and (args.output.exists() or args.output.is_symlink()):
            raise ValueError('output already exists; choose an unused file')
        text = sys.stdin.read() if args.input == '-' else Path(args.input).read_text()
        result = forecast(json.loads(text))
        document = json.dumps(result, indent=2, allow_nan=False) + '\n'
        if args.output:
            with args.output.open('x') as output:
                output.write(document)
        else:
            print(document, end='')
        return 0 if result['fits_with_backlog'] else 1
    except (OSError, ValueError, TypeError) as error:
        cli.exit(2, f'capacity forecast failed: {error}\n')


if __name__ == '__main__':
    sys.exit(main())
