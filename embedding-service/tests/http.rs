//! HTTP contract tests. The limit and 404 paths need no model; the embedding path runs only when
//! `EMBEDDING_TEST_BUNDLE` points at a verified bge-small bundle directory.

use std::sync::Arc;

use embedding::OnnxOptions;
use embedding_service::server::{AppState, Limits, load_slot, router};

fn limits() -> Limits {
    Limits {
        max_batch: 4,
        max_text_chars: 20,
        max_inflight: 2,
        query_inflight: 1,
        batch_size: 8,
        intra_threads: Some(1),
        query_lane: true,
    }
}

async fn spawn(state: AppState) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router(Arc::new(state)))
            .await
            .unwrap()
    });
    format!("http://{addr}")
}

#[tokio::test]
async fn limits_and_unknown_slot_without_a_model() {
    let base = spawn(AppState::new(Vec::new(), limits())).await;
    let c = reqwest::Client::new();

    assert_eq!(
        c.get(format!("{base}/health/live"))
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    assert_eq!(
        c.get(format!("{base}/health/ready"))
            .send()
            .await
            .unwrap()
            .status(),
        503
    );

    let too_many = serde_json::json!({ "slot": "x", "purpose": "document", "texts": ["a", "b", "c", "d", "e"] });
    let r = c
        .post(format!("{base}/embed"))
        .json(&too_many)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 413);
    assert_eq!(
        r.json::<serde_json::Value>().await.unwrap()["error"]["code"],
        "batch_too_large"
    );

    let too_long = serde_json::json!({ "slot": "x", "purpose": "query", "texts": ["this text is longer than twenty chars"] });
    let r = c
        .post(format!("{base}/embed"))
        .json(&too_long)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 413);
    assert_eq!(
        r.json::<serde_json::Value>().await.unwrap()["error"]["code"],
        "text_too_long"
    );

    let unknown = serde_json::json!({ "slot": "nope", "purpose": "query", "texts": ["hi"] });
    let r = c
        .post(format!("{base}/embed"))
        .json(&unknown)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 404);
    assert_eq!(
        r.json::<serde_json::Value>().await.unwrap()["error"]["code"],
        "unknown_slot"
    );

    let bad_purpose = serde_json::json!({ "slot": "nope", "purpose": "passage", "texts": ["hi"] });
    assert_eq!(
        c.post(format!("{base}/embed"))
            .json(&bad_purpose)
            .send()
            .await
            .unwrap()
            .status(),
        422
    );

    let info: serde_json::Value = c
        .get(format!("{base}/info"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(info["slots"], serde_json::json!({}));
    assert_eq!(info["limits"]["max_batch"], 4);
}

#[tokio::test]
async fn embeds_with_a_real_bundle() {
    let Ok(dir) = std::env::var("EMBEDDING_TEST_BUNDLE") else {
        eprintln!("EMBEDDING_TEST_BUNDLE not set; skipping");
        return;
    };
    let slot = load_slot(
        std::path::Path::new(&dir),
        OnnxOptions {
            intra_threads: Some(2),
            batch_size: 8,
            query_lane: true,
        },
    )
    .unwrap();
    let slot_id = slot.slot.clone();
    let hash = slot.hash.clone();
    let mut l = limits();
    l.max_text_chars = 8000;
    let base = spawn(AppState::new(vec![slot], l)).await;
    let c = reqwest::Client::new();

    assert_eq!(
        c.get(format!("{base}/health/ready"))
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    let info: serde_json::Value = c
        .get(format!("{base}/info"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(info["slots"][&slot_id]["descriptor_hash"], hash);
    assert_eq!(
        info["slots"][&slot_id]["vector_field"],
        format!("emb_{slot_id}")
    );

    for purpose in ["document", "query"] {
        let body = serde_json::json!({ "slot": slot_id, "purpose": purpose, "texts": ["Bitcoin is a store of value", ""] });
        let r: serde_json::Value = c
            .post(format!("{base}/embed"))
            .json(&body)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(r["slot"], slot_id);
        assert_eq!(r["descriptor_hash"], hash);
        assert_eq!(r["dimensions"], 384);
        assert_eq!(r["vectors"].as_array().unwrap().len(), 2);
        assert_eq!(r["vectors"][0].as_array().unwrap().len(), 384);
    }
    let empty = serde_json::json!({ "slot": slot_id, "purpose": "document", "texts": [] });
    let r: serde_json::Value = c
        .post(format!("{base}/embed"))
        .json(&empty)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(r["vectors"], serde_json::json!([]));
}
