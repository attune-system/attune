"""
Test Helpers for Attune E2E Tests

This module provides utilities for writing end-to-end tests:
- AttuneClient: High-level API client
- wait_for_*: Polling utilities
- create_*: Resource creation helpers
- Fixtures and test data generators
"""

from .client_wrapper import AttuneClient
from .fixtures import (
    create_cron_timer,
    create_date_timer,
    create_echo_action,
    create_failing_action,
    create_interval_timer,
    create_rule,
    create_simple_action,
    create_sleep_action,
    create_test_pack,
    create_timer_automation,
    create_webhook_automation,
    create_webhook_trigger,
    timestamp_future,
    timestamp_now,
    unique_ref,
)
from .inquiries import InquiryRun, start_inquiry_workflow, workflow_task_children
from .polling import (
    wait_for_condition,
    wait_for_enforcement_count,
    wait_for_event_count,
    wait_for_execution_completion,
    wait_for_execution_count,
    wait_for_execution_status,
    wait_for_inquiry_count,
    wait_for_inquiry_status,
)

__all__ = [
    # Client
    "AttuneClient",
    # Polling utilities
    "wait_for_condition",
    "wait_for_enforcement_count",
    "wait_for_event_count",
    "wait_for_execution_completion",
    "wait_for_execution_count",
    "wait_for_execution_status",
    "wait_for_inquiry_count",
    "wait_for_inquiry_status",
    # Inquiry workflow helpers
    "InquiryRun",
    "start_inquiry_workflow",
    "workflow_task_children",
    # Fixture creators
    "create_test_pack",
    "create_interval_timer",
    "create_date_timer",
    "create_cron_timer",
    "create_webhook_trigger",
    "create_simple_action",
    "create_echo_action",
    "create_failing_action",
    "create_sleep_action",
    "create_timer_automation",
    "create_webhook_automation",
    "create_rule",
    "unique_ref",
    "timestamp_now",
    "timestamp_future",
]
