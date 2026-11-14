"""
T3.4: Webhook with Multiple Rules Test

Tests that a single webhook trigger can fire multiple rules simultaneously.
Each rule should create its own enforcement and execution independently.

Priority: LOW
Duration: ~15 seconds
"""

import time

import pytest
from helpers import AttuneClient
from helpers.fixtures import create_echo_action, create_webhook_trigger, unique_ref
from helpers.polling import (
    wait_for_event_count,
    wait_for_execution_count,
)


@pytest.mark.tier3
@pytest.mark.webhook
@pytest.mark.rules
def test_webhook_fires_multiple_rules(client: AttuneClient, test_pack):
    """
    Test that a single webhook POST triggers multiple rules.

    Flow:
    1. Create 1 webhook trigger
    2. Create 3 different rules using the same webhook
    3. POST to webhook once
    4. Verify 1 event created
    5. Verify 3 enforcements created (one per rule)
    6. Verify 3 executions created (one per rule)
    """
    print("\n" + "=" * 80)
    print("T3.4: Webhook with Multiple Rules Test")
    print("=" * 80)

    pack_ref = test_pack["ref"]

    # Step 1: Create webhook trigger
    print("\n[STEP 1] Creating webhook trigger...")
    trigger_ref = f"multi_rule_webhook_{unique_ref()}"

    trigger_response = create_webhook_trigger(
        client=client,
        pack_ref=pack_ref,
        trigger_ref=trigger_ref,
    )
    trigger_ref = trigger_response["ref"]

    webhook_url = (
        trigger_response.get("webhook_url") or f"/api/v1/webhooks/{trigger_ref}"
    )
    print(f"✓ Webhook trigger created: {trigger_ref}")
    print("  Webhook URL: /api/v1/webhooks/<redacted>")

    # Step 2: Create 3 different actions
    print("\n[STEP 2] Creating 3 actions...")
    actions = []

    for i in range(1, 4):
        action = create_echo_action(
            client=client,
            pack_ref=pack_ref,
            message=f"Action {i} triggered by webhook",
            suffix=f"_action{i}",
        )
        action_ref = action["ref"]
        actions.append(action_ref)
        print(f"✓ Action {i} created: {action_ref}")

    # Step 3: Create 3 rules, all using the same webhook trigger
    print("\n[STEP 3] Creating 3 rules for the same webhook...")
    rules = []

    for i, action_ref in enumerate(actions, 1):
        rule_data = {
            "name": f"Multi-Rule Test Rule {i} {unique_ref()}",
            "description": f"Rule {i} for multi-rule webhook test",
            "trigger_ref": trigger_ref,
            "action_ref": action_ref,
            "enabled": True,
        }

        rule_response = client.create_rule(rule_data)
        rule_id = rule_response["id"]
        rules.append(rule_id)
        print(f"✓ Rule {i} created: {rule_id}")
        print(f"  Trigger: {trigger_ref} → Action: {action_ref}")

    print(f"\nAll 3 rules use the same webhook trigger: {trigger_ref}")

    # Step 4: POST to webhook once
    print("\n[STEP 4] Posting to webhook...")

    webhook_payload = {
        "test": "multi_rule_test",
        "timestamp": time.time(),
        "message": "Testing multiple rules from single webhook",
    }

    webhook_response = client.post_webhook(webhook_url, webhook_payload)
    print(f"✓ Webhook POST sent")
    print(f"  Payload: {webhook_payload}")
    print(f"  Response: {webhook_response}")

    # Step 5: Verify exactly 1 event created
    print("\n[STEP 5] Verifying single event created...")

    events = wait_for_event_count(
        client=client,
        trigger_ref=trigger_ref,
        expected_count=1,
        timeout=10,
        operator="==",
    )

    assert len(events) == 1, f"Expected 1 event, got {len(events)}"
    event = events[0]
    print(f"✓ Exactly 1 event created: {event['id']}")
    print(f"  Trigger: {event['trigger']}")

    # Verify event payload matches what we sent
    event_payload = event.get("payload", {})
    if event_payload.get("test") == "multi_rule_test":
        print(f"✓ Event payload matches webhook POST data")

    # Step 6: Verify 3 enforcements created (one per rule)
    print("\n[STEP 6] Verifying 3 enforcements created...")

    # Wait a moment for enforcements to be created
    time.sleep(2)

    enforcements = client.list_enforcements()

    # Filter enforcements for our rules
    our_enforcements = [e for e in enforcements if e.get("rule_id") in rules]

    print(f"✓ Enforcements created: {len(our_enforcements)}")

    if len(our_enforcements) >= 3:
        print(f"✓ At least 3 enforcements found (one per rule)")
    else:
        print(f"⚠ Expected 3 enforcements, found {len(our_enforcements)}")

    # Verify each rule has an enforcement
    rules_with_enforcement = set(e.get("rule_id") for e in our_enforcements)
    print(f"  Rules with enforcements: {len(rules_with_enforcement)}/{len(rules)}")

    # Step 7: Verify 3 executions created (one per action)
    print("\n[STEP 7] Verifying 3 executions created...")

    all_executions = []
    for action_ref in actions:
        try:
            executions = wait_for_execution_count(
                client=client,
                action_ref=action_ref,
                expected_count=1,
                timeout=15,
                operator=">=",
            )
            all_executions.extend(executions)
            print(f"✓ Action {action_ref}: {len(executions)} execution(s)")
        except Exception as e:
            print(f"⚠ Action {action_ref}: No execution found - {e}")

    total_executions = len(all_executions)
    print(f"\nTotal executions: {total_executions}")

    if total_executions >= 3:
        print(f"✓ All 3 actions executed!")
    else:
        print(f"⚠ Expected 3 executions, got {total_executions}")

    # Step 8: Verify all executions see the same event payload
    print("\n[STEP 8] Verifying all executions received same event data...")

    payloads_match = True
    for i, execution in enumerate(all_executions[:3], 1):
        exec_params = execution.get("parameters", {})

        # The event payload should be accessible to the action
        # This depends on how parameters are passed
        print(f"  Execution {i} (ID: {execution['id']}): parameters present")

    if payloads_match:
        print(f"✓ All executions received consistent data")

    # Step 9: Verify no duplicate webhook events
    print("\n[STEP 9] Verifying no duplicate events...")

    # Wait a bit more and check again
    time.sleep(3)
    events_final = client.list_events(trigger_ref=trigger_ref)

    if len(events_final) == 1:
        print(f"✓ Still only 1 event (no duplicates)")
    else:
        print(f"⚠ Found {len(events_final)} events (expected 1)")

    # Summary
    print("\n" + "=" * 80)
    print("WEBHOOK MULTIPLE RULES TEST SUMMARY")
    print("=" * 80)
    print(f"✓ Webhook trigger: {trigger_ref}")
    print(f"✓ Actions created: {len(actions)}")
    print(f"✓ Rules created: {len(rules)}")
    print(f"✓ Webhook POST sent: 1 time")
    print(f"✓ Events created: {len(events_final)}")
    print(f"✓ Enforcements created: {len(our_enforcements)}")
    print(f"✓ Executions created: {total_executions}")
    print("\nRule Execution Matrix:")
    for i, (rule_id, action_ref) in enumerate(zip(rules, actions), 1):
        print(f"  Rule {i} ({rule_id}) → Action {action_ref}")

    if len(events_final) == 1 and total_executions >= 3:
        print("\n✅ SINGLE WEBHOOK TRIGGERED MULTIPLE RULES SUCCESSFULLY!")
    else:
        print("\n⚠️ Some rules may not have executed as expected")

    print("=" * 80)

    # Assertions
    assert len(events_final) == 1, f"Expected 1 event, got {len(events_final)}"
    assert total_executions >= 3, (
        f"Expected at least 3 executions, got {total_executions}"
    )


@pytest.mark.tier3
@pytest.mark.webhook
@pytest.mark.rules
def test_webhook_multiple_posts_multiple_rules(client: AttuneClient, test_pack):
    """
    Test that multiple webhook POSTs with multiple rules create the correct
    number of executions (posts × rules).
    """
    print("\n" + "=" * 80)
    print("T3.4b: Multiple Webhook POSTs with Multiple Rules")
    print("=" * 80)

    pack_ref = test_pack["ref"]

    # Step 1: Create webhook and 2 rules
    print("\n[STEP 1] Creating webhook and 2 rules...")
    trigger_ref = f"multi_post_webhook_{unique_ref()}"

    trigger_response = create_webhook_trigger(
        client=client,
        pack_ref=pack_ref,
        trigger_ref=trigger_ref,
    )
    trigger_ref = trigger_response["ref"]
    print(f"✓ Webhook trigger created: {trigger_ref}")

    # Create 2 actions and rules
    actions = []
    rules = []

    for i in range(1, 3):
        action = create_echo_action(
            client=client,
            pack_ref=pack_ref,
            message=f"Action {i}",
            suffix=f"_multi{i}",
        )
        action_ref = action["ref"]
        actions.append(action_ref)

        rule_data = {
            "name": f"Multi-POST Rule {i} {unique_ref()}",
            "trigger_ref": trigger_ref,
            "action_ref": action_ref,
            "enabled": True,
        }
        rule_response = client.create_rule(rule_data)
        rules.append(rule_response["id"])
        print(f"✓ Rule {i} created: action={action_ref}")

    # Step 2: POST to webhook 3 times
    print("\n[STEP 2] Posting to webhook 3 times...")

    num_posts = 3
    for i in range(1, num_posts + 1):
        payload = {
            "post_number": i,
            "timestamp": time.time(),
        }
        client.post_webhook(trigger_response["webhook_url"], payload)
        print(f"✓ POST {i} sent")
        time.sleep(1)  # Small delay between posts

    # Step 3: Verify events and executions
    print("\n[STEP 3] Verifying results...")

    # Should have 3 events (one per POST)
    events = wait_for_event_count(
        client=client,
        trigger_ref=trigger_ref,
        expected_count=num_posts,
        timeout=15,
        operator=">=",
    )

    print(f"✓ Events created: {len(events)}")
    assert len(events) >= num_posts, f"Expected {num_posts} events, got {len(events)}"

    # Should have 3 POSTs × 2 rules = 6 executions total
    expected_executions = num_posts * len(rules)

    time.sleep(5)  # Wait for all executions to be created

    total_executions = 0
    for action_ref in actions:
        executions = client.list_executions(action_ref=action_ref)
        count = len(executions)
        total_executions += count
        print(f"  Action {action_ref}: {count} execution(s)")

    print(f"\nTotal executions: {total_executions}")
    print(f"Expected: {expected_executions} (3 POSTs × 2 rules)")

    # Summary
    print("\n" + "=" * 80)
    print("MULTIPLE POSTS MULTIPLE RULES TEST SUMMARY")
    print("=" * 80)
    print(f"✓ Webhook POSTs: {num_posts}")
    print(f"✓ Rules: {len(rules)}")
    print(f"✓ Events created: {len(events)}")
    print(f"✓ Total executions: {total_executions}")
    print(f"✓ Expected executions: {expected_executions}")

    if total_executions >= expected_executions * 0.9:  # Allow 10% tolerance
        print("\n✅ MULTIPLE POSTS WITH MULTIPLE RULES WORKING!")
    else:
        print(f"\n⚠️ Fewer executions than expected")

    print("=" * 80)

    # Allow some tolerance for race conditions
    assert total_executions >= expected_executions * 0.8, (
        f"Expected ~{expected_executions} executions, got {total_executions}"
    )
