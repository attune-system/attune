//! Integration tests for Identity repository
//!
//! These tests verify CRUD operations, queries, and constraints
//! for the Identity repository.

mod helpers;

use attune_common::{
    repositories::{
        identity::{CreateIdentityInput, IdentityRepository, OidcUpsertInput, UpdateIdentityInput},
        Create, Delete, FindById, List, Update,
    },
    Error,
};
use helpers::*;
use serde_json::json;

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_create_identity() {
    let pool = create_test_pool().await.unwrap();

    let input = CreateIdentityInput {
        login: unique_pack_ref("testuser"),
        display_name: Some("Test User".to_string()),
        attributes: json!({"email": "test@example.com"}),
        password_hash: None,
    };

    let identity = IdentityRepository::create(&pool, input.clone())
        .await
        .unwrap();

    assert!(identity.login.starts_with("testuser_"));
    assert_eq!(identity.display_name, Some("Test User".to_string()));
    assert_eq!(identity.attributes["email"], "test@example.com");
    assert!(identity.created.timestamp() > 0);
    assert!(identity.updated.timestamp() > 0);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_create_identity_minimal() {
    let pool = create_test_pool().await.unwrap();

    let input = CreateIdentityInput {
        login: unique_pack_ref("minimal"),
        display_name: None,
        attributes: json!({}),
        password_hash: None,
    };

    let identity = IdentityRepository::create(&pool, input).await.unwrap();

    assert!(identity.login.starts_with("minimal_"));
    assert_eq!(identity.display_name, None);
    assert_eq!(identity.attributes, json!({}));
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_create_identity_duplicate_login() {
    let pool = create_test_pool().await.unwrap();

    let login = unique_pack_ref("duplicate");

    // Create first identity
    let input1 = CreateIdentityInput {
        login: login.clone(),
        display_name: Some("First".to_string()),
        attributes: json!({}),
        password_hash: None,
    };
    IdentityRepository::create(&pool, input1).await.unwrap();

    // Try to create second identity with same login
    let input2 = CreateIdentityInput {
        login: login.clone(),
        display_name: Some("Second".to_string()),
        attributes: json!({}),
        password_hash: None,
    };
    let result = IdentityRepository::create(&pool, input2).await;

    assert!(result.is_err());
    let err = result.unwrap_err();
    println!("Actual error: {:?}", err);
    match err {
        Error::AlreadyExists { entity, field, .. } => {
            assert_eq!(entity, "Identity");
            assert_eq!(field, "login");
        }
        _ => panic!("Expected AlreadyExists error, got: {:?}", err),
    }
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_find_identity_by_id() {
    let pool = create_test_pool().await.unwrap();

    let input = CreateIdentityInput {
        login: unique_pack_ref("findbyid"),
        display_name: Some("Find By ID".to_string()),
        attributes: json!({"key": "value"}),
        password_hash: None,
    };

    let created = IdentityRepository::create(&pool, input).await.unwrap();

    let found = IdentityRepository::find_by_id(&pool, created.id)
        .await
        .unwrap()
        .expect("Identity not found");

    assert_eq!(found.id, created.id);
    assert_eq!(found.login, created.login);
    assert_eq!(found.display_name, created.display_name);
    assert_eq!(found.attributes, created.attributes);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_find_identity_by_id_not_found() {
    let pool = create_test_pool().await.unwrap();

    let found = IdentityRepository::find_by_id(&pool, 999999).await.unwrap();

    assert!(found.is_none());
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_find_identity_by_login() {
    let pool = create_test_pool().await.unwrap();

    let login = unique_pack_ref("findbylogin");
    let input = CreateIdentityInput {
        login: login.clone(),
        display_name: Some("Find By Login".to_string()),
        attributes: json!({}),
        password_hash: None,
    };

    let created = IdentityRepository::create(&pool, input).await.unwrap();

    let found = IdentityRepository::find_by_login(&pool, &login)
        .await
        .unwrap()
        .expect("Identity not found");

    assert_eq!(found.id, created.id);
    assert_eq!(found.login, login);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_find_identity_by_login_not_found() {
    let pool = create_test_pool().await.unwrap();

    let found = IdentityRepository::find_by_login(&pool, "nonexistent_user_12345")
        .await
        .unwrap();

    assert!(found.is_none());
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_list_identities() {
    let pool = create_test_pool().await.unwrap();

    // Create multiple identities
    let input1 = CreateIdentityInput {
        login: unique_pack_ref("user1"),
        display_name: Some("User 1".to_string()),
        attributes: json!({}),
        password_hash: None,
    };
    let identity1 = IdentityRepository::create(&pool, input1).await.unwrap();

    let input2 = CreateIdentityInput {
        login: unique_pack_ref("user2"),
        display_name: Some("User 2".to_string()),
        attributes: json!({}),
        password_hash: None,
    };
    let identity2 = IdentityRepository::create(&pool, input2).await.unwrap();

    let identities = IdentityRepository::list(&pool).await.unwrap();

    // Should contain at least our created identities
    assert!(identities.len() >= 2);

    let identity_ids: Vec<i64> = identities.iter().map(|i| i.id).collect();
    assert!(identity_ids.contains(&identity1.id));
    assert!(identity_ids.contains(&identity2.id));
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_update_identity() {
    let pool = create_test_pool().await.unwrap();

    let input = CreateIdentityInput {
        login: unique_pack_ref("updatetest"),
        display_name: Some("Original Name".to_string()),
        attributes: json!({"key": "original"}),
        password_hash: None,
    };

    let identity = IdentityRepository::create(&pool, input).await.unwrap();
    let original_updated = identity.updated - chrono::Duration::seconds(1);
    set_updated_for_test(&pool, "identity", identity.id, original_updated).await;

    let update_input = UpdateIdentityInput {
        display_name: Some("Updated Name".to_string()),
        password_hash: None,
        attributes: Some(json!({"key": "updated", "new_key": "new_value"})),
        frozen: None,
    };

    let updated = IdentityRepository::update(&pool, identity.id, update_input)
        .await
        .unwrap();

    assert_eq!(updated.id, identity.id);
    assert_eq!(updated.login, identity.login); // Login should not change
    assert_eq!(updated.display_name, Some("Updated Name".to_string()));
    assert_eq!(updated.attributes["key"], "updated");
    assert_eq!(updated.attributes["new_key"], "new_value");
    assert!(updated.updated > original_updated);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_update_identity_partial() {
    let pool = create_test_pool().await.unwrap();

    let input = CreateIdentityInput {
        login: unique_pack_ref("partial"),
        display_name: Some("Original".to_string()),
        attributes: json!({"key": "value"}),
        password_hash: None,
    };

    let identity = IdentityRepository::create(&pool, input).await.unwrap();

    // Update only display_name
    let update_input = UpdateIdentityInput {
        display_name: Some("Only Display Name Changed".to_string()),
        password_hash: None,
        attributes: None,
        frozen: None,
    };

    let updated = IdentityRepository::update(&pool, identity.id, update_input)
        .await
        .unwrap();

    assert_eq!(
        updated.display_name,
        Some("Only Display Name Changed".to_string())
    );
    assert_eq!(updated.attributes, identity.attributes); // Should remain unchanged
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_update_identity_not_found() {
    let pool = create_test_pool().await.unwrap();

    let update_input = UpdateIdentityInput {
        display_name: Some("Updated Name".to_string()),
        password_hash: None,
        attributes: None,
        frozen: None,
    };

    let result = IdentityRepository::update(&pool, 999999, update_input).await;

    assert!(result.is_err());
    let err = result.unwrap_err();
    println!("Actual error: {:?}", err);
    match err {
        Error::NotFound { entity, .. } => {
            assert_eq!(entity, "identity");
        }
        _ => panic!("Expected NotFound error, got: {:?}", err),
    }
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_delete_identity() {
    let pool = create_test_pool().await.unwrap();

    let input = CreateIdentityInput {
        login: unique_pack_ref("deletetest"),
        display_name: Some("To Be Deleted".to_string()),
        attributes: json!({}),
        password_hash: None,
    };

    let identity = IdentityRepository::create(&pool, input).await.unwrap();

    // Verify identity exists
    let found = IdentityRepository::find_by_id(&pool, identity.id)
        .await
        .unwrap();
    assert!(found.is_some());

    // Delete the identity
    let deleted = IdentityRepository::delete(&pool, identity.id)
        .await
        .unwrap();
    assert!(deleted);

    // Verify identity no longer exists
    let not_found = IdentityRepository::find_by_id(&pool, identity.id)
        .await
        .unwrap();
    assert!(not_found.is_none());
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_delete_identity_not_found() {
    let pool = create_test_pool().await.unwrap();

    let deleted = IdentityRepository::delete(&pool, 999999).await.unwrap();

    assert!(!deleted);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_identity_timestamps_auto_populated() {
    let pool = create_test_pool().await.unwrap();

    let input = CreateIdentityInput {
        login: unique_pack_ref("timestamps"),
        display_name: Some("Timestamp Test".to_string()),
        attributes: json!({}),
        password_hash: None,
    };

    let identity = IdentityRepository::create(&pool, input).await.unwrap();

    // Timestamps should be set
    assert!(identity.created.timestamp() > 0);
    assert!(identity.updated.timestamp() > 0);

    // Created and updated should be very close initially
    let diff = (identity.updated - identity.created)
        .num_milliseconds()
        .abs();
    assert!(diff < 1000); // Within 1 second
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_identity_updated_changes_on_update() {
    let pool = create_test_pool().await.unwrap();

    let input = CreateIdentityInput {
        login: unique_pack_ref("updatetimestamp"),
        display_name: Some("Original".to_string()),
        attributes: json!({}),
        password_hash: None,
    };

    let identity = IdentityRepository::create(&pool, input).await.unwrap();
    let original_created = identity.created;
    let original_updated = identity.updated - chrono::Duration::seconds(1);
    set_updated_for_test(&pool, "identity", identity.id, original_updated).await;

    let update_input = UpdateIdentityInput {
        display_name: Some("Updated".to_string()),
        password_hash: None,
        attributes: None,
        frozen: None,
    };

    let updated = IdentityRepository::update(&pool, identity.id, update_input)
        .await
        .unwrap();

    // Created should remain the same
    assert_eq!(updated.created, original_created);

    // Updated should be newer
    assert!(updated.updated > original_updated);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_identity_with_complex_attributes() {
    let pool = create_test_pool().await.unwrap();

    let complex_attrs = json!({
        "email": "complex@example.com",
        "roles": ["admin", "user"],
        "metadata": {
            "last_login": "2024-01-01T00:00:00Z",
            "login_count": 42
        },
        "preferences": {
            "theme": "dark",
            "notifications": true
        }
    });

    let input = CreateIdentityInput {
        login: unique_pack_ref("complex"),
        display_name: Some("Complex User".to_string()),
        attributes: complex_attrs.clone(),
        password_hash: None,
    };

    let identity = IdentityRepository::create(&pool, input).await.unwrap();

    assert_eq!(identity.attributes, complex_attrs);
    assert_eq!(identity.attributes["roles"][0], "admin");
    assert_eq!(identity.attributes["metadata"]["login_count"], 42);
    assert_eq!(identity.attributes["preferences"]["theme"], "dark");

    // Verify it can be retrieved correctly
    let found = IdentityRepository::find_by_id(&pool, identity.id)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(found.attributes, complex_attrs);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_identity_login_case_sensitive() {
    let pool = create_test_pool().await.unwrap();

    let base = unique_pack_ref("case");
    let lower_login = format!("{}lower", base);
    let upper_login = format!("{}UPPER", base);

    // Create identity with lowercase login
    let input1 = CreateIdentityInput {
        login: lower_login.clone(),
        display_name: Some("Lower".to_string()),
        attributes: json!({}),
        password_hash: None,
    };
    let identity1 = IdentityRepository::create(&pool, input1).await.unwrap();

    // Create identity with uppercase login (should work - different login)
    let input2 = CreateIdentityInput {
        login: upper_login.clone(),
        display_name: Some("Upper".to_string()),
        attributes: json!({}),
        password_hash: None,
    };
    let identity2 = IdentityRepository::create(&pool, input2).await.unwrap();

    // Both should exist
    assert_ne!(identity1.id, identity2.id);
    assert_eq!(identity1.login, lower_login);
    assert_eq!(identity2.login, upper_login);

    // Find by login should be exact match
    let found_lower = IdentityRepository::find_by_login(&pool, &lower_login)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(found_lower.id, identity1.id);

    let found_upper = IdentityRepository::find_by_login(&pool, &upper_login)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(found_upper.id, identity2.id);
}

// ── LDAP-specific tests ──────────────────────────────────────────────────────

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_find_by_ldap_dn_found() {
    let pool = create_test_pool().await.unwrap();

    let login = unique_pack_ref("ldap_found");
    let server_url = "ldap://ldap.example.com";
    let dn = "uid=jdoe,ou=users,dc=example,dc=com";

    let input = CreateIdentityInput {
        login: login.clone(),
        display_name: Some("LDAP User".to_string()),
        attributes: json!({
            "ldap": {
                "server_url": server_url,
                "dn": dn,
                "login": "jdoe",
                "email": "jdoe@example.com"
            }
        }),
        password_hash: None,
    };

    let created = IdentityRepository::create(&pool, input).await.unwrap();

    let found = IdentityRepository::find_by_ldap_dn(&pool, server_url, dn)
        .await
        .unwrap()
        .expect("LDAP identity not found");

    assert_eq!(found.id, created.id);
    assert_eq!(found.login, login);
    assert_eq!(found.attributes["ldap"]["server_url"], server_url);
    assert_eq!(found.attributes["ldap"]["dn"], dn);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_find_by_ldap_dn_not_found() {
    let pool = create_test_pool().await.unwrap();

    let found = IdentityRepository::find_by_ldap_dn(
        &pool,
        "ldap://nonexistent.example.com",
        "uid=nobody,ou=users,dc=example,dc=com",
    )
    .await
    .unwrap();

    assert!(found.is_none());
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_find_by_ldap_dn_wrong_server() {
    let pool = create_test_pool().await.unwrap();

    let dn = "uid=jdoe,ou=users,dc=example,dc=com";

    let input = CreateIdentityInput {
        login: unique_pack_ref("ldap_wrong_srv"),
        display_name: Some("Server A User".to_string()),
        attributes: json!({
            "ldap": {
                "server_url": "ldap://server-a.example.com",
                "dn": dn,
                "login": "jdoe"
            }
        }),
        password_hash: None,
    };

    IdentityRepository::create(&pool, input).await.unwrap();

    // Search with same DN but different server — composite key must match both
    let found = IdentityRepository::find_by_ldap_dn(&pool, "ldap://server-b.example.com", dn)
        .await
        .unwrap();

    assert!(found.is_none());
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_find_by_ldap_dn_multiple_identities_different_servers() {
    let pool = create_test_pool().await.unwrap();

    let dn = "uid=shared,ou=users,dc=example,dc=com";
    let server_a = "ldap://multi-a.example.com";
    let server_b = "ldap://multi-b.example.com";

    let input_a = CreateIdentityInput {
        login: unique_pack_ref("ldap_multi_a"),
        display_name: Some("User on Server A".to_string()),
        attributes: json!({
            "ldap": {
                "server_url": server_a,
                "dn": dn,
                "login": "shared_a"
            }
        }),
        password_hash: None,
    };
    let identity_a = IdentityRepository::create(&pool, input_a).await.unwrap();

    let input_b = CreateIdentityInput {
        login: unique_pack_ref("ldap_multi_b"),
        display_name: Some("User on Server B".to_string()),
        attributes: json!({
            "ldap": {
                "server_url": server_b,
                "dn": dn,
                "login": "shared_b"
            }
        }),
        password_hash: None,
    };
    let identity_b = IdentityRepository::create(&pool, input_b).await.unwrap();

    // Query server A — should return identity_a
    let found_a = IdentityRepository::find_by_ldap_dn(&pool, server_a, dn)
        .await
        .unwrap()
        .expect("Identity for server A not found");
    assert_eq!(found_a.id, identity_a.id);
    assert_eq!(found_a.attributes["ldap"]["server_url"], server_a);

    // Query server B — should return identity_b
    let found_b = IdentityRepository::find_by_ldap_dn(&pool, server_b, dn)
        .await
        .unwrap()
        .expect("Identity for server B not found");
    assert_eq!(found_b.id, identity_b.id);
    assert_eq!(found_b.attributes["ldap"]["server_url"], server_b);

    // Confirm they are distinct identities
    assert_ne!(found_a.id, found_b.id);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_find_by_ldap_dn_ignores_oidc_attributes() {
    let pool = create_test_pool().await.unwrap();

    // Create an identity with OIDC attributes (no "ldap" key)
    let input = CreateIdentityInput {
        login: unique_pack_ref("ldap_oidc"),
        display_name: Some("OIDC User".to_string()),
        attributes: json!({
            "oidc": {
                "issuer": "https://auth.example.com",
                "subject": "abc123",
                "email": "oidc@example.com"
            }
        }),
        password_hash: None,
    };

    IdentityRepository::create(&pool, input).await.unwrap();

    // Searching by LDAP DN should not match OIDC-only identities
    let found = IdentityRepository::find_by_ldap_dn(&pool, "https://auth.example.com", "abc123")
        .await
        .unwrap();

    assert!(found.is_none());
}

// ── OIDC-specific tests ─────────────────────────────────────────────────────

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_find_by_oidc_subject_strict_three_way_match() {
    let pool = create_test_pool().await.unwrap();

    let issuer = "https://auth.example.com";
    let sub = "user-strict-1";
    let client_id = "client-a";

    let input = CreateIdentityInput {
        login: unique_pack_ref("oidc_strict"),
        display_name: Some("OIDC User".to_string()),
        attributes: json!({
            "oidc": {
                "issuer": issuer,
                "sub": sub,
                "client_id": client_id,
                "email": "user@example.com"
            }
        }),
        password_hash: None,
    };

    let created = IdentityRepository::create(&pool, input).await.unwrap();

    // Strict match succeeds for the same client_id.
    let found = IdentityRepository::find_by_oidc_subject(&pool, issuer, sub, client_id)
        .await
        .unwrap()
        .expect("strict OIDC identity not found");
    assert_eq!(found.id, created.id);

    // A different client_id with the same (issuer, sub) must NOT match — this
    // is the defense-in-depth invariant the strict lookup enforces.
    let other = IdentityRepository::find_by_oidc_subject(&pool, issuer, sub, "client-b")
        .await
        .unwrap();
    assert!(other.is_none());

    // The legacy lookup must also reject this row because client_id is set.
    let legacy = IdentityRepository::find_legacy_oidc_subject(&pool, issuer, sub)
        .await
        .unwrap();
    assert!(legacy.is_none());
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_find_legacy_oidc_subject_matches_rows_without_client_id() {
    let pool = create_test_pool().await.unwrap();

    let issuer = "https://auth.example.com";
    let sub = "user-legacy-1";

    // Simulate a pre-upgrade row: oidc attributes without client_id.
    let input = CreateIdentityInput {
        login: unique_pack_ref("oidc_legacy"),
        display_name: Some("Legacy OIDC User".to_string()),
        attributes: json!({
            "oidc": {
                "issuer": issuer,
                "sub": sub,
                "email": "legacy@example.com"
            }
        }),
        password_hash: None,
    };

    let created = IdentityRepository::create(&pool, input).await.unwrap();

    // Strict lookup must NOT find the legacy row regardless of which
    // client_id the caller supplies.
    let strict = IdentityRepository::find_by_oidc_subject(&pool, issuer, sub, "client-a")
        .await
        .unwrap();
    assert!(strict.is_none());

    // The legacy lookup, however, finds it so the upsert path can upgrade it.
    let legacy = IdentityRepository::find_legacy_oidc_subject(&pool, issuer, sub)
        .await
        .unwrap()
        .expect("legacy OIDC identity not found");
    assert_eq!(legacy.id, created.id);

    // Upgrade the row: stamp client_id onto the attributes and verify that
    // strict lookup now finds it and legacy lookup no longer does.
    let upgraded_attrs = json!({
        "oidc": {
            "issuer": issuer,
            "sub": sub,
            "client_id": "client-a",
            "email": "legacy@example.com"
        }
    });
    IdentityRepository::update(
        &pool,
        created.id,
        UpdateIdentityInput {
            display_name: None,
            password_hash: None,
            attributes: Some(upgraded_attrs),
            frozen: None,
        },
    )
    .await
    .unwrap();

    let strict_after = IdentityRepository::find_by_oidc_subject(&pool, issuer, sub, "client-a")
        .await
        .unwrap()
        .expect("strict lookup must find upgraded row");
    assert_eq!(strict_after.id, created.id);

    let legacy_after = IdentityRepository::find_legacy_oidc_subject(&pool, issuer, sub)
        .await
        .unwrap();
    assert!(legacy_after.is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "integration test — requires database"]
async fn test_upsert_oidc_identity_is_race_safe() {
    // Two concurrent OIDC logins for the same (issuer, sub) but with
    // different client_ids must not produce two identity rows. The partial
    // unique index `uq_identity_oidc_issuer_sub` (added in migration
    // 20250101000013) plus the in-repository transactional retry loop
    // collapse the race to a single winning row.

    let pool = create_test_pool().await.unwrap();

    let issuer = format!("https://auth.example.com/{}", unique_pack_ref("race"));
    let sub = unique_pack_ref("subject");
    let issuer_a = issuer.clone();
    let sub_a = sub.clone();
    let issuer_b = issuer.clone();
    let sub_b = sub.clone();
    let pool_a = pool.clone();
    let pool_b = pool.clone();

    let make_input = |client_id: &str, login_suffix: &str| OidcUpsertInput {
        issuer: issuer.clone(),
        sub: sub.clone(),
        client_id: client_id.to_string(),
        desired_login: format!("{}@example.com", unique_pack_ref(login_suffix)),
        fallback_login: format!("oidc:{}:{}:{}", issuer, sub, client_id),
        display_name: Some(format!("OIDC User ({client_id})")),
        attributes: json!({
            "oidc": {
                "issuer": issuer.clone(),
                "sub": sub.clone(),
                "client_id": client_id,
            }
        }),
    };

    let input_a = make_input("client-a", "race_a");
    let input_b = make_input("client-b", "race_b");

    let task_a =
        tokio::spawn(
            async move { IdentityRepository::upsert_oidc_identity(&pool_a, input_a).await },
        );
    let task_b =
        tokio::spawn(
            async move { IdentityRepository::upsert_oidc_identity(&pool_b, input_b).await },
        );

    let result_a = task_a.await.expect("task_a panicked");
    let result_b = task_b.await.expect("task_b panicked");

    let identity_a = result_a.expect("upsert A failed");
    let identity_b = result_b.expect("upsert B failed");

    // Both calls must observe the same row id — that's the whole point.
    assert_eq!(
        identity_a.id, identity_b.id,
        "concurrent upserts for the same (issuer, sub) must converge on one row"
    );

    // Verify exactly one row exists in the database for this (issuer, sub).
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM identity
         WHERE attributes->'oidc'->>'issuer' = $1
           AND attributes->'oidc'->>'sub' = $2",
    )
    .bind(&issuer_a)
    .bind(&sub_a)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count, 1, "expected exactly one identity row, got {count}");

    // The single row's client_id must be one of the two contenders — not
    // null, not garbage. We don't care which one wins.
    let client_id: Option<String> = sqlx::query_scalar(
        "SELECT attributes->'oidc'->>'client_id' FROM identity
         WHERE attributes->'oidc'->>'issuer' = $1
           AND attributes->'oidc'->>'sub' = $2",
    )
    .bind(&issuer_b)
    .bind(&sub_b)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(
        matches!(client_id.as_deref(), Some("client-a") | Some("client-b")),
        "row's client_id should be one of the two contenders, got {client_id:?}"
    );
}
