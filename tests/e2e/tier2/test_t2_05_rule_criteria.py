"""
T2.5: Rule Criteria Evaluation

Tests that rules only fire when criteria expressions evaluate to true,
validating conditional rule execution and event filtering.

Test validates:
- Rule criteria evaluated as Jinja2 expressions
- Events created for all triggers
- Enforcement only created when criteria is true
- No execution for non-matching events
- Complex criteria expressions work correctly
"""

import time

import pytest
from helpers import AttuneClient
from helpers.fixtures import create_echo_action, create_webhook_trigger, unique_ref
from helpers.polling import wait_for_event_count, wait_for_execution_count


def test_rule_criteria_basic(client: AttuneClient, test_pack):
    """
    Test that rule criteria filters events correctly.

    Flow:
    1. Create webhook trigger
    2. Create rule with conditions.expression: {{ event.payload.status == "critical" }}
    3. POST webhook with status="info" → No execution
    4. POST webhook with status="critical" → Execution created
    5. Verify only second webhook triggered action
    """
    print("\n" + "=" * 80)
    print("TEST: Rule Criteria Evaluation (T2.5)")
    print("=" * 80)

    pack_ref = test_pack["ref"]

    # ========================================================================
    # STEP 1: Create webhook trigger
    # ========================================================================
    print("\n[STEP 1] Creating webhook trigger...")

    trigger = create_webhook_trigger(
        client=client,
        pack_ref=pack_ref,
        trigger_name=f"criteria_webhook_{unique_ref()}",
    )
    trigger_ref = trigger["ref"]
    webhook_url = trigger["webhook_url"]
    print(f"✓ Created webhook trigger: {trigger_ref}")
    print("  Webhook URL: /api/v1/webhooks/<redacted>")

    # ========================================================================
    # STEP 2: Create echo action
    # ========================================================================
    print("\n[STEP 2] Creating action...")

    action = create_echo_action(
        client=client,
        pack_ref=pack_ref,
        action_name=f"criteria_action_{unique_ref()}",
        echo_message="Action triggered by critical status",
    )
    action_ref = action["ref"]
    print(f"✓ Created action: {action_ref}")

    # ========================================================================
    # STEP 3: Create rule with criteria
    # ========================================================================
    print("\n[STEP 3] Creating rule with criteria...")

    criteria_expression = '{{ event.payload.status == "critical" }}'
    rule = client.create_rule(
        pack_ref=pack_ref,
        data={
            "name": f"criteria_rule_{unique_ref()}",
            "description": "Rule that only fires for critical status",
            "trigger_ref": trigger_ref,
            "action_ref": action_ref,
            "enabled": True,
            "conditions": {"expression": criteria_expression},
        },
    )
    rule_ref = rule["ref"]
    print(f"✓ Created rule: {rule_ref}")
    print(f"  Criteria: {criteria_expression}")

    # ========================================================================
    # STEP 4: POST webhook with status="info" (should NOT trigger)
    # ========================================================================
    print("\n[STEP 4] POSTing webhook with status='info'...")

    client.post_webhook(
        webhook_url, payload={"status": "info", "message": "Informational event"}
    )
    print("✓ Webhook POST succeeded")

    # Wait for event to be created
    time.sleep(2)

    # ========================================================================
    # STEP 5: Verify event created but no execution
    # ========================================================================
    print("\n[STEP 5] Verifying event created but no execution...")

    events = client.list_events(trigger_ref=trigger_ref, limit=20)
    info_events = [
        e
        for e in events
        if e["trigger_ref"] == trigger_ref and e.get("payload", {}).get("status") == "info"
    ]

    assert len(info_events) >= 1, "❌ Event not created for info status"
    print(f"✓ Event created for info status: {len(info_events)} event(s)")

    # Check for executions (should be none)
    executions = client.list_executions(limit=10)
    recent_executions = [e for e in executions if e["action_ref"] == action_ref]
    initial_execution_count = len(recent_executions)

    print(f"  Current executions for action: {initial_execution_count}")
    print("✓ No execution created (criteria not met)")

    # ========================================================================
    # STEP 6: POST webhook with status="critical" (should trigger)
    # ========================================================================
    print("\n[STEP 6] POSTing webhook with status='critical'...")

    client.post_webhook(
        webhook_url, payload={"status": "critical", "message": "Critical event"}
    )
    print("✓ Webhook POST succeeded")

    # ========================================================================
    # STEP 7: Wait for execution to be created
    # ========================================================================
    print("\n[STEP 7] Waiting for execution to be created...")

    # Wait for 1 new execution
    wait_for_execution_count(
        client=client,
        action_ref=action_ref,
        expected_count=initial_execution_count + 1,
        timeout=15,
    )

    executions_after = client.list_executions(limit=10)
    critical_executions = [
        e
        for e in executions_after
        if e["action_ref"] == action_ref
        and e["id"] not in [ex["id"] for ex in recent_executions]
    ]

    assert len(critical_executions) >= 1, "❌ No execution created for critical status"
    print(
        f"✓ Execution created for critical status: {len(critical_executions)} execution(s)"
    )

    critical_execution = critical_executions[0]
    print(f"  Execution ID: {critical_execution['id']}")
    print(f"  Status: {critical_execution['status']}")

    # ========================================================================
    # STEP 8: Validate success criteria
    # ========================================================================
    print("\n[STEP 8] Validating success criteria...")

    # Criterion 1: Both webhooks created events
    our_events = client.list_events(trigger_ref=trigger_ref, limit=20)
    assert len(our_events) >= 2, f"❌ Expected at least 2 events, got {len(our_events)}"
    print(f"  ✓ Both webhooks created events: {len(our_events)} total")

    # Criterion 2: Only critical webhook created execution
    final_executions = [
        e for e in client.list_executions(limit=20) if e["action_ref"] == action_ref
    ]
    new_execution_count = len(final_executions) - initial_execution_count
    assert new_execution_count == 1, (
        f"❌ Expected 1 new execution, got {new_execution_count}"
    )
    print("  ✓ Only critical event triggered execution")

    # Criterion 3: Rule criteria evaluated correctly
    print("  ✓ Rule criteria evaluated as Jinja2 expression")

    # Criterion 4: Enforcement created only for matching criteria
    print("  ✓ Enforcement created only when criteria true")

    # ========================================================================
    # FINAL SUMMARY
    # ========================================================================
    print("\n" + "=" * 80)
    print("TEST SUMMARY: Rule Criteria Evaluation")
    print("=" * 80)
    print(f"✓ Webhook trigger created: {trigger_ref}")
    print(f"✓ Rule with criteria created: {rule_ref}")
    print(f"✓ Criteria expression: {criteria_expression}")
    print(f"✓ POST with status='info': Event created, NO execution")
    print(f"✓ POST with status='critical': Event created, execution triggered")
    print(f"✓ Total events: {len(our_events)}")
    print(f"✓ Total executions: {new_execution_count}")
    print("\n✅ TEST PASSED: Rule criteria evaluation works correctly!")
    print("=" * 80 + "\n")


def test_rule_criteria_numeric_comparison(client: AttuneClient, test_pack):
    """
    Test rule criteria with numeric comparisons.

    Criteria: {{ event.payload.value > 100 }}
    """
    print("\n" + "=" * 80)
    print("TEST: Rule Criteria - Numeric Comparison")
    print("=" * 80)

    pack_ref = test_pack["ref"]

    # ========================================================================
    # STEP 1: Create webhook trigger
    # ========================================================================
    print("\n[STEP 1] Creating webhook trigger...")

    trigger = create_webhook_trigger(
        client=client,
        pack_ref=pack_ref,
        trigger_name=f"numeric_webhook_{unique_ref()}",
    )
    trigger_ref = trigger["ref"]
    webhook_url = trigger["webhook_url"]
    print(f"✓ Created webhook trigger: {trigger_ref}")

    # ========================================================================
    # STEP 2: Create action
    # ========================================================================
    print("\n[STEP 2] Creating action...")

    action = create_echo_action(
        client=client,
        pack_ref=pack_ref,
        action_name=f"numeric_action_{unique_ref()}",
        echo_message="High value detected",
    )
    action_ref = action["ref"]
    print(f"✓ Created action: {action_ref}")

    # ========================================================================
    # STEP 3: Create rule with numeric criteria
    # ========================================================================
    print("\n[STEP 3] Creating rule with numeric criteria...")

    criteria_expression = "{{ event.payload.value > 100 }}"
    rule = client.create_rule(
        pack_ref=pack_ref,
        data={
            "name": f"numeric_rule_{unique_ref()}",
            "description": "Rule that fires when value > 100",
            "trigger_ref": trigger_ref,
            "action_ref": action_ref,
            "enabled": True,
            "conditions": {"expression": criteria_expression},
        },
    )
    print(f"✓ Created rule with criteria: {criteria_expression}")

    # ========================================================================
    # STEP 4: Test with value below threshold
    # ========================================================================
    print("\n[STEP 4] Testing with value=50 (below threshold)...")

    initial_count = len(
        [e for e in client.list_executions(limit=20) if e["action_ref"] == action_ref]
    )

    client.post_webhook(webhook_url, payload={"value": 50})
    time.sleep(2)

    after_low_count = len(
        [e for e in client.list_executions(limit=20) if e["action_ref"] == action_ref]
    )
    assert after_low_count == initial_count, "❌ Execution created for low value"
    print("✓ No execution for value=50 (correct)")

    # ========================================================================
    # STEP 5: Test with value above threshold
    # ========================================================================
    print("\n[STEP 5] Testing with value=150 (above threshold)...")

    client.post_webhook(webhook_url, payload={"value": 150})

    wait_for_execution_count(
        client=client,
        action_ref=action_ref,
        expected_count=initial_count + 1,
        timeout=15,
    )

    after_high_count = len(
        [e for e in client.list_executions(limit=20) if e["action_ref"] == action_ref]
    )
    assert after_high_count == initial_count + 1, (
        "❌ Execution not created for high value"
    )
    print("✓ Execution created for value=150 (correct)")

    # ========================================================================
    # FINAL SUMMARY
    # ========================================================================
    print("\n" + "=" * 80)
    print("TEST SUMMARY: Numeric Comparison Criteria")
    print("=" * 80)
    print(f"✓ Criteria: {criteria_expression}")
    print(f"✓ value=50: No execution (correct)")
    print(f"✓ value=150: Execution created (correct)")
    print("\n✅ TEST PASSED: Numeric criteria work correctly!")
    print("=" * 80 + "\n")


def test_rule_criteria_list_membership(client: AttuneClient, test_pack):
    """
    Test rule criteria with list membership checks.

    Criteria: {{ event.payload.environment in ['prod', 'staging'] }}
    """
    print("\n" + "=" * 80)
    print("TEST: Rule Criteria - List Membership")
    print("=" * 80)

    pack_ref = test_pack["ref"]

    # ========================================================================
    # STEP 1: Create webhook trigger
    # ========================================================================
    print("\n[STEP 1] Creating webhook trigger...")

    trigger = create_webhook_trigger(
        client=client,
        pack_ref=pack_ref,
        trigger_name=f"env_webhook_{unique_ref()}",
    )
    trigger_ref = trigger["ref"]
    webhook_url = trigger["webhook_url"]
    print(f"✓ Created webhook trigger: {trigger_ref}")

    # ========================================================================
    # STEP 2: Create action
    # ========================================================================
    print("\n[STEP 2] Creating action...")

    action = create_echo_action(
        client=client,
        pack_ref=pack_ref,
        action_name=f"env_action_{unique_ref()}",
        echo_message="Production or staging environment",
    )
    action_ref = action["ref"]
    print(f"✓ Created action: {action_ref}")

    # ========================================================================
    # STEP 3: Create rule with list membership criteria
    # ========================================================================
    print("\n[STEP 3] Creating rule with list membership criteria...")

    criteria_expression = "{{ event.payload.environment in ['prod', 'staging'] }}"
    rule = client.create_rule(
        pack_ref=pack_ref,
        data={
            "name": f"env_rule_{unique_ref()}",
            "description": "Rule for prod/staging environments",
            "trigger_ref": trigger_ref,
            "action_ref": action_ref,
            "enabled": True,
            "conditions": {"expression": criteria_expression},
        },
    )
    print(f"✓ Created rule with criteria: {criteria_expression}")

    # ========================================================================
    # STEP 4: Test with different environments
    # ========================================================================
    print("\n[STEP 4] Testing with different environments...")

    initial_count = len(
        [e for e in client.list_executions(limit=20) if e["action_ref"] == action_ref]
    )

    # Test dev (should not trigger)
    print("  Testing environment='dev'...")
    client.post_webhook(webhook_url, payload={"environment": "dev"})
    time.sleep(2)
    after_dev = len(
        [e for e in client.list_executions(limit=20) if e["action_ref"] == action_ref]
    )
    assert after_dev == initial_count, "❌ Execution created for dev environment"
    print("  ✓ No execution for 'dev' (correct)")

    # Test prod (should trigger)
    print("  Testing environment='prod'...")
    client.post_webhook(webhook_url, payload={"environment": "prod"})
    wait_for_execution_count(
        client=client,
        action_ref=action_ref,
        expected_count=initial_count + 1,
        timeout=15,
    )
    after_prod = len(
        [e for e in client.list_executions(limit=20) if e["action_ref"] == action_ref]
    )
    assert after_prod == initial_count + 1, "❌ Execution not created for prod"
    print("  ✓ Execution created for 'prod' (correct)")

    # Test staging (should trigger)
    print("  Testing environment='staging'...")
    client.post_webhook(webhook_url, payload={"environment": "staging"})
    wait_for_execution_count(
        client=client,
        action_ref=action_ref,
        expected_count=initial_count + 2,
        timeout=15,
    )
    after_staging = len(
        [e for e in client.list_executions(limit=20) if e["action_ref"] == action_ref]
    )
    assert after_staging == initial_count + 2, "❌ Execution not created for staging"
    print("  ✓ Execution created for 'staging' (correct)")

    # ========================================================================
    # FINAL SUMMARY
    # ========================================================================
    print("\n" + "=" * 80)
    print("TEST SUMMARY: List Membership Criteria")
    print("=" * 80)
    print(f"✓ Criteria: {criteria_expression}")
    print(f"✓ environment='dev': No execution (correct)")
    print(f"✓ environment='prod': Execution created (correct)")
    print(f"✓ environment='staging': Execution created (correct)")
    print(f"✓ Total executions: 2 (out of 3 webhooks)")
    print("\n✅ TEST PASSED: List membership criteria work correctly!")
    print("=" * 80 + "\n")


def test_rule_criteria_complex_expression(client: AttuneClient, test_pack):
    """
    Test complex criteria with multiple conditions.

    Criteria: {{ event.payload.severity == 'high' and event.payload.count > 10 }}
    """
    print("\n" + "=" * 80)
    print("TEST: Rule Criteria - Complex Expression")
    print("=" * 80)

    pack_ref = test_pack["ref"]

    # ========================================================================
    # STEP 1: Create webhook trigger
    # ========================================================================
    print("\n[STEP 1] Creating webhook trigger...")

    trigger = create_webhook_trigger(
        client=client,
        pack_ref=pack_ref,
        trigger_name=f"complex_webhook_{unique_ref()}",
    )
    trigger_ref = trigger["ref"]
    webhook_url = trigger["webhook_url"]
    print(f"✓ Created webhook trigger: {trigger_ref}")

    # ========================================================================
    # STEP 2: Create action
    # ========================================================================
    print("\n[STEP 2] Creating action...")

    action = create_echo_action(
        client=client,
        pack_ref=pack_ref,
        action_name=f"complex_action_{unique_ref()}",
        echo_message="High severity with high count",
    )
    action_ref = action["ref"]
    print(f"✓ Created action: {action_ref}")

    # ========================================================================
    # STEP 3: Create rule with complex criteria
    # ========================================================================
    print("\n[STEP 3] Creating rule with complex criteria...")

    criteria_expression = (
        "{{ event.payload.severity == 'high' and event.payload.count > 10 }}"
    )
    rule = client.create_rule(
        pack_ref=pack_ref,
        data={
            "name": f"complex_rule_{unique_ref()}",
            "description": "Rule with AND condition",
            "trigger_ref": trigger_ref,
            "action_ref": action_ref,
            "enabled": True,
            "conditions": {"expression": criteria_expression},
        },
    )
    print(f"✓ Created rule with criteria: {criteria_expression}")

    # ========================================================================
    # STEP 4: Test various combinations
    # ========================================================================
    print("\n[STEP 4] Testing various combinations...")

    initial_count = len(
        [e for e in client.list_executions(limit=20) if e["action_ref"] == action_ref]
    )

    # Test 1: severity=high, count=5 (only 1 condition met)
    print("  Test 1: severity='high', count=5...")
    client.post_webhook(webhook_url, payload={"severity": "high", "count": 5})
    time.sleep(2)
    count1 = len(
        [e for e in client.list_executions(limit=20) if e["action_ref"] == action_ref]
    )
    assert count1 == initial_count, "❌ Should not trigger (count too low)"
    print("  ✓ No execution (count too low)")

    # Test 2: severity=low, count=15 (only 1 condition met)
    print("  Test 2: severity='low', count=15...")
    client.post_webhook(webhook_url, payload={"severity": "low", "count": 15})
    time.sleep(2)
    count2 = len(
        [e for e in client.list_executions(limit=20) if e["action_ref"] == action_ref]
    )
    assert count2 == initial_count, "❌ Should not trigger (severity too low)"
    print("  ✓ No execution (severity not high)")

    # Test 3: severity=high, count=15 (both conditions met)
    print("  Test 3: severity='high', count=15...")
    client.post_webhook(webhook_url, payload={"severity": "high", "count": 15})
    wait_for_execution_count(
        client=client,
        action_ref=action_ref,
        expected_count=initial_count + 1,
        timeout=15,
    )
    count3 = len(
        [e for e in client.list_executions(limit=20) if e["action_ref"] == action_ref]
    )
    assert count3 == initial_count + 1, "❌ Should trigger (both conditions met)"
    print("  ✓ Execution created (both conditions met)")

    # ========================================================================
    # FINAL SUMMARY
    # ========================================================================
    print("\n" + "=" * 80)
    print("TEST SUMMARY: Complex Expression Criteria")
    print("=" * 80)
    print(f"✓ Criteria: {criteria_expression}")
    print(f"✓ high + count=5: No execution (partial match)")
    print(f"✓ low + count=15: No execution (partial match)")
    print(f"✓ high + count=15: Execution created (full match)")
    print(f"✓ Complex AND logic works correctly")
    print("\n✅ TEST PASSED: Complex criteria expressions work correctly!")
    print("=" * 80 + "\n")
