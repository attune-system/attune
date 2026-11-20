mod helpers;
use attune_common::{
    execution_authority::consumer_contract::{
        ConsumerRole, ContractSnapshot, ExpectedMember, ExpectedMembership,
    },
    repositories::recorded_rollout::RecordedRolloutRepository as Repo,
    test_database::TestDatabase,
};
use sqlx::{postgres::PgPoolOptions, PgPool};
use uuid::Uuid;

fn membership() -> ExpectedMembership {
    ExpectedMembership {
        snapshot: ContractSnapshot {
            contract_version: 1,
            catalog_epoch: 1,
            catalog_revision: 0,
            release_digest: "a".repeat(64),
            rollout_generation: Uuid::new_v4(),
        },
        members: [
            ConsumerRole::Api,
            ConsumerRole::Notifier,
            ConsumerRole::Executor,
            ConsumerRole::Worker,
        ]
        .into_iter()
        .map(|role| ExpectedMember {
            role,
            instance: Uuid::new_v4(),
            boot: Uuid::new_v4(),
            required_mask: role.allowed_gate_mask(),
        })
        .collect(),
    }
}
struct Fixture {
    db: TestDatabase,
    pools: Vec<PgPool>,
    roles: Vec<String>,
    manifest: std::path::PathBuf,
}
impl Fixture {
    async fn new(fail_setup: bool) -> Self {
        use futures::FutureExt;
        helpers::init_test_env();
        let config = attune_common::config::Config::load_from_file(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../config.test.yaml"
        ))
        .unwrap();
        let db = TestDatabase::create_before_with_history(
            &config.database,
            "20261119000007_recorded_rollout_membership.sql",
        )
        .await
        .unwrap();
        // Real SQLx upgrade/checksums, never a hand-written migration ledger.

        let manifest =
            std::env::temp_dir().join(format!("ws2-rollout-roles-{}.json", Uuid::new_v4()));
        let mut f = Self {
            db,
            pools: Vec::new(),
            roles: Vec::new(),
            manifest,
        };
        let setup=std::panic::AssertUnwindSafe(async {
            std::fs::write(&f.manifest,"[]").unwrap();
            sqlx::migrate!("../../migrations").run(f.db.pool()).await.unwrap();
            let history=f.db.migration_history().await.unwrap();
            for m in sqlx::migrate!("../../migrations").iter().filter(|m|(20261119000001..=20261119000007).contains(&m.version)) {
                assert_eq!(history.iter().find(|(v,_)|*v==m.version).unwrap().1,m.checksum.as_ref());
            }
            // One suite owns this reserved capability in the disposable lane.
            // An existing role is a provisioning conflict, never reused/deleted.
            let capability:String=sqlx::query_scalar("SELECT recorded_rollout_operator_role()::TEXT").fetch_one(&*f.db).await.unwrap();
            assert!(capability.starts_with("attune_rollout_operator_") && capability.len()<=63 && capability.bytes().all(|b|b.is_ascii_lowercase()||b.is_ascii_digit()||b==b'_'));
            assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM pg_roles WHERE rolname=$1").bind(&capability).fetch_one(&*f.db).await.unwrap(),0);
            sqlx::query(&format!("CREATE ROLE {capability} NOLOGIN")).execute(&*f.db).await.unwrap();
            f.roles.push(capability);
            std::fs::write(&f.manifest,serde_json::to_vec(&f.roles).unwrap()).unwrap();
            assert!(!fail_setup,"injected owned-role setup failure");
            for _ in 0..3 {
                let name=format!("ws2_rollout_{}",Uuid::new_v4().simple());
                assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM pg_roles WHERE rolname=$1").bind(&name).fetch_one(&*f.db).await.unwrap(),0);
                sqlx::query(&format!("CREATE ROLE {name} LOGIN")).execute(&*f.db).await.unwrap();
                f.roles.push(name.clone());
                std::fs::write(&f.manifest,serde_json::to_vec(&f.roles).unwrap()).unwrap();
                sqlx::raw_sql(&format!("GRANT USAGE ON SCHEMA \"{}\" TO {name}; GRANT SELECT ON recorded_rollout_generation,recorded_rollout_active TO {name}; GRANT EXECUTE ON FUNCTION stage_recorded_rollout(JSONB),activate_recorded_rollout(UUID,TEXT,BIGINT,BIGINT) TO {name}",f.db.schema())).execute(&*f.db).await.unwrap();
                let mut url=url::Url::parse(f.db.database_url()).unwrap();url.set_username(&name).unwrap();
                url.query_pairs_mut().append_pair("options",&format!("-csearch_path={},public",f.db.schema()));
                f.pools.push(PgPoolOptions::new().max_connections(4).connect(url.as_str()).await.unwrap());
            }
            sqlx::raw_sql(&format!("GRANT {} TO {}; GRANT {} TO {}",f.roles[0],f.roles[1],f.roles[1],f.roles[3])).execute(&*f.db).await.unwrap();
        }).catch_unwind().await;
        if let Err(panic) = setup {
            f.cleanup().await;
            std::panic::resume_unwind(panic);
        }
        f
    }
    async fn cleanup(self) {
        let mut errors = Vec::new();
        for pool in &self.pools {
            pool.close().await;
        }
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            match sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM pg_stat_activity WHERE usename::TEXT=ANY($1)",
            )
            .bind(&self.roles)
            .fetch_one(&*self.db)
            .await
            {
                Ok(0) => break,
                Ok(n) if tokio::time::Instant::now() >= deadline => {
                    errors.push(format!("{n} owned sessions remain"));
                    break;
                }
                Ok(_) => tokio::time::sleep(std::time::Duration::from_millis(10)).await,
                Err(e) => {
                    errors.push(e.to_string());
                    break;
                }
            }
        }
        for role in self.roles.iter().rev() {
            if let Err(e) = sqlx::raw_sql(&format!("DROP OWNED BY {role}; DROP ROLE {role}"))
                .execute(&*self.db)
                .await
            {
                errors.push(e.to_string());
            }
        }
        let remaining = sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM pg_roles WHERE rolname::TEXT=ANY($1)",
        )
        .bind(&self.roles)
        .fetch_one(&*self.db)
        .await;
        match &remaining {
            Ok(0) => {}
            Ok(n) => errors.push(format!("{n} owned roles remain")),
            Err(e) => errors.push(e.to_string()),
        }
        if let Err(e)=std::fs::write(&self.manifest,serde_json::to_vec(&serde_json::json!({"created":self.roles,"remaining":remaining.ok(),"errors":errors})).unwrap()) {errors.push(e.to_string());}
        if let Err(e) = self.db.cleanup().await {
            errors.push(e.to_string());
        }
        assert!(errors.is_empty(), "owned role cleanup failed: {errors:?}");
    }
}
#[tokio::test]
async fn rollout_repository_real_login_boundaries() {
    use futures::FutureExt;
    let f = Fixture::new(false).await;
    let result = std::panic::AssertUnwindSafe(async {
        malformed_and_conflicting_membership_denies_at_database_boundary(&f).await;
        login_authority_cannot_be_borrowed_or_self_elected(&f).await;
        exact_stage_load_switch_cas_and_rollback(&f).await;
    })
    .catch_unwind()
    .await;
    f.cleanup().await;
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
    // Exercise actual setup-failure cleanup, not just a success-only destructor.
    let failure = std::panic::AssertUnwindSafe(Fixture::new(true))
        .catch_unwind()
        .await;
    assert!(failure.is_err());
}
async fn exact_stage_load_switch_cas_and_rollback(f: &Fixture) {
    assert!(Repo::active(&mut f.pools[1].acquire().await.unwrap())
        .await
        .unwrap()
        .is_none());
    let mut input = membership();
    input.members.sort_by_key(|m| m.instance);
    let a = Repo::stage(&f.pools[0], input.clone()).await.unwrap();
    assert!(!a.active);
    assert_eq!(a.expected, input);
    let mut reordered = input.clone();
    reordered.members.reverse();
    assert_eq!(Repo::stage(&f.pools[0], reordered).await.unwrap(), a);
    let mut changed = input.clone();
    changed.members[0].boot = Uuid::new_v4();
    assert!(Repo::stage(&f.pools[0], changed.clone()).await.is_err());
    changed.snapshot.rollout_generation = Uuid::new_v4();
    let b = Repo::stage(&f.pools[0], changed).await.unwrap();
    assert!(Repo::activate(
        &f.pools[0],
        a.expected.snapshot.rollout_generation,
        "wrong",
        None,
        0
    )
    .await
    .is_err());
    assert!(
        Repo::activate(&f.pools[0], Uuid::new_v4(), &a.membership_digest, None, 0)
            .await
            .is_err()
    );
    Repo::activate(
        &f.pools[0],
        a.expected.snapshot.rollout_generation,
        &a.membership_digest,
        None,
        0,
    )
    .await
    .unwrap();
    let active = Repo::active(&mut f.pools[1].acquire().await.unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(active.rollout.expected, input);
    assert_eq!(active.rollout.membership_digest, a.membership_digest);
    assert_eq!(active.revision, 1);
    assert!(Repo::activate(
        &f.pools[0],
        b.expected.snapshot.rollout_generation,
        &b.membership_digest,
        None,
        0
    )
    .await
    .is_err());
    let mut tx = f.pools[0].begin().await.unwrap();
    sqlx::query("SELECT activate_recorded_rollout($1,$2,$3,$4)")
        .bind(b.expected.snapshot.rollout_generation)
        .bind(&b.membership_digest)
        .bind(a.id)
        .bind(1_i64)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(
        Repo::active(&mut f.pools[1].acquire().await.unwrap())
            .await
            .unwrap()
            .unwrap(),
        active
    );
    let (left, right) = tokio::join!(
        Repo::activate(
            &f.pools[0],
            b.expected.snapshot.rollout_generation,
            &b.membership_digest,
            Some(a.id),
            1
        ),
        Repo::activate(
            &f.pools[0],
            b.expected.snapshot.rollout_generation,
            &b.membership_digest,
            Some(a.id),
            1
        )
    );
    assert_ne!(left.is_ok(), right.is_ok());
    let audit: (String, i32, bool) = sqlx::query_as(
        "SELECT actor::TEXT,backend,activated IS NOT NULL FROM recorded_rollout_active",
    )
    .fetch_one(&*f.db)
    .await
    .unwrap();
    assert_eq!(audit.0, f.roles[1]);
    assert!(audit.1 > 0 && audit.2);
    sqlx::query(&format!(
        "GRANT UPDATE,DELETE ON recorded_rollout_generation TO {}",
        f.roles[1]
    ))
    .execute(&*f.db)
    .await
    .unwrap();
    for stmt in [
        "UPDATE recorded_rollout_generation SET expected=expected",
        "DELETE FROM recorded_rollout_generation",
        "UPDATE recorded_rollout_active SET revision=revision+1",
    ] {
        assert!(sqlx::query(stmt).execute(&f.pools[0]).await.is_err());
        assert!(sqlx::query(stmt).execute(&*f.db).await.is_err());
        assert!(sqlx::query(stmt).execute(&f.pools[1]).await.is_err());
    }
}
async fn login_authority_cannot_be_borrowed_or_self_elected(f: &Fixture) {
    let schema = f.db.schema();
    let mut service = f.pools[1].acquire().await.unwrap();
    for trusted in [schema, "pg_catalog"] {
        assert!(!sqlx::query_scalar::<_, bool>(
            "SELECT has_schema_privilege(session_user,$1,'CREATE')"
        )
        .bind(trusted)
        .fetch_one(&mut *service)
        .await
        .unwrap());
    }
    drop(service);
    let m = membership();
    assert!(
        Repo::stage(&f.db, m.clone()).await.is_err(),
        "definer/table ownership is not direct operator membership"
    );
    for pool in [&f.pools[1], &f.pools[2]] {
        assert!(Repo::stage(pool, m.clone()).await.is_err());
    }
    let mut c = f.pools[2].acquire().await.unwrap();
    sqlx::query(&format!("SET ROLE {}", f.roles[1]))
        .execute(&mut *c)
        .await
        .unwrap();
    assert!(
        !sqlx::query_scalar::<_, bool>("SELECT recorded_rollout_operator()")
            .fetch_one(&mut *c)
            .await
            .unwrap()
    );
    assert!(sqlx::query("SELECT stage_recorded_rollout($1)")
        .bind(serde_json::to_value(&m).unwrap())
        .execute(&mut *c)
        .await
        .is_err());
    sqlx::query("RESET ROLE").execute(&mut *c).await.unwrap();
    drop(c);
    let (a, b) = tokio::join!(
        Repo::stage(&f.pools[0], m.clone()),
        Repo::stage(&f.pools[0], m)
    );
    let staged = a.unwrap();
    assert_eq!(staged, b.unwrap());
    for pool in [&f.pools[1], &f.pools[2]] {
        assert!(Repo::activate(
            pool,
            staged.expected.snapshot.rollout_generation,
            &staged.membership_digest,
            None,
            0
        )
        .await
        .is_err());
    }
    let mut conflicting = staged.expected.clone();
    conflicting.snapshot.rollout_generation = Uuid::new_v4();
    let mut alternate = conflicting.clone();
    alternate.members[0].boot = Uuid::new_v4();
    let (left, right) = tokio::join!(
        Repo::stage(&f.pools[0], conflicting),
        Repo::stage(&f.pools[0], alternate)
    );
    assert_ne!(left.is_ok(), right.is_ok());
}
async fn malformed_and_conflicting_membership_denies_at_database_boundary(f: &Fixture) {
    let original = serde_json::to_value(membership()).unwrap();
    let mut invalid = Vec::new();
    for path in [
        "contract_version",
        "catalog_epoch",
        "catalog_revision",
        "release_digest",
        "rollout_generation",
    ] {
        let mut v = original.clone();
        v["snapshot"][path] = serde_json::json!(null);
        invalid.push(v);
    }
    for (field, value) in [
        ("contract_version", serde_json::json!(2)),
        ("catalog_epoch", serde_json::json!(0)),
        ("catalog_revision", serde_json::json!(-1)),
        ("release_digest", serde_json::json!("z".repeat(64))),
        ("rollout_generation", serde_json::json!(Uuid::nil())),
    ] {
        let mut v = original.clone();
        v["snapshot"][field] = value;
        invalid.push(v);
    }
    for members in [
        serde_json::json!([]),
        serde_json::json!([original["members"][0].clone()]),
    ] {
        let mut v = original.clone();
        v["members"] = members;
        invalid.push(v);
    }
    for field in ["instance", "boot"] {
        let mut v = original.clone();
        v["members"][1][field] = v["members"][0][field].clone();
        invalid.push(v);
        let mut v = original.clone();
        v["members"][0][field] = serde_json::json!(Uuid::nil());
        invalid.push(v);
    }
    for value in [
        serde_json::json!(0),
        serde_json::json!(127),
        serde_json::json!(1),
    ] {
        let mut v = original.clone();
        v["members"][0]["required_mask"] = value;
        invalid.push(v);
    }
    let mut v = original.clone();
    v["members"][0]["role"] = serde_json::json!("sensor");
    invalid.push(v);
    let mut v = original.clone();
    v["members"][0]["instance"] = serde_json::json!({"nested":[[]]});
    invalid.push(v);
    let mut v = original.clone();
    v["extra"] = serde_json::json!("x".repeat(2097153));
    invalid.push(v);
    let mut too_many = original.clone();
    too_many["members"] = serde_json::json!(vec![original["members"][0].clone(); 4097]);
    invalid.push(too_many);
    for value in invalid {
        assert!(sqlx::query("SELECT stage_recorded_rollout($1)")
            .bind(value)
            .execute(&f.pools[0])
            .await
            .is_err());
    }
    assert!(Repo::active(&mut f.pools[1].acquire().await.unwrap())
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM recorded_rollout_generation")
            .fetch_one(&*f.db)
            .await
            .unwrap(),
        0
    );
}
