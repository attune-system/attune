//! Focused execution-batch producer comparison. Full release gates belong to
//! measure_native_workload. Every sample and both wall/p95 ratios are retained.

use anyhow::{ensure, Result};
use attune_common::{
    config::Config,
    models::ExecutionStatus,
    repositories::{
        execution::{CreateExecutionInput, ExecutionRepository},
        Create,
    },
    test_database::TestDatabase,
};
use serde_json::{json, Value};
use sqlx::{Connection, PgPool};
use std::{path::PathBuf, sync::Arc, time::Instant};
use tokio::sync::Barrier;

const WRITERS: usize = 4;
const TRANSACTIONS: usize = 100;
const ROWS: usize = 25;

fn p95(samples: &[f64]) -> f64 {
    let mut sorted = samples.to_vec();
    sorted.sort_by(f64::total_cmp);
    sorted[(sorted.len() * 95).div_ceil(100) - 1]
}

async fn sample(pool: &PgPool, payloads: Arc<Vec<Value>>, tracking: bool) -> Result<Value> {
    sqlx::raw_sql("TRUNCATE execution,execution_history,audit_event,native_summary_invalidation RESTART IDENTITY CASCADE")
        .execute(pool).await?;
    sqlx::query(if tracking {
        "ALTER TABLE execution_history ENABLE TRIGGER native_summary_insert"
    } else {
        "ALTER TABLE execution_history DISABLE TRIGGER native_summary_insert"
    })
    .execute(pool)
    .await?;
    let barrier = Arc::new(Barrier::new(WRITERS + 1));
    let mut tasks = Vec::new();
    for writer in 0..WRITERS {
        let mut connection = pool.acquire().await?;
        let barrier = barrier.clone();
        let payloads = payloads.clone();
        tasks.push(tokio::spawn(async move {
            let mut times = Vec::new();
            let mut errors = Vec::new();
            barrier.wait().await;
            for transaction in 0..TRANSACTIONS {
                let start = Instant::now();
                let outcome = async {
                    let mut tx = connection.begin().await?;
                    for row in 0..ROWS {
                        let i = (writer * TRANSACTIONS + transaction) * ROWS + row;
                        let result = ExecutionRepository::create(&mut *tx, CreateExecutionInput {
                            action_ref: format!("evidence.action_{}", i % 8),
                            config: Some(json!({"host":format!("node-{}",i%100),"limit":100,"tags":["synthetic","test"]})),
                            result: Some(payloads[i].clone()),
                            status: ExecutionStatus::Completed,
                            trace_tag: Some(format!("np.write.{i}")),
                            ..Default::default()
                        }).await;
                        if let Err(error) = result { tx.rollback().await?; return Err(anyhow::Error::from(error)); }
                    }
                    tx.commit().await?;
                    Ok::<_,anyhow::Error>(())
                }.await;
                times.push(start.elapsed().as_secs_f64()*1000.0);
                if let Err(error)=outcome { errors.push(error.to_string()); break; }
            }
            (times, errors)
        }));
    }
    let start = Instant::now();
    barrier.wait().await;
    let mut samples = Vec::new();
    let mut errors = Vec::new();
    for task in tasks {
        match task.await {
            Ok((times, failed)) => {
                samples.extend(times);
                errors.extend(failed);
            }
            Err(error) => errors.push(error.to_string()),
        }
    }
    let wall = start.elapsed().as_secs_f64() * 1000.0;
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM execution")
        .fetch_one(pool)
        .await?;
    let history: i64 = sqlx::query_scalar("SELECT count(*) FROM execution_history")
        .fetch_one(pool)
        .await?;
    let markers: i64 = sqlx::query_scalar("SELECT count(*) FROM native_summary_invalidation")
        .fetch_one(pool)
        .await?;
    let expected = (WRITERS * TRANSACTIONS * ROWS) as i64;
    let correct = errors.is_empty()
        && rows == expected
        && history == expected
        && markers
            == if tracking {
                (WRITERS * TRANSACTIONS) as i64
            } else {
                0
            };
    Ok(
        json!({"tracking":tracking,"wall_ms":wall,"p95_ms":p95(&samples),"samples_ms":samples,"rows":rows,
        "history":history,"markers":markers,"correct":correct,"errors":errors}),
    )
}

async fn diagnostic(pool: &PgPool, payloads: &Arc<Vec<Value>>, name: &str) -> Result<()> {
    sqlx::raw_sql("TRUNCATE execution,execution_history,audit_event,native_summary_invalidation RESTART IDENTITY CASCADE; ALTER TABLE execution_history ENABLE TRIGGER native_summary_insert")
        .execute(pool).await?;
    let mut connection = pool.acquire().await?;
    sqlx::query("SELECT set_config('application_name',$1,false)")
        .bind(format!("nd_path_{name}"))
        .execute(&mut *connection)
        .await?;
    sqlx::raw_sql("LOAD 'auto_explain'; SET auto_explain.log_min_duration=0; SET auto_explain.log_nested_statements=on; SET auto_explain.log_analyze=on; SET auto_explain.log_timing=off; SET auto_explain.log_buffers=on; SET auto_explain.log_format='json'")
        .execute(&mut *connection).await?;
    let mut tx = connection.begin().await?;
    for i in 0..32 {
        ExecutionRepository::create(&mut *tx,CreateExecutionInput {
            action_ref:format!("evidence.action_{}",i%8),
            config:Some(json!({"host":format!("node-{}",i%100),"limit":100,"tags":["synthetic","test"]})),
            result:Some(payloads[i].clone()), status:ExecutionStatus::Completed,
            trace_tag:Some(format!("np.diagnostic.{i}")),..Default::default()
        }).await?;
    }
    tx.rollback().await?;
    // These samples are diagnostics, never acceptance samples. A pooled
    // connection must not carry instrumentation into the timed workload.
    sqlx::raw_sql("SET auto_explain.log_min_duration=-1; SET auto_explain.log_nested_statements=off; SET application_name=''")
        .execute(&mut *connection).await?;
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    ensure!(
        args.len() == 3,
        "expected variant directory and output file"
    );
    let variants = PathBuf::from(&args[1]);
    let output = PathBuf::from(&args[2]);
    ensure!(!output.exists(), "refusing existing evidence");
    let config = Config::load_from_file(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../config.test.yaml"
    ))?;
    let db = TestDatabase::create(&config.database)
        .await?
        .with_cleanup_on_drop();
    let payloads:Vec<Value>=sqlx::query_scalar("SELECT jsonb_build_object('request_id',g,'host','node-'||(g%100)::text,'records',(SELECT string_agg(md5(g::text||':'||b::text),'') FROM generate_series(1,64) b),'labels',jsonb_build_object('region','test-region','attempt',g%3)) FROM generate_series(1,10000) g")
        .fetch_all(db.pool()).await?;
    let payloads = Arc::new(payloads);
    let names: Vec<String> =
        serde_json::from_str(&std::fs::read_to_string(variants.join("variants.json"))?)?;
    ensure!(!names.is_empty(), "expected selected variants");
    let mut evidence = json!({"writers":WRITERS,"transactions":TRANSACTIONS,"rows_per_transaction":ROWS,
        "pairs":3,"durability":"fsync/synchronous_commit unchanged","threshold":1.10,"variants":{}});
    let mut failed = false;
    let diagnostics_only = std::env::var("NATIVE_PRODUCER_DIAGNOSTIC_ONLY").as_deref() == Ok("1");
    evidence["diagnostics_only"] = json!(diagnostics_only);
    for pair in 0..if diagnostics_only { 0 } else { 3 } {
        // Rotate variant order; baseline/treatment order also alternates. No
        // retry or sample exclusion. Restore the actual original between variants.
        for offset in 0..names.len() {
            let name = names[(offset + pair) % names.len()].as_str();
            let ddl = std::fs::read_to_string(variants.join(format!("{name}.sql")))?;
            sqlx::raw_sql(&ddl).execute(db.pool()).await?;
            let order = if pair % 2 == 0 {
                [false, true]
            } else {
                [true, false]
            };
            let mut runs = Vec::new();
            for tracking in order {
                runs.push(sample(db.pool(), payloads.clone(), tracking).await?);
            }
            let baseline = runs.iter().find(|r| r["tracking"] == false).unwrap();
            let treatment = runs.iter().find(|r| r["tracking"] == true).unwrap();
            let wall_ratio =
                treatment["wall_ms"].as_f64().unwrap() / baseline["wall_ms"].as_f64().unwrap();
            let p95_ratio =
                treatment["p95_ms"].as_f64().unwrap() / baseline["p95_ms"].as_f64().unwrap();
            let correct = runs.iter().all(|r| r["correct"] == true);
            let passed = correct && wall_ratio <= 1.10 && p95_ratio <= 1.10;
            failed |= !correct;
            let row = json!({"pair":pair,"correct":correct,"passed":passed,"wall_ratio":wall_ratio,"p95_ratio":p95_ratio,"runs":runs});
            if evidence["variants"].get(name).is_none() {
                evidence["variants"][name] = json!([]);
            }
            evidence["variants"][name].as_array_mut().unwrap().push(row);
            std::fs::write(&output, serde_json::to_string_pretty(&evidence)?)?;
            println!(
                "{name} pair {pair}: wall={wall_ratio:.6}, p95={p95_ratio:.6}, passed={passed}"
            );
        }
    }
    for name in &names {
        sqlx::raw_sql(&std::fs::read_to_string(
            variants.join(format!("{name}.sql")),
        )?)
        .execute(db.pool())
        .await?;
        diagnostic(db.pool(), &payloads, name).await?;
    }
    db.cleanup().await?;
    evidence["cleanup_complete"] = json!(true);
    std::fs::write(output, serde_json::to_string_pretty(&evidence)?)?;
    ensure!(!failed, "producer comparison correctness failure");
    Ok(())
}
