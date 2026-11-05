use super::*;
use attune_common::repositories::{
    execution_admission::ExecutionAdmissionRepository as Admissions,
    recorded_input_authority::RecordedInputAuthorityRepository as Inputs,
};

// A fixed span lets SQLx traces distinguish the operation from fixture setup.
// It carries neither authority bodies nor SQL bind values.
#[tracing::instrument(level = "debug", skip_all)]
async fn input_collection_probe(
    connection: &mut sqlx::PgConnection,
    execution: Id,
) -> attune_common::Result<Vec<attune_common::delegation::DelegationAuthority>> {
    Admissions::input_authorities(connection, execution, None).await
}

#[tokio::test]
async fn recorded_input_reads_revalidate_local_mutations_and_keep_accepted_human_authority() {
    let (db, fixture) = fixture().await;
    let execution = fixture.credential.execution_id;
    let mut tx = db.begin().await.unwrap();
    Inputs::capture_human(&mut tx, "execution", execution, fixture.human_id)
        .await
        .unwrap();
    IdentityRepository::update(
        &mut *tx,
        fixture.human_id,
        UpdateIdentityInput {
            frozen: Some(true),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let authorities = input_collection_probe(&mut tx, execution).await.unwrap();
    assert_eq!(authorities.len(), 1);
    assert_eq!(authorities[0].identity_id, fixture.human_id);
    let single = Admissions::input_authority(&mut tx, execution, None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(single.identity_id, authorities[0].identity_id);

    for case in ["admission", "policy", "sponsor", "terminal", "approval"] {
        sqlx::query("SAVEPOINT input_mutation")
            .execute(&mut *tx)
            .await
            .unwrap();
        match case {
            "admission" => {
                RuntimeAuthorityRepository::revoke_admission(&mut tx, fixture.admission.id)
                    .await
                    .unwrap();
            }
            "policy" => {
                RuntimeAuthorityRepository::revoke_policy(&mut tx, fixture.policy.id)
                    .await
                    .unwrap();
            }
            "sponsor" => {
                IdentityRepository::update(
                    &mut *tx,
                    fixture.credential.sponsor_identity_id,
                    UpdateIdentityInput {
                        frozen: Some(true),
                        ..Default::default()
                    },
                )
                .await
                .unwrap();
            }
            "terminal" => {
                ExecutionRepository::update(
                    &mut *tx,
                    execution,
                    UpdateExecutionInput {
                        status: Some(ExecutionStatus::Completed),
                        ..Default::default()
                    },
                )
                .await
                .unwrap();
            }
            "approval" => {
                RuntimeAuthorityRepository::revoke_approval(&mut tx, fixture.approval.id)
                    .await
                    .unwrap();
            }
            _ => unreachable!(),
        }
        assert!(
            matches!(
                Admissions::input_authorities(&mut tx, execution, None).await,
                Err(attune_common::Error::PermissionDenied(_))
            ),
            "collection must observe local {case} mutation"
        );
        assert!(
            matches!(
                Admissions::input_authority(&mut tx, execution, None).await,
                Err(attune_common::Error::PermissionDenied(_))
            ),
            "single authority must observe local {case} mutation"
        );
        sqlx::query("ROLLBACK TO SAVEPOINT input_mutation")
            .execute(&mut *tx)
            .await
            .unwrap();
    }
    tx.rollback().await.unwrap();
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn recorded_input_reads_reject_approval_expiring_during_snapshot_wait() {
    for collection in [false, true] {
        let (db, fixture) =
            fixture_with_approval_lifetime(None, None, Some(Duration::seconds(30))).await;
        let execution = fixture.credential.execution_id;
        let expiry = fixture.approval.expires_at.unwrap();
        let mut setup = db.begin().await.unwrap();
        Inputs::capture_human(&mut setup, "execution", execution, fixture.human_id)
            .await
            .unwrap();
        setup.commit().await.unwrap();

        let mut blocker = db.begin().await.unwrap();
        sqlx::query("SELECT owner_id FROM recorded_execution_input_authority WHERE owner_kind='execution' AND owner_id=$1 FOR UPDATE")
            .bind(execution)
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let blocker_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let mut waiter = db.begin().await.unwrap();
        let waiter_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *waiter)
            .await
            .unwrap();

        let read = async {
            tokio::time::timeout(std::time::Duration::from_secs(45), async {
                if collection {
                    Admissions::input_authorities(&mut waiter, execution, None)
                        .await
                        .map(|_| ())
                } else {
                    Admissions::input_authority(&mut waiter, execution, None)
                        .await
                        .map(|_| ())
                }
            })
            .await
        };
        let release = async {
            let observation = tokio::time::timeout(std::time::Duration::from_secs(40), async {
                let mut observed_wait = false;
                loop {
                    let (blocked, expired): (bool, bool) = sqlx::query_as(
                        "SELECT $2 = ANY(pg_blocking_pids($1)), clock_timestamp() >= $3",
                    )
                    .bind(waiter_pid)
                    .bind(blocker_pid)
                    .bind(expiry)
                    .fetch_one(&*db)
                    .await?;
                    if blocked && !observed_wait {
                        if expired {
                            return Ok::<_, sqlx::Error>(false);
                        }
                        observed_wait = true;
                    }
                    if observed_wait && blocked && expired {
                        return Ok(true);
                    }
                    // Poll a readiness/expiry predicate, never assume a sleep
                    // established the actual target-lock wait.
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
            })
            .await;
            let rollback = blocker.rollback().await;
            (observation, rollback)
        };
        let (result, (observation, rollback)) = tokio::join!(read, release);
        let waiter_cleanup = waiter.rollback().await;
        let cleanup = db.cleanup().await;
        rollback.unwrap();
        waiter_cleanup.unwrap();
        cleanup.unwrap();
        assert!(
            observation.unwrap().unwrap(),
            "observe an unexpired snapshot wait"
        );
        assert!(
            matches!(
                result.unwrap(),
                Err(attune_common::Error::PermissionDenied(message)) if message.contains("expired")
            ),
            "input delivery must recheck expiry after its snapshot wait (collection={collection})"
        );
    }
}
