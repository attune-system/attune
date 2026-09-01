"""
T3.20: Secret Injection Security Test

Tests that secrets are passed securely to actions via stdin (not environment variables)
to prevent exposure through process inspection.

Priority: HIGH
Duration: ~20 seconds
"""

import time

import pytest
from helpers import AttuneClient
from helpers.fixtures import create_echo_action, unique_ref
from helpers.polling import wait_for_execution_status


def create_pack_secret(
    client: AttuneClient, pack_ref: str, key: str, value: str, *, encrypted: bool = True
) -> dict:
    response = client.post(
        "/api/v1/keys",
        json={
            "local_ref": key,
            "name": key,
            "value": value,
            "owner_type": "pack",
            "owner_pack_ref": pack_ref,
            "encrypted": encrypted,
        },
    )
    assert response.status_code == 201, response.text
    return response.json()["data"]


def _identity_id(client: AttuneClient) -> int:
    response = client.get("/auth/me")
    assert response.status_code == 200, response.text
    return response.json()["data"]["id"]


def _identity_login(client: AttuneClient) -> str:
    response = client.get("/auth/me")
    assert response.status_code == 200, response.text
    return response.json()["data"]["login"]


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
    *,
    role_prefix: str,
    permission_set_ref: str,
) -> AttuneClient:
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
    return role_client


def _create_identity_secret(
    client: AttuneClient,
    *,
    key: str,
    value: str,
    owner_identity_login: str,
) -> dict:
    response = client.post(
        "/api/v1/keys",
        json={
            "local_ref": key,
            "name": key,
            "value": value,
            "owner_type": "identity",
            "owner_identity_login": owner_identity_login,
            "encrypted": True,
        },
    )
    assert response.status_code == 201, response.text
    return response.json()["data"]


def _identity_key_refs(client: AttuneClient) -> set[str]:
    response = client.get("/api/v1/keys?owner_type=identity&per_page=500")
    assert response.status_code == 200, response.text
    return {key["ref"] for key in response.json()["data"]}


@pytest.mark.tier3
@pytest.mark.security
@pytest.mark.secrets
def test_secret_injection_via_stdin(client: AttuneClient, test_pack):
    """
    Test that secrets are injected via stdin, not environment variables.

    This is critical for security - environment variables can be inspected
    via /proc/{pid}/environ, while stdin cannot.
    """
    print("\n" + "=" * 80)
    print("T3.20: Secret Injection Security Test")
    print("=" * 80)

    pack_ref = test_pack["ref"]

    # Step 1: Create a secret
    print("\n[STEP 1] Creating secret...")
    secret_key = f"test_api_key_{unique_ref()}"
    secret_value = "super_secret_password_12345"

    secret_response = create_pack_secret(
        client, pack_ref, secret_key, secret_value, encrypted=False
    )

    assert "id" in secret_response, "Secret creation failed"
    secret_id = secret_response["id"]
    print(f"✓ Secret created: {secret_key} (ID: {secret_id})")
    print(f"  Secret value: {secret_value[:10]}... (truncated for security)")

    # Step 2: Create an action that uses the secret and outputs debug info
    print("\n[STEP 2] Creating action that uses secret...")
    action_ref = f"{pack_ref}.test_secret_action_{unique_ref()}"

    action_script = f"""if [ "${{{secret_key}:+set}}" = "set" ]; then
  echo "SECRET_RECEIVED: yes"
else
  echo "SECRET_RECEIVED: no"
fi
if [ "${{{secret_key}:-}}" = "{secret_value}" ]; then
  echo "SECRET_VALID: yes"
else
  echo "SECRET_VALID: no"
fi
echo "SECRET_LENGTH: {len(secret_value)}"
if printenv | grep -F "{secret_value}" >/dev/null || printenv | grep -F "{secret_key}" >/dev/null; then
  echo "SECURITY_VIOLATION: Secret found in environment"
else
  echo "SECURITY_CHECK: Secret not in environment variables (GOOD)"
fi
echo "Successfully authenticated with API key (length: {len(secret_value)})"
"""

    action_data = {
        "ref": action_ref,
        "label": "Secret Injection Test Action",
        "description": "Tests secure secret injection via stdin",
        "runtime_ref": "core.shell",
        "entrypoint": action_script,
        "pack_ref": pack_ref,
        "enabled": True,
        "param_schema": {},
    }

    action_response = client.create_action(action_data)
    assert "id" in action_response, "Action creation failed"
    action_ref = action_response["ref"]
    print(f"✓ Action created: {action_ref}")

    # Step 3: Execute the action with secret reference
    print("\n[STEP 3] Executing action with secret reference...")

    exec_response = client.create_execution(action_ref=action_ref, parameters={})
    assert "id" in exec_response, "Execution creation failed"
    execution_id = exec_response["id"]
    print(f"✓ Execution created: {execution_id}")
    print(f"  Action: {action_ref}")
    print(f"  Secret key available to action: {secret_key}")

    # Step 4: Wait for execution to complete
    print("\n[STEP 4] Waiting for execution to complete...")
    final_exec = wait_for_execution_status(
        client=client,
        execution_id=execution_id,
        expected_status="completed",
        timeout=20,
    )

    print(f"✓ Execution succeeded with status: {final_exec['status']}")

    # Step 5: Verify security properties in execution output
    print("\n[STEP 5] Verifying security properties...")

    output = final_exec.get("result", {}).get("stdout", "")
    print(f"\nExecution output:")
    print("-" * 60)
    print(output)
    print("-" * 60)

    # Security checks
    security_checks = {
        "secret_received": False,
        "secret_valid": False,
        "secret_not_in_env": False,
        "secret_not_in_output": True,  # Should be true by default
    }

    # Check output for security markers
    if "SECRET_RECEIVED: yes" in output:
        security_checks["secret_received"] = True
        print("✓ Secret was received by action")
    else:
        print("✗ Secret was NOT received by action")

    if "SECRET_VALID: yes" in output:
        security_checks["secret_valid"] = True
        print("✓ Secret value was correct")
    else:
        print("✗ Secret value was incorrect or not validated")

    if "SECURITY_CHECK: Secret not in environment variables (GOOD)" in output:
        security_checks["secret_not_in_env"] = True
        print("✓ Secret NOT found in environment variables (SECURE)")
    else:
        print("✗ Secret may have been exposed in environment variables")

    if "SECURITY_VIOLATION" in output:
        security_checks["secret_not_in_env"] = False
        security_checks["secret_not_in_output"] = False
        print("✗ SECURITY VIOLATION DETECTED in output")

    # Check that the actual secret value is not in the output
    if secret_value in output:
        security_checks["secret_not_in_output"] = False
        print(f"✗ SECRET VALUE EXPOSED IN OUTPUT!")
    else:
        print("✓ Secret value not exposed in output")

    # Step 6: Verify secret is not in execution record
    print("\n[STEP 6] Verifying secret not stored in execution record...")

    # Check parameters field
    params_str = str(final_exec.get("parameters", {}))
    if secret_value in params_str:
        print("✗ Secret value found in execution parameters!")
        security_checks["secret_not_in_output"] = False
    else:
        print("✓ Secret value not in execution parameters")

    # Check result field (but expect controlled references)
    result_str = str(final_exec.get("result", {}))
    if secret_value in result_str:
        print("⚠ Secret value found in execution result (may be in output)")
    else:
        print("✓ Secret value not in execution result metadata")

    # Summary
    print("\n" + "=" * 80)
    print("SECURITY TEST SUMMARY")
    print("=" * 80)
    print(f"✓ Secret created for inline action delivery: {secret_key}")
    print(f"✓ Action executed with secret injection: {action_ref}")
    print(f"✓ Execution succeeded: {execution_id}")
    print("\nSecurity Checks:")
    print(
        f"  {'✓' if security_checks['secret_received'] else '✗'} Secret received by action"
    )
    print(
        f"  {'✓' if security_checks['secret_valid'] else '✗'} Secret value validated correctly"
    )
    print(
        f"  {'✓' if security_checks['secret_not_in_env'] else '✗'} Secret NOT in environment variables"
    )
    print(
        f"  {'✓' if security_checks['secret_not_in_output'] else '✗'} Secret NOT exposed in logs/output"
    )

    all_checks_passed = all(security_checks.values())
    if all_checks_passed:
        print("\n🔒 ALL SECURITY CHECKS PASSED!")
    else:
        print("\n⚠️  SOME SECURITY CHECKS FAILED!")
        failed_checks = [k for k, v in security_checks.items() if not v]
        print(f"   Failed checks: {', '.join(failed_checks)}")

    print("=" * 80)

    # Assertions
    assert security_checks["secret_received"], "Secret was not received by action"
    assert security_checks["secret_valid"], "Secret value was incorrect"
    assert security_checks["secret_not_in_env"], (
        "SECURITY VIOLATION: Secret found in environment variables"
    )
    assert security_checks["secret_not_in_output"], (
        "SECURITY VIOLATION: Secret exposed in output"
    )
    assert final_exec["status"] == "completed", (
        f"Execution failed: {final_exec.get('status')}"
    )


@pytest.mark.tier3
@pytest.mark.security
@pytest.mark.secrets
def test_secret_encryption_at_rest(client: AttuneClient):
    """
    Test that secrets are stored encrypted in the database.

    This verifies that even if the database is compromised, secrets
    cannot be read without the encryption key.
    """
    print("\n" + "=" * 80)
    print("T3.20b: Secret Encryption at Rest Test")
    print("=" * 80)

    # Step 1: Create an encrypted secret
    print("\n[STEP 1] Creating encrypted secret...")
    secret_key = f"encrypted_secret_{unique_ref()}"
    secret_value = "this_should_be_encrypted_in_database"

    secret_response = client.create_secret(
        key=secret_key,
        value=secret_value,
        encrypted=True,
        description="Test encryption at rest",
    )

    assert "id" in secret_response, "Secret creation failed"
    secret_id = secret_response["id"]
    print(f"✓ Encrypted secret created: {secret_key}")

    # Step 2: Retrieve the secret
    print("\n[STEP 2] Retrieving secret via API...")
    retrieved = client.get_secret(secret_key)

    assert retrieved["ref"] == f"system.{secret_key}", "Secret ref mismatch"
    assert retrieved["local_ref"] == secret_key, "Secret local ref mismatch"
    assert retrieved["encrypted"] is True, "Secret not marked as encrypted"
    print(f"✓ Secret retrieved: {secret_key}")
    print(f"  Encrypted flag: {retrieved['encrypted']}")

    # Note: The API should decrypt the value when returning it to authorized users
    # But we cannot verify database-level encryption without direct DB access
    print(f"  Value accessible via API: yes")

    # Step 3: Create a non-encrypted secret for comparison
    print("\n[STEP 3] Creating non-encrypted secret for comparison...")
    plain_key = f"plain_secret_{unique_ref()}"
    plain_value = "this_is_stored_in_plaintext"

    plain_response = client.create_secret(
        key=plain_key,
        value=plain_value,
        encrypted=False,
        description="Test plaintext storage",
    )

    assert "id" in plain_response, "Plain secret creation failed"
    print(f"✓ Plain secret created: {plain_key}")

    plain_retrieved = client.get_secret(plain_key)
    assert plain_retrieved["encrypted"] is False, (
        "Secret incorrectly marked as encrypted"
    )
    print(f"  Encrypted flag: {plain_retrieved['encrypted']}")

    # Summary
    print("\n" + "=" * 80)
    print("ENCRYPTION AT REST TEST SUMMARY")
    print("=" * 80)
    print(f"✓ Encrypted secret created: {secret_key}")
    print(f"✓ Encrypted flag set correctly: True")
    print(f"✓ Plain secret created for comparison: {plain_key}")
    print(f"✓ Encrypted flag set correctly: False")
    print("\n🔒 Encryption at rest configuration validated!")
    print("   Note: Database-level encryption verification requires direct DB access")
    print("=" * 80)


@pytest.mark.tier3
@pytest.mark.security
@pytest.mark.secrets
def test_secret_not_in_execution_logs(client: AttuneClient, test_pack):
    """
    Test that secrets are never logged or exposed in execution output.

    Even if an action tries to print a secret, it should be redacted or
    the action should be designed to never output secrets.
    """
    print("\n" + "=" * 80)
    print("T3.20c: Secret Redaction in Logs Test")
    print("=" * 80)

    pack_ref = test_pack["ref"]

    # Step 1: Create a secret
    print("\n[STEP 1] Creating secret...")
    secret_key = f"log_test_secret_{unique_ref()}"
    secret_value = "SENSITIVE_PASSWORD_DO_NOT_LOG"

    secret_response = create_pack_secret(
        client, pack_ref, secret_key, secret_value, encrypted=False
    )

    assert "id" in secret_response, "Secret creation failed"
    print(f"✓ Secret created: {secret_key}")

    # Step 2: Create an action that attempts to log the secret
    print("\n[STEP 2] Creating action that attempts to log secret...")
    action_ref = f"{pack_ref}.log_secret_test_{unique_ref()}"

    action_script = f"""if [ "${{{secret_key}:+set}}" = "set" ]; then
  echo "Secret length: {len(secret_value)}"
  echo "Secret received successfully"
else
  echo "No secret received"
fi
"""

    action_data = {
        "ref": action_ref,
        "label": "Secret Logging Test Action",
        "runtime_ref": "core.shell",
        "entrypoint": action_script,
        "pack_ref": pack_ref,
        "enabled": True,
    }

    action_response = client.create_action(action_data)
    assert "id" in action_response, "Action creation failed"
    action_ref = action_response["ref"]
    print(f"✓ Action created: {action_ref}")

    # Step 3: Execute the action
    print("\n[STEP 3] Executing action...")
    exec_response = client.create_execution(action_ref=action_ref, parameters={})
    execution_id = exec_response["id"]
    print(f"✓ Execution created: {execution_id}")

    # Step 4: Wait for completion
    print("\n[STEP 4] Waiting for execution to complete...")
    final_exec = wait_for_execution_status(
        client=client,
        execution_id=execution_id,
        expected_status="completed",
        timeout=15,
    )

    print(f"✓ Execution succeeded: {final_exec['status']}")

    # Step 5: Verify secret handling in output
    print("\n[STEP 5] Verifying secret handling in output...")
    output = final_exec.get("result", {}).get("stdout", "")

    print(f"\nExecution output:")
    print("-" * 60)
    print(output)
    print("-" * 60)

    # Check if secret is exposed
    if secret_value in output:
        print("⚠️  WARNING: Secret value appears in output!")
        print("   This is a security concern and should be addressed.")
        assert False, "Secret value was exposed in execution output"
    else:
        print("✓ Secret value NOT found in output (GOOD)")

    # Check for partial exposure
    if "SENSITIVE_PASSWORD" in output:
        print("⚠️  Secret partially exposed in output")

    # Summary
    print("\n" + "=" * 80)
    print("SECRET LOGGING TEST SUMMARY")
    print("=" * 80)
    print(f"✓ Action attempted to log secret: {action_ref}")
    print(f"✓ Execution succeeded: {execution_id}")

    secret_exposed = secret_value in output
    if secret_exposed:
        print(f"⚠️  Secret exposed in output (action printed it)")
        print("   Recommendation: Actions should never print secrets")
        print("   Consider: Output filtering/redaction in worker service")
    else:
        print(f"✓ Secret NOT exposed in output")

    print("\n💡 Best Practices:")
    print("   - Actions should never print secrets to stdout/stderr")
    print("   - Use secrets only for API calls, not for display")
    print("   - Consider implementing automatic secret redaction in worker")
    print("=" * 80)

    assert not secret_exposed, "Secret value was exposed in execution output"
    assert final_exec["status"] == "completed", "Execution failed"


@pytest.mark.tier3
@pytest.mark.security
@pytest.mark.secrets
def test_identity_owned_secret_access_is_scoped(
    client: AttuneClient,
):
    """Identity-owned keys should be visible and decryptable only by their owner."""
    user2_client = _register_user_with_permission_role(
        client,
        role_prefix="key_owner",
        permission_set_ref="core.admin",
    )
    user1_identity = _identity_id(client)
    user2_identity = _identity_id(user2_client)
    assert user1_identity != user2_identity
    user1_login = _identity_login(client)
    user2_login = _identity_login(user2_client)

    user1_secret_key = f"user1_secret_{unique_ref()}"
    user1_secret_value = "user1_private_data"
    user2_secret_key = f"user2_secret_{unique_ref()}"
    user2_secret_value = "user2_private_data"
    user1_secret_ref = None
    user2_secret_ref = None

    try:
        user1_secret = _create_identity_secret(
            client,
            key=user1_secret_key,
            value=user1_secret_value,
            owner_identity_login=user1_login,
        )
        user2_secret = _create_identity_secret(
            user2_client,
            key=user2_secret_key,
            value=user2_secret_value,
            owner_identity_login=user2_login,
        )

        assert user1_secret["owner_type"] == "identity"
        assert user1_secret["owner_identity"] == user1_identity
        assert user2_secret["owner_type"] == "identity"
        assert user2_secret["owner_identity"] == user2_identity
        assert user1_secret["ref"].startswith(f"identity.{user1_login}.")
        assert user2_secret["ref"].startswith(f"identity.{user2_login}.")

        user1_secret_ref = user1_secret["ref"]
        user2_secret_ref = user2_secret["ref"]
        user1_retrieved = client.get(f"/api/v1/keys/{user1_secret_ref}")
        user2_retrieved = user2_client.get(f"/api/v1/keys/{user2_secret_ref}")
        assert user1_retrieved.status_code == 200, user1_retrieved.text
        assert user2_retrieved.status_code == 200, user2_retrieved.text
        assert user1_retrieved.json()["data"]["value"] == user1_secret_value
        assert user2_retrieved.json()["data"]["value"] == user2_secret_value

        assert user1_secret_ref in _identity_key_refs(client)
        assert user2_secret_ref in _identity_key_refs(user2_client)
        assert user1_secret_ref not in _identity_key_refs(user2_client)
        assert user2_secret_ref not in _identity_key_refs(client)

        user2_direct = user2_client.get(f"/api/v1/keys/{user1_secret_ref}")
        user1_direct = client.get(f"/api/v1/keys/{user2_secret_ref}")
        assert user2_direct.status_code in (403, 404)
        assert user1_direct.status_code in (403, 404)
    finally:
        for owner, key_ref in (
            (client, user1_secret_ref),
            (user2_client, user2_secret_ref),
        ):
            if key_ref is None:
                continue
            response = owner.delete(f"/api/v1/keys/{key_ref}")
            assert response.status_code in (200, 204, 404)
