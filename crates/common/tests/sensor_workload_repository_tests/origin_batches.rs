use attune_common::{
    models::Id,
    repositories::{
        managed_sensor_metadata::ManagedSensorMetadataRepository,
        sensor_rule_origin::{
            SensorRuleOriginProof, SensorRuleOriginRepository, MAX_SENSOR_ORIGIN_BATCH,
        },
    },
    Result,
};
use futures::{future::BoxFuture, stream::BoxStream};
use serde_json::json;
use sqlx::{postgres::PgRow, Describe, Either, Execute, Executor, PgPool, Postgres};
use std::sync::atomic::{AtomicUsize, Ordering};

/// Counts executions forwarded to the real SQLx executor, not planned batches.
#[derive(Debug)]
struct Counted<'p> {
    pool: &'p PgPool,
    statements: AtomicUsize,
}

impl<'p> Executor<'p> for &'p Counted<'p> {
    type Database = Postgres;

    fn fetch_many<'e, 'q: 'e, E>(
        self,
        query: E,
    ) -> BoxStream<
        'e,
        std::result::Result<Either<<Postgres as sqlx::Database>::QueryResult, PgRow>, sqlx::Error>,
    >
    where
        E: 'q + Execute<'q, Postgres>,
    {
        self.statements.fetch_add(1, Ordering::Relaxed);
        self.pool.fetch_many(query)
    }

    fn fetch_optional<'e, 'q: 'e, E>(
        self,
        query: E,
    ) -> BoxFuture<'e, std::result::Result<Option<PgRow>, sqlx::Error>>
    where
        E: 'q + Execute<'q, Postgres>,
    {
        self.statements.fetch_add(1, Ordering::Relaxed);
        self.pool.fetch_optional(query)
    }

    fn prepare_with<'e, 'q: 'e>(
        self,
        sql: &'q str,
        parameters: &'e [<Postgres as sqlx::Database>::TypeInfo],
    ) -> BoxFuture<'e, std::result::Result<<Postgres as sqlx::Database>::Statement<'q>, sqlx::Error>>
    {
        self.pool.prepare_with(sql, parameters)
    }

    fn describe<'e, 'q: 'e>(
        self,
        sql: &'q str,
    ) -> BoxFuture<'e, std::result::Result<Describe<Postgres>, sqlx::Error>> {
        self.pool.describe(sql)
    }
}

async fn insert_source(
    pool: &PgPool,
    original: &SensorRuleOriginProof,
    index: usize,
) -> Result<Id> {
    Ok(sqlx::query_scalar("INSERT INTO sensor_rule_config_delivery(generation_binding,rule,rule_ref,pack_ref,trigger_ref,fingerprint,config) VALUES ($1,$2,$3,$4,$5,$6,$7) RETURNING id")
        .bind(original.generation_binding).bind(original.rule_id).bind(&original.rule_ref).bind(&original.pack_ref)
        .bind(&original.trigger_ref).bind(format!("{index:064x}")).bind(json!({"interval":9})).fetch_one(pool).await?)
}

async fn insert_proof(
    pool: &PgPool,
    proof: &SensorRuleOriginProof,
    policy: Option<Id>,
    release: Option<Id>,
    generation: Id,
    corrupt: bool,
) -> Result<()> {
    let mut body = serde_json::to_value(proof)?;
    let hash = super::fixture_digest(&body);
    if corrupt {
        body["private-raw-sentinel"] = json!("must-not-appear-in-errors");
    }
    sqlx::query("INSERT INTO sensor_rule_structural_origin(source_entity,source_id,rule_id,generation_binding,dispatch_policy_revision,dispatch_source_release,proof,proof_digest) VALUES ($1,$2,$3,$4,$5,$6,$7,$8)")
        .bind(&proof.source_entity).bind(proof.source_id).bind(proof.rule_id).bind(generation)
        .bind(policy).bind(release).bind(body).bind(hash).execute(pool).await?;
    Ok(())
}

/// Called by the real foreign managed-rule producer fixture after replacing its
/// current policy. All 100 sources keep the same ORIGINAL retained rule proof.
pub(super) async fn assert_foreign_batch_budgets(
    pool: &PgPool,
    original: &SensorRuleOriginProof,
    other_policy: Id,
    other_release: Id,
) -> Result<()> {
    let (binding, _) =
        attune_common::repositories::sensor_schema::SensorSchemaRepository::find_by_binding_id(
            pool,
            original.generation_binding,
        )
        .await?
        .unwrap();
    let mut keys = Vec::new();
    for index in 1..=100 {
        let mut proof = original.clone();
        proof.source_entity = "sensor_rule_delivery_config".into();
        proof.source_id = insert_source(pool, original, index).await?;
        insert_proof(
            pool,
            &proof,
            proof.dispatch_policy_revision,
            proof.dispatch_source_release,
            proof.generation_binding,
            false,
        )
        .await?;
        keys.push((proof.source_entity.clone(), proof.source_id, proof.rule_id));
    }
    for count in [1, 64, 100] {
        let executor = Counted {
            pool,
            statements: AtomicUsize::new(0),
        };
        let proofs = SensorRuleOriginRepository::find_many(&executor, &keys[..count]).await?;
        assert_eq!(proofs.len(), count);
        assert_eq!(
            executor.statements.load(Ordering::Relaxed),
            1,
            "actual find_many executions for {count} distinct sources"
        );
        for proof in proofs.values() {
            proof.validate_binding(original.generation_binding, &binding)?;
            assert_eq!(proof.managing_pack_id, original.managing_pack_id);
            assert_eq!(
                proof.dispatch_policy_revision,
                original.dispatch_policy_revision
            );
        }
    }
    let executor = Counted {
        pool,
        statements: AtomicUsize::new(0),
    };
    let repeated = vec![keys[0].clone(); 100];
    assert_eq!(
        SensorRuleOriginRepository::find_many(&executor, &repeated)
            .await?
            .len(),
        1
    );
    assert_eq!(executor.statements.load(Ordering::Relaxed), 1);
    let single = SensorRuleOriginRepository::find(pool, &keys[0].0, keys[0].1, keys[0].2)
        .await?
        .unwrap();
    assert_eq!(single.source_id, keys[0].1);

    // The second stored tuple must not leak through an independent source/rule
    // IN predicate when only a different rule at that source was requested.
    let absent = (keys[1].0.clone(), keys[1].1, original.rule_id + 1_000_000);
    let exact =
        SensorRuleOriginRepository::find_many(pool, &[keys[0].clone(), absent.clone()]).await?;
    assert_eq!(exact.len(), 1);
    assert!(!exact.contains_key(&keys[1]));
    assert!(!exact.contains_key(&absent));
    let wrong_entity = ("sensor_generation_rule_config".into(), keys[0].1, keys[0].2);
    assert!(SensorRuleOriginRepository::find_many(pool, &[wrong_entity])
        .await?
        .is_empty());

    let executor = Counted {
        pool,
        statements: AtomicUsize::new(0),
    };
    assert!(SensorRuleOriginRepository::find_many(&executor, &[])
        .await?
        .is_empty());
    assert!(SensorRuleOriginRepository::find_many(
        &executor,
        &vec![keys[0].clone(); MAX_SENSOR_ORIGIN_BATCH + 1]
    )
    .await
    .is_err());
    assert!(
        SensorRuleOriginRepository::find_many(&executor, &[("unknown".into(), 1, 1)])
            .await
            .is_err()
    );
    assert!(
        SensorRuleOriginRepository::find_many(&executor, &[(keys[0].0.clone(), 0, 1)])
            .await
            .is_err()
    );
    assert_eq!(executor.statements.load(Ordering::Relaxed), 0);

    let other_generation:Id=sqlx::query_scalar("INSERT INTO sensor_generation_schema_binding(workload,generation,binding,binding_digest,config,initial_rules) SELECT workload,generation+3000000,binding,binding_digest,config,initial_rules FROM sensor_generation_schema_binding WHERE id=$1 RETURNING id")
        .bind(original.generation_binding).fetch_one(pool).await?;
    for (index, wrong_policy, wrong_release, generation, corrupt, invalid_shape) in [
        (
            201,
            Some(other_policy),
            original.dispatch_source_release,
            original.generation_binding,
            false,
            false,
        ),
        (
            202,
            original.dispatch_policy_revision,
            Some(other_release),
            original.generation_binding,
            false,
            false,
        ),
        (
            203,
            original.dispatch_policy_revision,
            original.dispatch_source_release,
            original.generation_binding,
            true,
            false,
        ),
        (
            204,
            original.dispatch_policy_revision,
            original.dispatch_source_release,
            original.generation_binding,
            false,
            true,
        ),
        (
            205,
            original.dispatch_policy_revision,
            original.dispatch_source_release,
            other_generation,
            false,
            false,
        ),
    ] {
        let mut proof = original.clone();
        proof.source_entity = "sensor_rule_delivery_config".into();
        proof.source_id = insert_source(pool, original, index).await?;
        if invalid_shape {
            proof.dispatch_metadata_digest = Some("bad-shape-sentinel".into());
        }
        insert_proof(
            pool,
            &proof,
            wrong_policy,
            wrong_release,
            generation,
            corrupt,
        )
        .await?;
        let key = (proof.source_entity.clone(), proof.source_id, proof.rule_id);
        for error in [
            SensorRuleOriginRepository::find(pool, &key.0, key.1, key.2)
                .await
                .err()
                .unwrap(),
            SensorRuleOriginRepository::find_many(pool, &[key])
                .await
                .err()
                .unwrap(),
        ] {
            let message = error.to_string();
            assert!(!message.contains("sentinel") && !message.contains("must-not-appear"));
        }
    }
    // Unrequested corrupt rows must not be read by a broad/cross-product query.
    assert_eq!(
        SensorRuleOriginRepository::find_many(pool, &keys[..64])
            .await?
            .len(),
        64
    );
    Ok(())
}

pub(super) async fn assert_binding_batch_budgets(pool: &PgPool, generation: Id) -> Result<()> {
    let mut ids = vec![generation];
    // Retained immutable snapshots can outlive their assignment. No runtime
    // fence or current owner lookup is used for historical evidence preparation.
    for index in 1..100_i64 {
        let id=sqlx::query_scalar("INSERT INTO sensor_generation_schema_binding(workload,generation,binding,binding_digest,config,initial_rules) SELECT workload,generation+$2,binding,binding_digest,config,initial_rules FROM sensor_generation_schema_binding WHERE id=$1 RETURNING id")
            .bind(generation).bind(index+1_000_000).fetch_one(pool).await?;
        ids.push(id);
    }
    for count in [1, 64, 100] {
        let executor = Counted {
            pool,
            statements: AtomicUsize::new(0),
        };
        let bindings =
            ManagedSensorMetadataRepository::prepare_exact_bindings(&executor, &ids[..count])
                .await?;
        assert_eq!(bindings.len(), count);
        assert_eq!(executor.statements.load(Ordering::Relaxed), 1);
    }
    let executor = Counted {
        pool,
        statements: AtomicUsize::new(0),
    };
    assert_eq!(
        ManagedSensorMetadataRepository::prepare_exact_bindings(
            &executor,
            &[generation, generation]
        )
        .await?
        .len(),
        1
    );
    assert_eq!(executor.statements.load(Ordering::Relaxed), 1);
    assert!(
        ManagedSensorMetadataRepository::prepare_exact_bindings(pool, &[i64::MAX])
            .await?
            .is_empty()
    );
    let executor = Counted {
        pool,
        statements: AtomicUsize::new(0),
    };
    assert!(
        ManagedSensorMetadataRepository::prepare_exact_bindings(&executor, &[])
            .await?
            .is_empty()
    );
    assert!(ManagedSensorMetadataRepository::prepare_exact_bindings(
        &executor,
        &vec![generation; MAX_SENSOR_ORIGIN_BATCH + 1]
    )
    .await
    .is_err());
    assert!(
        ManagedSensorMetadataRepository::prepare_exact_bindings(&executor, &[0])
            .await
            .is_err()
    );
    assert_eq!(executor.statements.load(Ordering::Relaxed), 0);
    Ok(())
}

pub(super) async fn assert_metadata_binding(
    pool: &PgPool,
    generation: Id,
    revision: Id,
) -> Result<()> {
    let executor = Counted {
        pool,
        statements: AtomicUsize::new(0),
    };
    let mut prepared = ManagedSensorMetadataRepository::prepare_exact_bindings(
        &executor,
        &[generation, generation],
    )
    .await?;
    assert_eq!(prepared.len(), 1);
    assert_eq!(executor.statements.load(Ordering::Relaxed), 1);
    let prepared = prepared.remove(&generation).unwrap();
    assert_eq!(prepared.binding.metadata_revision, Some(revision));
    let metadata = prepared.metadata.unwrap();
    assert_eq!(metadata.purposes.len(), 3);
    for purpose in ["input", "signing", "cache"] {
        assert!(matches!(&metadata.purposes[purpose],
            attune_common::repositories::managed_sensor_metadata::SensorPurposeAuthority::Declared {..}));
    }
    // A copied snapshot's self-asserted metadata_revision is not a retained pin.
    let missing:Id=sqlx::query_scalar("INSERT INTO sensor_generation_schema_binding(workload,generation,binding,binding_digest,config,initial_rules) SELECT workload,generation+2000000,binding,binding_digest,config,initial_rules FROM sensor_generation_schema_binding WHERE id=$1 RETURNING id")
        .bind(generation).fetch_one(pool).await?;
    assert!(
        ManagedSensorMetadataRepository::prepare_exact_bindings(pool, &[missing])
            .await
            .is_err()
    );
    let corrupt:Id=sqlx::query_scalar("INSERT INTO sensor_generation_schema_binding(workload,generation,binding,binding_digest,config,initial_rules) SELECT workload,generation+2000001,binding || '{\"private-raw-sentinel\":\"do-not-disclose\"}'::jsonb,binding_digest,config,initial_rules FROM sensor_generation_schema_binding WHERE id=$1 RETURNING id")
        .bind(generation).fetch_one(pool).await?;
    let error = ManagedSensorMetadataRepository::prepare_exact_bindings(pool, &[corrupt])
        .await
        .err()
        .unwrap()
        .to_string();
    assert!(!error.contains("sentinel") && !error.contains("do-not-disclose"));
    Ok(())
}
