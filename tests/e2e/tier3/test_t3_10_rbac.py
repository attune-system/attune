"""
T3.10: RBAC Permission Checks Test

Tests that role-based access control (RBAC) is enforced across all API endpoints.
Users with different roles should have different levels of access.

Priority: MEDIUM
Duration: ~20 seconds
"""

import pytest
from helpers import AttuneClient
from helpers.fixtures import unique_ref


def _data(response):
    assert response.status_code in (200, 201), response.text
    body = response.json()
    return body.get("data", body) if isinstance(body, dict) else body


def _permission_set_id(client: AttuneClient, permission_set_ref: str) -> int:
    permission_sets = _data(client._request("GET", "/api/v1/permissions/sets"))
    for permission_set in permission_sets:
        if permission_set["ref"] == permission_set_ref:
            return permission_set["id"]
    raise AssertionError(f"Permission set {permission_set_ref!r} not found")


def _register_user_with_permission_role(
    client: AttuneClient,
    role_prefix: str,
    permission_set_ref: str,
) -> tuple[AttuneClient, str]:
    login = f"{role_prefix}_{unique_ref()}@example.com"
    password = f"{role_prefix}_password_123"
    role = f"e2e_{role_prefix}_{unique_ref()}"

    registration = client.register(
        login=login,
        password=password,
        display_name=f"E2E {role_prefix.title()}",
    )
    identity_id = registration["user"]["id"]

    permission_set_id = _permission_set_id(client, permission_set_ref)
    _data(
        client._request(
            "POST",
            f"/api/v1/permissions/sets/{permission_set_id}/roles",
            json={"role": role},
        )
    )
    _data(
        client._request(
            "POST",
            f"/api/v1/identities/{identity_id}/roles",
            json={"role": role},
        )
    )

    role_client = AttuneClient(base_url=client.base_url)
    role_client.login(login=login, password=password)
    return role_client, login


def _is_forbidden(error: Exception) -> bool:
    message = str(error).lower()
    return "403" in message or "forbidden" in message or "permission" in message


@pytest.mark.tier3
@pytest.mark.security
@pytest.mark.rbac
def test_viewer_role_permissions(client: AttuneClient):
    """
     Test that viewer role can only read resources, not create/update/delete.
    """
    print("\n" + "=" * 80)
    print("T3.10a: Viewer Role Permission Test")
    print("=" * 80)

    print("\n[STEP 1] Creating viewer user and assigning role...")
    viewer_client, viewer_login = _register_user_with_permission_role(
        client,
        role_prefix="viewer",
        permission_set_ref="core.viewer",
    )
    print(f"✓ Viewer logged in: {viewer_login}")

    # Step 2: Test READ operations (should succeed)
    print("\n[STEP 2] Testing READ operations (should succeed)...")

    read_tests = []

    # Test listing packs
    try:
        packs = viewer_client.list_packs()
        print(f"✓ Viewer can list packs: {len(packs)} packs visible")
        read_tests.append(("list_packs", True))
    except Exception as e:
        print(f"✗ Viewer cannot list packs: {e}")
        read_tests.append(("list_packs", False))

    # Test listing actions
    try:
        actions = viewer_client.list_actions()
        print(f"✓ Viewer can list actions: {len(actions)} actions visible")
        read_tests.append(("list_actions", True))
    except Exception as e:
        print(f"✗ Viewer cannot list actions: {e}")
        read_tests.append(("list_actions", False))

    # Test listing rules
    try:
        rules = viewer_client.list_rules()
        print(f"✓ Viewer can list rules: {len(rules)} rules visible")
        read_tests.append(("list_rules", True))
    except Exception as e:
        print(f"✗ Viewer cannot list rules: {e}")
        read_tests.append(("list_rules", False))

    # Step 3: Test CREATE operations (should fail)
    print("\n[STEP 3] Testing CREATE operations (should fail with 403)...")

    create_tests = []

    # Test creating pack
    try:
        pack_data = {
            "ref": f"viewer_test_pack_{unique_ref()}",
            "name": "Test Pack",
            "version": "1.0.0",
        }
        pack_response = viewer_client.create_pack(pack_data)
        print(f"✗ SECURITY VIOLATION: Viewer created pack: {pack_response.get('ref')}")
        create_tests.append(("create_pack", False))  # Should have failed
    except Exception as e:
        if _is_forbidden(e):
            print(f"✓ Viewer blocked from creating pack (403 Forbidden)")
            create_tests.append(("create_pack", True))
        else:
            print(f"⚠ Viewer create pack failed with unexpected error: {e}")
            create_tests.append(("create_pack", False))

    # Test creating action
    try:
        action_data = {
            "ref": f"viewer_test_action_{unique_ref()}",
            "name": "Test Action",
            "runtime_ref": "core.shell",
            "entrypoint": "echo.sh",
            "pack_ref": "core",
        }
        action_response = viewer_client.create_action(action_data)
        print(
            f"✗ SECURITY VIOLATION: Viewer created action: {action_response.get('ref')}"
        )
        create_tests.append(("create_action", False))
    except Exception as e:
        if _is_forbidden(e):
            print(f"✓ Viewer blocked from creating action (403 Forbidden)")
            create_tests.append(("create_action", True))
        else:
            print(f"⚠ Viewer create action failed: {e}")
            create_tests.append(("create_action", False))

    # Test creating rule
    try:
        rule_data = {
            "name": f"viewer_test_rule_{unique_ref()}",
            "pack_ref": "core",
            "trigger_ref": "core.intervaltimer",
            "action_ref": "core.echo",
            "enabled": True,
            "trigger_params": {"unit": "seconds", "interval": 60},
            "action_params": {"message": "viewer should not create this"},
        }
        rule_response = viewer_client.create_rule(rule_data)
        print(f"✗ SECURITY VIOLATION: Viewer created rule: {rule_response.get('id')}")
        create_tests.append(("create_rule", False))
    except Exception as e:
        if _is_forbidden(e):
            print(f"✓ Viewer blocked from creating rule (403 Forbidden)")
            create_tests.append(("create_rule", True))
        else:
            print(f"⚠ Viewer create rule failed: {e}")
            create_tests.append(("create_rule", False))

    # Step 4: Test EXECUTE operations (should fail)
    print("\n[STEP 4] Testing EXECUTE operations (should fail with 403)...")

    execute_tests = []

    # Test executing action
    try:
        exec_data = {"action": "core.echo", "parameters": {"message": "test"}}
        exec_response = viewer_client.execute_action(exec_data)
        print(
            f"✗ SECURITY VIOLATION: Viewer executed action: {exec_response.get('id')}"
        )
        execute_tests.append(("execute_action", False))
    except Exception as e:
        if _is_forbidden(e):
            print(f"✓ Viewer blocked from executing action (403 Forbidden)")
            execute_tests.append(("execute_action", True))
        else:
            print(f"⚠ Viewer execute failed: {e}")
            execute_tests.append(("execute_action", False))

    # Summary
    print("\n" + "=" * 80)
    print("VIEWER ROLE TEST SUMMARY")
    print("=" * 80)
    print(f"User: {viewer_login} (role: viewer)")
    print("\nREAD Permissions (should succeed):")
    for operation, passed in read_tests:
        status = "✓" if passed else "✗"
        print(f"  {status} {operation}: {'PASS' if passed else 'FAIL'}")

    print("\nCREATE Permissions (should fail):")
    for operation, blocked in create_tests:
        status = "✓" if blocked else "✗"
        print(
            f"  {status} {operation}: {'BLOCKED' if blocked else 'ALLOWED (VIOLATION)'}"
        )

    print("\nEXECUTE Permissions (should fail):")
    for operation, blocked in execute_tests:
        status = "✓" if blocked else "✗"
        print(
            f"  {status} {operation}: {'BLOCKED' if blocked else 'ALLOWED (VIOLATION)'}"
        )

    # Check results
    all_read_passed = all(passed for _, passed in read_tests)
    all_create_blocked = all(blocked for _, blocked in create_tests)
    all_execute_blocked = all(blocked for _, blocked in execute_tests)

    if all_read_passed and all_create_blocked and all_execute_blocked:
        print("\n✅ VIEWER ROLE PERMISSIONS CORRECT!")
    else:
        print("\n⚠️  RBAC ISSUES DETECTED:")
        if not all_read_passed:
            print("   - Viewer cannot read some resources")
        if not all_create_blocked:
            print("   - Viewer can create resources (SECURITY ISSUE)")
        if not all_execute_blocked:
            print("   - Viewer can execute actions (SECURITY ISSUE)")

    print("=" * 80)

    assert all_read_passed, "Viewer should be able to read resources"
    assert all_create_blocked, "Viewer should not be able to create resources"
    assert all_execute_blocked, "Viewer should not be able to execute actions"


@pytest.mark.tier3
@pytest.mark.security
@pytest.mark.rbac
def test_admin_role_permissions(client: AttuneClient):
    """
    Test that admin role has full access to all resources.
    """
    print("\n" + "=" * 80)
    print("T3.10b: Admin Role Permission Test")
    print("=" * 80)

    # The default client is typically admin
    print("\n[STEP 1] Testing admin permissions (using default client)...")

    operations = []

    # Test create pack
    try:
        pack_data = {
            "ref": f"admin_test_pack_{unique_ref()}",
            "name": "Admin Test Pack",
            "version": "1.0.0",
            "description": "Testing admin permissions",
        }
        pack_response = client.create_pack(pack_data)
        print(f"✓ Admin can create pack: {pack_response['ref']}")
        operations.append(("create_pack", True))

        # Clean up
        client.delete_pack(pack_response["ref"])
        print(f"✓ Admin can delete pack")
        operations.append(("delete_pack", True))
    except Exception as e:
        print(f"✗ Admin cannot create/delete pack: {e}")
        operations.append(("create_pack", False))
        operations.append(("delete_pack", False))

    # Test create action
    try:
        action_data = {
            "ref": f"admin_test_action_{unique_ref()}",
            "name": "Admin Test Action",
            "runtime_ref": "core.shell",
            "entrypoint": "echo.sh",
            "pack_ref": "core",
            "enabled": True,
        }
        action_response = client.create_action(action_data)
        print(f"✓ Admin can create action: {action_response['ref']}")
        operations.append(("create_action", True))

        # Clean up
        client.delete_action(action_response["ref"])
        print(f"✓ Admin can delete action")
        operations.append(("delete_action", True))
    except Exception as e:
        print(f"✗ Admin cannot create/delete action: {e}")
        operations.append(("create_action", False))

    # Test execute action
    try:
        exec_data = {"action": "core.echo", "parameters": {"message": "admin test"}}
        exec_response = client.execute_action(exec_data)
        print(f"✓ Admin can execute action: execution {exec_response['id']}")
        operations.append(("execute_action", True))
    except Exception as e:
        print(f"✗ Admin cannot execute action: {e}")
        operations.append(("execute_action", False))

    # Summary
    print("\n" + "=" * 80)
    print("ADMIN ROLE TEST SUMMARY")
    print("=" * 80)
    print("Admin Operations:")
    for operation, passed in operations:
        status = "✓" if passed else "✗"
        print(f"  {status} {operation}: {'PASS' if passed else 'FAIL'}")

    all_passed = all(passed for _, passed in operations)
    if all_passed:
        print("\n✅ ADMIN HAS FULL ACCESS!")
    else:
        print("\n⚠️  ADMIN MISSING SOME PERMISSIONS")

    print("=" * 80)

    assert all_passed, "Admin should have full permissions"


@pytest.mark.tier3
@pytest.mark.security
@pytest.mark.rbac
def test_executor_role_permissions(client: AttuneClient):
    """
    Test that executor role can execute actions but not create resources.

    Executor role is for service accounts or CI/CD systems that only need
    to trigger executions, not manage infrastructure.
    """
    print("\n" + "=" * 80)
    print("T3.10c: Executor Role Permission Test")
    print("=" * 80)

    print("\n[STEP 1] Creating executor user and assigning role...")
    executor_client, executor_login = _register_user_with_permission_role(
        client,
        role_prefix="executor",
        permission_set_ref="core.executor",
    )
    print(f"✓ Executor logged in: {executor_login}")

    # Step 2: Test EXECUTE permissions (should succeed)
    print("\n[STEP 2] Testing EXECUTE permissions (should succeed)...")

    execute_tests = []

    try:
        exec_data = {"action": "core.echo", "parameters": {"message": "executor test"}}
        exec_response = executor_client.execute_action(exec_data)
        print(f"✓ Executor can execute action: execution {exec_response['id']}")
        execute_tests.append(("execute_action", True))
    except Exception as e:
        print(f"✗ Executor cannot execute action: {e}")
        execute_tests.append(("execute_action", False))

    # Step 3: Test CREATE permissions (should fail)
    print("\n[STEP 3] Testing CREATE permissions (should fail)...")

    create_tests = []

    # Try to create pack (should fail)
    try:
        pack_data = {
            "ref": f"exec_test_pack_{unique_ref()}",
            "name": "Executor Test Pack",
            "version": "1.0.0",
        }
        pack_response = executor_client.create_pack(pack_data)
        print(f"✗ VIOLATION: Executor created pack: {pack_response['ref']}")
        create_tests.append(("create_pack", False))
    except Exception as e:
        if _is_forbidden(e):
            print(f"✓ Executor blocked from creating pack")
            create_tests.append(("create_pack", True))
        else:
            print(f"⚠ Unexpected error: {e}")
            create_tests.append(("create_pack", False))

    # Step 4: Test READ permissions (should succeed)
    print("\n[STEP 4] Testing READ permissions (should succeed)...")

    read_tests = []

    try:
        actions = executor_client.list_actions()
        print(f"✓ Executor can list actions: {len(actions)} visible")
        read_tests.append(("list_actions", True))
    except Exception as e:
        print(f"✗ Executor cannot list actions: {e}")
        read_tests.append(("list_actions", False))

    # Summary
    print("\n" + "=" * 80)
    print("EXECUTOR ROLE TEST SUMMARY")
    print("=" * 80)
    print(f"User: {executor_login} (role: executor)")
    print("\nEXECUTE Permissions (should succeed):")
    for operation, passed in execute_tests:
        status = "✓" if passed else "✗"
        print(f"  {status} {operation}: {'PASS' if passed else 'FAIL'}")

    print("\nCREATE Permissions (should fail):")
    for operation, blocked in create_tests:
        status = "✓" if blocked else "✗"
        print(
            f"  {status} {operation}: {'BLOCKED' if blocked else 'ALLOWED (VIOLATION)'}"
        )

    print("\nREAD Permissions (should succeed):")
    for operation, passed in read_tests:
        status = "✓" if passed else "✗"
        print(f"  {status} {operation}: {'PASS' if passed else 'FAIL'}")

    all_execute_ok = all(passed for _, passed in execute_tests)
    all_create_blocked = all(blocked for _, blocked in create_tests)
    all_read_ok = all(passed for _, passed in read_tests)

    if all_execute_ok and all_create_blocked and all_read_ok:
        print("\n✅ EXECUTOR ROLE PERMISSIONS CORRECT!")
    else:
        print("\n⚠️  EXECUTOR ROLE ISSUES DETECTED")

    print("=" * 80)

    assert all_execute_ok, "Executor should be able to execute actions"
    assert all_create_blocked, "Executor should not be able to create packs"
    assert all_read_ok, "Executor should be able to read actions"


@pytest.mark.tier3
@pytest.mark.security
@pytest.mark.rbac
def test_role_permissions_summary():
    """
    Summary test documenting the expected RBAC permission matrix.

    This is a documentation test that doesn't execute API calls,
    but serves as a reference for the expected permission model.
    """
    print("\n" + "=" * 80)
    print("T3.10d: RBAC Permission Matrix Reference")
    print("=" * 80)

    permission_matrix = {
        "admin": {
            "packs": ["create", "read", "update", "delete"],
            "actions": ["create", "read", "update", "delete", "execute"],
            "rules": ["create", "read", "update", "delete"],
            "triggers": ["create", "read", "update", "delete"],
            "executions": ["read", "update", "cancel"],
            "events": ["read"],
            "enforcements": ["read"],
            "inquiries": ["read", "create", "update", "delete", "respond"],
            "keys": ["create", "read", "update", "delete", "decrypt"],
            "artifacts": ["create", "read", "update", "delete"],
            "identities": ["create", "read", "update", "delete", "manage"],
            "permissions": ["create", "read", "update", "delete", "manage"],
        },
        "editor": {
            "packs": ["create", "read", "update"],
            "actions": ["create", "read", "update", "execute"],
            "rules": ["create", "read", "update"],
            "triggers": ["create", "read", "update"],
            "executions": ["read", "cancel"],
            "queues": ["create", "read", "update", "delete"],
            "keys": ["read", "update", "decrypt"],
            "artifacts": ["read"],
        },
        "executor": {
            "packs": ["read"],
            "actions": ["read", "execute"],
            "rules": ["read"],
            "triggers": ["read"],
            "executions": ["read"],
            "keys": ["read"],
            "artifacts": ["read"],
        },
        "viewer": {
            "packs": ["read"],
            "actions": ["read"],
            "rules": ["read"],
            "triggers": ["read"],
            "executions": ["read"],
            "queues": ["read"],
            "keys": ["read"],
            "artifacts": ["read"],
        },
    }

    print("\nExpected Permission Matrix:\n")

    for role, permissions in permission_matrix.items():
        print(f"{role.upper()} Role:")
        for resource, ops in permissions.items():
            ops_str = ", ".join(ops) if ops else "none"
            print(f"  - {resource}: {ops_str}")
        print()

    print("=" * 80)
    print("📋 This matrix defines the expected RBAC behavior")
    print("=" * 80)

    # This test always passes - it's documentation
    assert True
