"""
Polling Utilities for E2E Tests

Provides helper functions for waiting on asynchronous conditions
during end-to-end testing.
"""

import time
import os
from typing import Any, Callable, List, Optional

from requests.models import HTTPError

from .client_wrapper import AttuneClient


DEFAULT_POLL_INTERVAL = float(os.getenv("ATTUNE_E2E_POLL_INTERVAL", "0.25"))


def wait_for_condition(
    condition_fn: Callable[[], bool],
    timeout: float = 30.0,
    poll_interval: float = DEFAULT_POLL_INTERVAL,
    error_message: str = "Condition not met within timeout",
) -> bool:
    """
    Wait for a condition function to return True

    Args:
        condition_fn: Function that returns True when condition is met
        timeout: Maximum time to wait in seconds
        poll_interval: Time between checks in seconds
        error_message: Error message if timeout occurs

    Returns:
        True if condition met

    Raises:
        TimeoutError: If condition not met within timeout
    """
    start_time = time.time()
    elapsed = 0.0

    while elapsed < timeout:
        try:
            if condition_fn():
                return True
        except (UnexpectedTerminalStatusError, KeyboardInterrupt):
            raise  # Never swallow these
        except Exception:
            # Ignore transient exceptions during polling (e.g., 404 errors)
            pass

        time.sleep(poll_interval)
        elapsed = time.time() - start_time

    raise TimeoutError(f"{error_message} (waited {elapsed:.1f}s)")


class UnexpectedTerminalStatusError(Exception):
    """Raised when an execution reaches a terminal status other than the expected one."""

    def __init__(self, execution_id: int, expected: str, actual: str, result: Any = None):
        self.execution_id = execution_id
        self.expected = expected
        self.actual = actual
        self.result = result
        detail = ""
        if isinstance(result, dict):
            detail = f" — {result.get('error', result)}"
        elif result is not None:
            detail = f" — {result}"
        super().__init__(
            f"Execution {execution_id} reached terminal status '{actual}' "
            f"(expected '{expected}'){detail}"
        )


TERMINAL_STATUSES = frozenset([
    "completed", "failed", "cancelled", "timeout", "abandoned",
])


def wait_for_execution_status(
    client: AttuneClient,
    execution_id: int,
    expected_status: str | list[str],
    timeout: float = 30.0,
    poll_interval: float = DEFAULT_POLL_INTERVAL,
) -> dict:
    """
    Wait for execution to reach expected status

    Args:
        client: AttuneClient instance
        execution_id: Execution ID to monitor
        expected_status: Expected status (completed, failed, cancelled, etc)
        timeout: Maximum time to wait in seconds
        poll_interval: Time between status checks

    Returns:
        Final execution object

    Raises:
        UnexpectedTerminalStatusError: If execution reaches a different terminal status
        TimeoutError: If status not reached within timeout
    """
    execution = client.get_execution(execution_id)

    expected_values = expected_status if isinstance(expected_status, list) else [expected_status]
    expected_values = set(expected_values)

    def check_status():
        nonlocal execution
        execution = client.get_execution(execution_id)
        status = execution["status"]
        if status in expected_values:
            return True
        # Bail early if a different terminal status was reached
        if status in TERMINAL_STATUSES:
            raise UnexpectedTerminalStatusError(
                execution_id, str(expected_status), status, execution.get("result")
            )
        return False

    wait_for_condition(
        check_status,
        timeout=timeout,
        poll_interval=poll_interval,
        error_message=f"Execution {execution_id} did not reach status '{expected_status}'",
    )

    return execution


def wait_for_execution_completion(
    client: AttuneClient,
    execution_id: int,
    timeout: float = 30.0,
    poll_interval: float = DEFAULT_POLL_INTERVAL,
) -> dict:
    """
    Wait for execution to complete (reach terminal status)

    Terminal statuses are: completed, failed, cancelled, timeout

    Args:
        client: AttuneClient instance
        execution_id: Execution ID to monitor
        timeout: Maximum time to wait in seconds
        poll_interval: Time between status checks

    Returns:
        Final execution object

    Raises:
        TimeoutError: If execution doesn't complete within timeout
    """
    execution = client.get_execution(execution_id)

    def check_completion():
        nonlocal execution
        execution = client.get_execution(execution_id)
        return execution["status"] in TERMINAL_STATUSES

    wait_for_condition(
        check_completion,
        timeout=timeout,
        poll_interval=poll_interval,
        error_message=f"Execution {execution_id} did not complete",
    )

    return execution


def wait_for_execution_count(
    client: AttuneClient,
    expected_count: int,
    action_ref: Optional[str] = None,
    status: Optional[str] = None,
    enforcement_id: Optional[int] = None,
    rule_id: Optional[int] = None,
    created_after: Optional[str] = None,
    timeout: float = 30.0,
    poll_interval: float = DEFAULT_POLL_INTERVAL,
    operator: str = ">=",
    verbose: bool = False,
) -> List[dict]:
    """
    Wait for execution count to reach threshold

    Args:
        client: AttuneClient instance
        expected_count: Expected number of executions
        action_ref: Optional filter by action reference
        status: Optional filter by status
        enforcement_id: Optional filter by enforcement ID (most precise)
        rule_id: Optional filter by rule ID (via enforcement)
        created_after: Optional ISO timestamp to filter executions created after this time
        timeout: Maximum time to wait
        poll_interval: Time between checks
        operator: Comparison operator (>=, ==, <=, >, <)
        verbose: Print debug information during polling

    Returns:
        List of executions

    Raises:
        TimeoutError: If count not reached within timeout
    """
    executions = []

    def check_count():
        nonlocal executions

        # If rule_id is provided, get executions via enforcements
        if rule_id is not None:
            # Get all enforcements for this rule
            enforcements = client.list_enforcements(rule_id=rule_id, limit=1000)
            if verbose:
                print(
                    f"  [DEBUG] Found {len(enforcements)} enforcements for rule {rule_id}"
                )
            # Get executions for each enforcement
            all_executions = []
            for enf in enforcements:
                enf_executions = client.list_executions(
                    enforcement_id=enf["id"], status=status, limit=1000
                )
                if verbose:
                    print(
                        f"  [DEBUG] Enforcement {enf['id']}: {len(enf_executions)} executions"
                    )
                all_executions.extend(enf_executions)
            executions = all_executions
        elif enforcement_id is not None:
            # Filter by specific enforcement
            executions = client.list_executions(
                enforcement_id=enforcement_id, status=status, limit=1000
            )
            if verbose:
                print(
                    f"  [DEBUG] Found {len(executions)} executions for enforcement {enforcement_id}"
                )
        else:
            # Use action_ref and status filters
            executions = client.list_executions(
                action_ref=action_ref, status=status, limit=1000
            )
            if verbose:
                filter_str = f"action_ref={action_ref}" if action_ref else "all"
                if status:
                    filter_str += f", status={status}"
                print(f"  [DEBUG] Found {len(executions)} executions ({filter_str})")

        # Apply timestamp filter if provided
        if created_after:
            from datetime import datetime

            cutoff = datetime.fromisoformat(created_after.replace("Z", "+00:00"))
            filtered = []
            for exec in executions:
                exec_time = datetime.fromisoformat(
                    exec["created"].replace("Z", "+00:00")
                )
                if exec_time > cutoff:
                    filtered.append(exec)
            if verbose:
                print(
                    f"  [DEBUG] After timestamp filter: {len(filtered)} executions (was {len(executions)})"
                )
            executions = filtered

        actual_count = len(executions)

        if verbose:
            print(f"  [DEBUG] Checking: {actual_count} {operator} {expected_count}")

        if operator == ">=":
            return actual_count >= expected_count
        elif operator == "==":
            return actual_count == expected_count
        elif operator == "<=":
            return actual_count <= expected_count
        elif operator == ">":
            return actual_count > expected_count
        elif operator == "<":
            return actual_count < expected_count
        else:
            raise ValueError(f"Invalid operator: {operator}")

    filter_desc = ""
    if rule_id:
        filter_desc += f" for rule {rule_id}"
    elif enforcement_id:
        filter_desc += f" for enforcement {enforcement_id}"
    elif action_ref:
        filter_desc += f" for action {action_ref}"
    if status:
        filter_desc += f" with status {status}"
    if created_after:
        filter_desc += f" created after {created_after}"

    wait_for_condition(
        check_count,
        timeout=timeout,
        poll_interval=poll_interval,
        error_message=f"Execution count did not reach {operator} {expected_count}{filter_desc}",
    )

    return executions


def wait_for_event_count(
    client: AttuneClient,
    expected_count: int,
    trigger_id: Optional[int] = None,
    trigger_ref: Optional[str] = None,
    rule_id: Optional[int] = None,
    created_after: Optional[str] = None,
    timeout: float = 30.0,
    poll_interval: float = DEFAULT_POLL_INTERVAL,
    operator: str = ">=",
) -> List[dict]:
    """
    Wait for event count to reach threshold

    Args:
        client: AttuneClient instance
        expected_count: Expected number of events
        trigger_id: Optional filter by trigger ID
        trigger_ref: Optional filter by trigger ref (e.g. "core.crontimer")
        rule_id: Optional filter by rule ID (events have a 'rule' field)
        created_after: Optional ISO timestamp - only count events created after this time
        timeout: Maximum time to wait
        poll_interval: Time between checks
        operator: Comparison operator (>=, ==, <=, >, <)

    Returns:
        List of events (filtered)

    Raises:
        TimeoutError: If count not reached within timeout
    """
    events = []

    def check_count():
        nonlocal events
        from datetime import datetime

        page_size = 1000
        page = 1
        cutoff = (
            datetime.fromisoformat(created_after.replace("Z", "+00:00"))
            if created_after
            else None
        )
        filtered = []

        while True:
            page_events = client.list_events(
                trigger_id=trigger_id,
                trigger_ref=trigger_ref,
                limit=page_size,
                page=page,
                enrich=False,
            )
            if not page_events:
                break

            filtered_page = page_events

            if cutoff is not None:
                filtered_page = [
                    e
                    for e in filtered_page
                    if datetime.fromisoformat(e["created"].replace("Z", "+00:00"))
                    >= cutoff
                ]

            if rule_id is not None:
                filtered_page = [e for e in filtered_page if e.get("rule") == rule_id]

            filtered.extend(filtered_page)

            if operator in {">=", ">"}:
                if operator == ">=" and len(filtered) >= expected_count:
                    break
                if operator == ">" and len(filtered) > expected_count:
                    break

            if cutoff is not None:
                oldest_event_time = datetime.fromisoformat(
                    page_events[-1]["created"].replace("Z", "+00:00")
                )
                if oldest_event_time < cutoff:
                    break

            if len(page_events) < page_size:
                break

            page += 1

        events = filtered
        actual_count = len(events)

        if operator == ">=":
            return actual_count >= expected_count
        elif operator == "==":
            return actual_count == expected_count
        elif operator == "<=":
            return actual_count <= expected_count
        elif operator == ">":
            return actual_count > expected_count
        elif operator == "<":
            return actual_count < expected_count
        else:
            raise ValueError(f"Invalid operator: {operator}")

    filter_desc = f" for trigger {trigger_ref or trigger_id}" if (trigger_id or trigger_ref) else ""

    wait_for_condition(
        check_count,
        timeout=timeout,
        poll_interval=poll_interval,
        error_message=f"Event count did not reach {operator} {expected_count}{filter_desc}",
    )

    # Enrich events with full payload now that we have the final set
    enriched = []
    for event in events:
        if event.get("has_payload") and "payload" not in event:
            try:
                full = client.get_event(event["id"])
                enriched.append(full)
            except Exception:
                enriched.append(event)
        else:
            enriched.append(event)

    return enriched


def wait_for_enforcement_count(
    client: AttuneClient,
    expected_count: int,
    rule_id: Optional[int] = None,
    timeout: float = 30.0,
    poll_interval: float = DEFAULT_POLL_INTERVAL,
    operator: str = ">=",
) -> List[dict]:
    """
    Wait for enforcement count to reach threshold

    Args:
        client: AttuneClient instance
        expected_count: Expected number of enforcements
        rule_id: Optional filter by rule ID
        timeout: Maximum time to wait
        poll_interval: Time between checks
        operator: Comparison operator (>=, ==, <=, >, <)

    Returns:
        List of enforcements

    Raises:
        TimeoutError: If count not reached within timeout
    """
    enforcements = []

    def check_count():
        nonlocal enforcements
        enforcements = client.list_enforcements(rule_id=rule_id, limit=1000)
        actual_count = len(enforcements)

        if operator == ">=":
            return actual_count >= expected_count
        elif operator == "==":
            return actual_count == expected_count
        elif operator == "<=":
            return actual_count <= expected_count
        elif operator == ">":
            return actual_count > expected_count
        elif operator == "<":
            return actual_count < expected_count
        else:
            raise ValueError(f"Invalid operator: {operator}")

    filter_desc = f" for rule {rule_id}" if rule_id else ""

    wait_for_condition(
        check_count,
        timeout=timeout,
        poll_interval=poll_interval,
        error_message=f"Enforcement count did not reach {operator} {expected_count}{filter_desc}",
    )

    return enforcements


def wait_for_inquiry_status(
    client: AttuneClient,
    inquiry_id: int,
    expected_status: str,
    timeout: float = 30.0,
    poll_interval: float = DEFAULT_POLL_INTERVAL,
) -> dict:
    """
    Wait for inquiry to reach expected status

    Args:
        client: AttuneClient instance
        inquiry_id: Inquiry ID to monitor
        expected_status: Expected status (pending, responded, expired)
        timeout: Maximum time to wait
        poll_interval: Time between checks

    Returns:
        Final inquiry object

    Raises:
        TimeoutError: If status not reached within timeout
    """
    inquiry = client.get_inquiry(inquiry_id)

    def check_status():
        nonlocal inquiry
        inquiry = client.get_inquiry(inquiry_id)
        return inquiry["status"] == expected_status

    wait_for_condition(
        check_status,
        timeout=timeout,
        poll_interval=poll_interval,
        error_message=f"Inquiry {inquiry_id} did not reach status '{expected_status}'",
    )

    return inquiry


def wait_for_inquiry_count(
    client: AttuneClient,
    expected_count: int,
    status: Optional[str] = None,
    timeout: float = 30.0,
    poll_interval: float = DEFAULT_POLL_INTERVAL,
    operator: str = ">=",
) -> List[dict]:
    """
    Wait for inquiry count to reach expected value

    Args:
        client: AttuneClient instance
        expected_count: Expected number of inquiries
        status: Optional status filter (pending, responded, expired, etc)
        timeout: Maximum time to wait
        poll_interval: Time between checks
        operator: Comparison operator (>=, ==, <=, >, <)

    Returns:
        List of inquiries matching criteria

    Raises:
        TimeoutError: If count not reached within timeout
    """
    inquiries = []

    def check_count():
        nonlocal inquiries
        try:
            response = client.get("/inquiries")
        except HTTPError:
            return False

        inquiries = response.get("data", [])

        # Filter by status if specified
        if status:
            inquiries = [i for i in inquiries if i.get("status") == status]

        actual_count = len(inquiries)

        # Check count based on operator
        if operator == "==":
            return actual_count == expected_count
        elif operator == ">=":
            return actual_count >= expected_count
        elif operator == "<=":
            return actual_count <= expected_count
        elif operator == ">":
            return actual_count > expected_count
        elif operator == "<":
            return actual_count < expected_count
        else:
            raise ValueError(f"Invalid operator: {operator}")

    filter_desc = f" with status {status}" if status else ""

    wait_for_condition(
        check_count,
        timeout=timeout,
        poll_interval=poll_interval,
        error_message=f"Inquiry count did not reach {operator} {expected_count}{filter_desc}",
    )

    return inquiries
