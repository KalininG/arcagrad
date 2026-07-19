//! Read/write pool concurrency checks.

use std::time::Duration;

#[tokio::test]
async fn concurrent_writes_serialize_without_busy() {
    let data = tempfile::tempdir().unwrap();
    let db = arcagrad::server::db::connect(data.path()).await.unwrap();

    let mut tx = db.write.begin().await.unwrap();
    sqlx::query(
        "INSERT INTO jobs (kind, state, attempts, created_at, updated_at, run_after) \
         VALUES ('scan', 'pending', 0, 0, 0, 0)",
    )
    .execute(&mut *tx)
    .await
    .unwrap();

    let mut handles = Vec::new();
    for _ in 0..20 {
        let w = db.write.clone();
        handles.push(tokio::spawn(async move {
            arcagrad::server::jobs::enqueue(&w, "thumbnail_sweep", None).await
        }));
    }
    tokio::time::sleep(Duration::from_millis(200)).await;

    let _during: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM jobs WHERE state = 'pending'")
        .fetch_one(&db.read)
        .await
        .expect("reads must not be blocked by the in-flight writer");

    tx.commit().await.unwrap();

    for h in handles {
        h.await
            .unwrap()
            .expect("concurrent write should queue, not busy-error");
    }

    let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM jobs")
        .fetch_one(&db.read)
        .await
        .unwrap();
    assert_eq!(
        total, 21,
        "1 from the tx + 20 concurrent enqueues all landed"
    );
}

#[tokio::test]
async fn read_pool_rejects_writes() {
    let data = tempfile::tempdir().unwrap();
    let db = arcagrad::server::db::connect(data.path()).await.unwrap();
    let err = sqlx::query(
        "INSERT INTO jobs (kind, state, attempts, created_at, updated_at, run_after) \
         VALUES ('scan', 'pending', 0, 0, 0, 0)",
    )
    .execute(&db.read)
    .await;
    assert!(err.is_err(), "writing through the read pool must fail");
}
