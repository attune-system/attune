"""Numeric reports for the owned artifact benchmark. No database connections."""

import math


def query_counts(queries):
    """Separate observed infrastructure costs without discarding raw totals."""
    total = queries['statements']
    categories = queries.get('statement_categories', {})
    excluded = ('driver_type_discovery', 'driver_enum_discovery',
                'driver_type_name_discovery', 'pool_session_setup')
    counts = [categories.get(category, 0) for category in excluded]
    if any(type(count) is not int or count < 0 for count in [total, *counts]):
        raise ValueError('query counts must be nonnegative integers')
    if sum(counts) > total:
        raise ValueError('infrastructure query counts exceed total statements')
    return dict(total=total, application=total - sum(counts),
                driver_metadata=sum(counts[:-1]), pool_setup=counts[-1])


def percentile(values, quantile):
    """Nearest-rank percentile, retaining errors/cancellations in the caller's report."""
    if not values or not 0 < quantile <= 1:
        raise ValueError("a nonempty sample and quantile in (0, 1] are required")
    if any(not math.isfinite(value) or value < 0 for value in values):
        raise ValueError("timing samples must be finite and nonnegative")
    return sorted(values)[math.ceil(len(values) * quantile) - 1]


def summarize_explain(document):
    """Keep per-node work and root-inclusive buffers without double counting."""
    entry = document[0]
    root = entry["Plan"]
    nodes = []

    def visit(node):
        fields = (
            "Node Type", "Relation Name", "Index Name", "Actual Rows", "Actual Loops",
            "Rows Removed by Filter", "Rows Removed by Join Filter", "Heap Fetches",
            "Shared Hit Blocks", "Shared Read Blocks", "Temp Read Blocks", "Temp Written Blocks",
        )
        nodes.append({field: node[field] for field in fields if field in node})
        for child in node.get("Plans", []):
            visit(child)

    visit(root)
    return {
        "planning_milliseconds": entry["Planning Time"],
        "execution_milliseconds": entry["Execution Time"],
        "returned_rows": root.get("Actual Rows", 0) * root.get("Actual Loops", 1),
        "root_buffers_including_descendants": {
            field: root.get(field, 0) for field in (
                "Shared Hit Blocks", "Shared Read Blocks", "Temp Read Blocks", "Temp Written Blocks"
            )
        },
        "event_relations_executed": sorted({
            node["Relation Name"] for node in nodes
            if node.get("Relation Name", "").startswith(("event_p", "event_default"))
            and node.get("Actual Loops", 0) > 0
        }),
        "nodes": nodes,
    }


def compare_latency(baseline, treatment):
    """Reject unequal workloads rather than report an unsupported percentage."""
    required = ("fixture", "permissions", "cache_state", "concurrency", "completed_requests", "server_settings")
    mismatches = [key for key in required if key not in baseline or baseline[key] != treatment.get(key)]
    if mismatches:
        return {"verified": False, "reason": "workload mismatch", "fields": mismatches}
    if not baseline.get("source_unchanged") or not treatment.get("source_unchanged"):
        return {"verified": False, "reason": "source changed during a run"}
    if baseline.get("errors", 0) or treatment.get("errors", 0) or baseline.get("cancelled", 0) or treatment.get("cancelled", 0):
        return {"verified": False, "reason": "errors or cancelled requests"}
    count = baseline["completed_requests"]
    if count < 20 or len(baseline.get("seconds", [])) != count or len(treatment.get("seconds", [])) != count:
        return {"verified": False, "reason": "at least 20 complete samples per case are required"}
    before = percentile(baseline["seconds"], .95)
    after = percentile(treatment["seconds"], .95)
    if not before:
        return {"verified": False, "reason": "zero baseline p95"}
    reduction = 1 - after / before
    return {"verified": True, "baseline_p95_seconds": before, "treatment_p95_seconds": after,
            "reduction_fraction": reduction, "target_reduction_fraction": .70, "target_met": reduction >= .70}


def compare_paired_reference(report, source_unchanged):
    """Apply declared gates to one paired fixture, never to historical builds."""
    if report.get('comparison_kind') != 'controlled-serial-reference-not-historical-revision':
        return {'passed': False, 'reason': 'unknown comparison kind'}
    if report.get('identical_response_bodies') is not True:
        return {'passed': False, 'reason': 'response equivalence was not established'}
    if report.get('query_reduction_target') != .90 or report.get('p95_reduction_target') != .70:
        return {'passed': False, 'reason': 'predeclared targets do not match'}
    baseline = report.get('baseline', [])
    treatment = report.get('treatment', [])
    if report.get('completed_requests') != 20 or len(baseline) != 20 or len(treatment) != 20:
        return {'passed': False, 'reason': 'twenty complete samples per reader are required'}
    if any(sample.get('status') != 200 for sample in baseline + treatment):
        return {'passed': False, 'reason': 'a paired request failed'}
    metadata = {key: report[key] for key in ('fixture','permissions','cache_state','concurrency','completed_requests')}
    # Both readers execute in the same process and owned database, alternately.
    metadata['server_settings'] = 'same paired server'
    metadata['source_unchanged'] = source_unchanged
    latency = compare_latency({**metadata, 'seconds':[sample['handler_seconds'] for sample in baseline]},
                              {**metadata, 'seconds':[sample['handler_seconds'] for sample in treatment]})
    baseline_counts = [query_counts(sample['queries']) for sample in baseline]
    treatment_counts = [query_counts(sample['queries']) for sample in treatment]
    before = sum(sample['application'] for sample in baseline_counts)
    after = sum(sample['application'] for sample in treatment_counts)
    before_total = sum(sample['total'] for sample in baseline_counts)
    after_total = sum(sample['total'] for sample in treatment_counts)
    reduction = 1 - after / before if before else 0
    reference_red = all(sample['application'] > 20 for sample in baseline_counts)
    batched_ceiling = all(sample['application'] <= 20 for sample in treatment_counts)
    return {
        'passed': latency.get('verified', False) and latency.get('target_met', False)
                  and reduction >= .90 and reference_red and batched_ceiling,
        'comparison_kind':report['comparison_kind'],
        'latency':latency,
        'query_reduction_fraction':reduction,
        'query_reduction_target':.90,
        'reference_exceeds_20_statement_ceiling':reference_red,
        'batched_within_20_statement_ceiling':batched_ceiling,
        'baseline_statements':before_total, 'treatment_statements':after_total,
        'baseline_application_statements':before, 'treatment_application_statements':after,
        'total_statement_reduction_fraction':1 - after_total / before_total if before_total else 0,
        'query_budget_basis':'application statements, excluding driver discovery and pool session setup',
        'historical_or_deployed_speedup_verified':False,
    }


def summarize_samples(samples):
    if not samples:
        raise ValueError("no completed request samples")
    return {
        "completed_requests": len(samples),
        "errors": sum(sample["status"] != 200 for sample in samples),
        "p50_handler_seconds": percentile([sample["handler_seconds"] for sample in samples], .5),
        "p95_handler_seconds": percentile([sample["handler_seconds"] for sample in samples], .95),
        "p95_completion_seconds": percentile([sample["completion_seconds"] for sample in samples], .95),
        "response_bytes": sum(sample["response_bytes"] for sample in samples),
        "handler_poll_cpu_seconds": sum(sample.get("handler_poll_cpu_seconds") or 0 for sample in samples),
        "disclosure_poll_cpu_seconds": sum(sample["queries"].get("execution_disclosure_poll_cpu_seconds") or 0 for sample in samples),
        "pool_acquire_seconds": sum(sample["queries"]["pool_acquire_seconds"] for sample in samples),
        "pool_acquisitions": sum(sample["queries"]["pool_acquisitions"] for sample in samples),
        "statements": sum(sample["queries"]["statements"] for sample in samples),
        "application_statements": sum(query_counts(sample["queries"])["application"] for sample in samples),
        "driver_metadata_statements": sum(query_counts(sample["queries"])["driver_metadata"] for sample in samples),
        "pool_setup_statements": sum(query_counts(sample["queries"])["pool_setup"] for sample in samples),
        "returned_rows": sum(sample["queries"]["returned_rows"] for sample in samples),
        "cpu_counter_available": all(sample.get("handler_poll_cpu_seconds") is not None for sample in samples),
        "disclosure_cpu_counter_available": all(sample["queries"].get("execution_disclosure_poll_cpu_seconds") is not None for sample in samples),
    }


def summarize_workload(profile):
    def lsn(value):
        high, low = value.split('/')
        return (int(high, 16) << 32) + int(low, 16)

    before = profile['capacity_before_churn']
    after = profile['capacity_after_churn']
    return {
        "partition_cases": [
            {"daily_partitions": item['daily_partitions'],
             "first_after_fixture_ddl": summarize_samples([item['first_after_fixture_ddl']]),
             "warm": summarize_samples(item['warm_samples']),
             "prepared_event_plan": summarize_explain(item['prepared_event_plan']['plan'])}
            for item in profile['partition_profiles']
        ],
        "concurrent_readers": summarize_samples(profile['concurrent_samples']),
        "graph_cases": [{"shared_ancestry":item['shared_ancestry'], "ancestor_depth":item['ancestor_depth'],
                         "source_executions":item['source_executions'], "requests":summarize_samples(item['samples']),
                         "lineage_plan":summarize_explain(item['lineage_plan']), "explain_scope":item['explain_scope']}
                        for item in profile.get('graph_profiles', [])],
        "maintenance": profile['maintenance'],
        "owned_server_wal_bytes_during_churn": lsn(after['owned_server']['wal_position']) - lsn(before['owned_server']['wal_position']),
        "measured_relation_bytes_before_churn": sum(item['total_bytes'] for item in before['relations']),
        "measured_relation_bytes_after_churn": sum(item['total_bytes'] for item in after['relations']),
        "remaining_expired_execution_rows": after['exact_fixture_counts']['expired_terminal_executions'],
        "audit_rows_added_during_churn_including_http_audit": after['exact_fixture_counts']['audit_event'] - before['exact_fixture_counts']['audit_event'],
        "post_churn_event_plan": summarize_explain(profile['post_churn_event_plan_without_forced_analyze']),
        "limitations": profile['limitations'],
    }


def summarize_cardinality(profile):
    return {
        "history_cases": [{"retained_executions":item['retained_executions'],
            "runtime_log_artifacts":item['runtime_log_artifacts'],
            "first_after_authorization_cache_invalidation":summarize_samples([item['first_after_authorization_cache_invalidation']]),
            "warm":summarize_samples(item['warm_samples']),
            "four_clients":summarize_samples(item['four_client_samples'])} for item in profile['history_cases']],
        "cardinality_cases": [{"artifacts":item['artifacts'], "per_page":item['per_page'],
            "large_progress_bytes":item['large_progress_bytes'],
            "warm":summarize_samples(item['warm_samples'])} for item in profile['cardinality_cases']],
        "limitations":profile['limitations'],
    }
