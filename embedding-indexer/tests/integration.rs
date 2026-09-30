//! End-to-end against a live OpenSearch and embedding-service. Skipped unless
//! `EMBED_TEST_OPENSEARCH_URL` and `EMBED_TEST_SERVICE_URL` are set (CI sets both; locally the
//! compose OpenSearch and a running `embedding-service serve` do). `EMBED_TEST_SLOT` defaults to
//! the bge-small bundle's slot.

use embedding::{Descriptor, slots};
use embedding_indexer::{Config, Engine};
use serde_json::{Value, json};

const TYPES: &str = embedding_indexer::scope::TYPES_RELATION_TYPE;
const T1: &str = "00000000-0000-0000-0000-000000000b01";
const T2: &str = "00000000-0000-0000-0000-000000000b02";
const SPACE: &str = "00000000-0000-4000-8000-000000000001";

struct Env {
    os: String,
    service: String,
    slot: String,
    http: reqwest::Client,
}

impl Env {
    fn from_env() -> Option<Self> {
        let os = std::env::var("EMBED_TEST_OPENSEARCH_URL").ok()?;
        let service = std::env::var("EMBED_TEST_SERVICE_URL").ok()?;
        Some(Self {
            os: os.trim_end_matches('/').to_string(),
            service: service.trim_end_matches('/').to_string(),
            slot: std::env::var("EMBED_TEST_SLOT").unwrap_or_else(|_| "79502860cd".into()),
            http: reqwest::Client::new(),
        })
    }
    async fn put(&self, path: &str, body: Value) -> Value {
        self.http
            .put(format!("{}{path}", self.os))
            .json(&body)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap()
    }
    async fn post(&self, path: &str, body: Value) -> Value {
        self.http
            .post(format!("{}{path}", self.os))
            .json(&body)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap()
    }
    async fn get(&self, path: &str) -> Value {
        self.http
            .get(format!("{}{path}", self.os))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap()
    }
    async fn delete(&self, path: &str) {
        let _ = self.http.delete(format!("{}{path}", self.os)).send().await;
    }
}

fn doc(
    entity: &str,
    name: Option<&str>,
    description: Option<&str>,
    deleted: bool,
    types: &[&str],
) -> Value {
    let rels: Vec<Value> = types
        .iter()
        .map(|t| json!({ "relation_id": format!("r-{t}"), "relation_type": TYPES, "to_entity_id": t }))
        .collect();
    let mut d = json!({ "entity_id": entity, "space_id": SPACE, "relations": rels, "indexed_at": chrono_now() });
    if let Some(n) = name {
        d["name"] = json!(n);
        d["name_raw"] = json!(n);
    }
    if let Some(x) = description {
        d["description"] = json!(x);
    }
    if deleted {
        d["deleted"] = json!(true);
    }
    d
}

fn chrono_now() -> String {
    // RFC 3339 with ms, as search-indexer stamps it.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap();
    let secs = now.as_secs();
    let ms = now.subsec_millis();
    let dt = time_from_unix(secs);
    format!("{dt}.{ms:03}Z")
}

fn time_from_unix(secs: u64) -> String {
    // minimal UTC formatter (no chrono in dev-deps): days since epoch → civil date
    let days = secs / 86_400;
    let rem = secs % 86_400;
    let (h, m, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mth = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if mth <= 2 { y + 1 } else { y };
    format!("{y:04}-{mth:02}-{d:02}T{h:02}:{m:02}:{s:02}")
}

fn config(env: &Env, index: &str, control: &str, types: &[&str]) -> Config {
    Config {
        opensearch_url: env.os.clone(),
        index_alias: "entities".into(),
        index: Some(index.into()),
        control_index: Some(control.into()),
        service_url: env.service.clone(),
        slot: env.slot.clone(),
        poll_interval_ms: 10,
        overlap_s: 30,
        page_size: 2, // several pages for four documents: exercises search_after
        batch_size: 8,
        max_docs_per_cycle: 1000,
        scope_type_ids: types.iter().map(|s| s.to_string()).collect(),
        scope_space_ids: vec![],
        skip_deleted: true,
        lru_size: 100,
        backfill_backend: "service".into(),
        once: true,
        health_port: 0,
    }
}

async fn slot_fields(env: &Env, entity: &str, vf: &str) -> (Option<String>, bool) {
    let r = env
        .get(&format!("/{}/_doc/{entity}_{SPACE}", env_index()))
        .await;
    let src = &r["_source"];
    (
        src[slots::src_hash_field(vf)].as_str().map(str::to_string),
        src.get(vf).is_some(),
    )
}

fn env_index() -> String {
    std::env::var("EMBED_TEST_INDEX_NAME").unwrap()
}

#[tokio::test]
async fn backfill_follow_and_cleanup_against_live_stack() {
    let Some(env) = Env::from_env() else {
        eprintln!("EMBED_TEST_OPENSEARCH_URL / EMBED_TEST_SERVICE_URL not set; skipping");
        return;
    };
    let nonce = format!("{:x}", rand::random::<u64>());
    let index = format!("embed_it_{nonce}");
    let control = format!("embed_it_control_{nonce}");
    // Share the index name with the helper through the environment (single test in this file).
    unsafe { std::env::set_var("EMBED_TEST_INDEX_NAME", &index) };

    // Descriptor from the running service; index shaped like gaia's, with the slot registered.
    let info = env
        .http
        .get(format!("{}/info", env.service))
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    let descriptor: Descriptor =
        serde_json::from_value(info["slots"][&env.slot]["descriptor"].clone())
            .expect("service descriptor");
    assert_eq!(descriptor.slot_id(), env.slot);
    let vf = descriptor.vector_field();
    let mut props = json!({
        "entity_id": { "type": "keyword" }, "space_id": { "type": "keyword" },
        "name": { "type": "text" }, "name_raw": { "type": "keyword" }, "description": { "type": "text" },
        "deleted": { "type": "boolean" }, "indexed_at": { "type": "date" },
        "relations": { "type": "nested", "properties": { "relation_id": { "type": "keyword" }, "relation_type": { "type": "keyword" }, "to_entity_id": { "type": "keyword" } } }
    });
    for (k, v) in slots::slot_field_mappings(&descriptor) {
        props[k] = v;
    }
    let created = env
        .put(&format!("/{index}"), json!({ "settings": { "index.knn": true, "number_of_shards": 1, "number_of_replicas": 0 },
            "mappings": { "_meta": slots::meta_with_slot(None, &descriptor, true), "properties": props } }))
        .await;
    assert_eq!(created["acknowledged"], true, "{created}");

    // A: in scope. B: nameless. C: deleted. D: wrong type. E: named, no types (out under the allowlist).
    for (e, d) in [
        (
            "a",
            doc("a", Some("Bitcoin is a store of value"), None, false, &[T1]),
        ),
        ("b", doc("b", None, Some("no name"), false, &[T1])),
        ("c", doc("c", Some("deleted claim"), None, true, &[T1])),
        (
            "d",
            doc("d", Some("Gold is a store of value"), None, false, &[T2]),
        ),
        ("e", doc("e", Some("untyped"), None, false, &[])),
    ] {
        env.put(&format!("/{index}/_doc/{e}_{SPACE}?refresh=true"), d)
            .await;
    }

    // 1. backfill + first follow pass
    let mut engine = Engine::start(config(&env, &index, &control, &[T1]))
        .await
        .expect("engine starts");
    let cycles = engine.run_once().await.expect("run once");
    let total_embedded: usize = cycles.iter().map(|c| c.embedded).sum();
    assert_eq!(total_embedded, 1, "only A is in scope: {cycles:?}");
    assert!(cycles.iter().any(|c| c.backfill_complete));
    env.post(&format!("/{index}/_refresh"), json!({})).await;
    let (a_hash, a_vec) = slot_fields(&env, "a", &vf).await;
    assert!(a_vec && a_hash.is_some(), "A has vector and hash");
    for e in ["b", "c", "d", "e"] {
        let (h, v) = slot_fields(&env, e, &vf).await;
        assert!(!v && h.is_none(), "{e} must have no slot fields");
    }

    // 2. A's name changes (search-indexer would stamp indexed_at): follow picks it up, hash changes
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    env.post(
        &format!("/{index}/_update/a_{SPACE}?refresh=true"),
        json!({ "doc": { "name": "Bitcoin is digital gold", "indexed_at": chrono_now() } }),
    )
    .await;
    let cycles = engine.run_once().await.unwrap();
    assert_eq!(
        cycles.iter().map(|c| c.embedded).sum::<usize>(),
        1,
        "{cycles:?}"
    );
    env.post(&format!("/{index}/_refresh"), json!({})).await;
    let (a_hash2, _) = slot_fields(&env, "a", &vf).await;
    assert_ne!(a_hash, a_hash2);
    assert_eq!(
        a_hash2.as_deref(),
        Some(embedding::template::content_hash("Bitcoin is digital gold").as_str())
    );

    // 3. unchanged text → nothing re-embedded
    let cycles = engine.run_once().await.unwrap();
    assert_eq!(cycles.iter().map(|c| c.embedded).sum::<usize>(), 0);
    assert!(cycles.iter().map(|c| c.unchanged).sum::<usize>() >= 1);

    // 4. name unset → out of scope → slot fields removed
    env.post(&format!("/{index}/_update/a_{SPACE}?refresh=true"), json!({ "script": { "source": "ctx._source.remove('name'); ctx._source.indexed_at = params.now", "params": { "now": chrono_now() } } })).await;
    let cycles = engine.run_once().await.unwrap();
    assert_eq!(
        cycles.iter().map(|c| c.removed).sum::<usize>(),
        1,
        "{cycles:?}"
    );
    env.post(&format!("/{index}/_refresh"), json!({})).await;
    let (h, v) = slot_fields(&env, "a", &vf).await;
    assert!(
        !v && h.is_none(),
        "A's slot fields removed after name unset"
    );

    // 5. the vector is queryable through the knn clause with a filter
    let q = env
        .http
        .post(format!("{}/embed", env.service))
        .json(&json!({ "slot": env.slot, "purpose": "query", "texts": ["store of value"] }))
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    env.post(&format!("/{index}/_update/d_{SPACE}?refresh=true"), json!({ "doc": { "relations": [{ "relation_id": "r", "relation_type": TYPES, "to_entity_id": T1 }], "indexed_at": chrono_now() } })).await;
    engine.run_once().await.unwrap(); // D is now in scope
    env.post(&format!("/{index}/_refresh"), json!({})).await;
    let hits = env.post(&format!("/{index}/_search"), json!({ "size": 5, "_source": ["entity_id"], "query": { "knn": { vf.clone(): { "vector": q["vectors"][0], "k": 5, "method_parameters": { "ef_search": 100 }, "filter": { "term": { "space_id": SPACE } } } } } })).await;
    let ids: Vec<&str> = hits["hits"]["hits"]
        .as_array()
        .unwrap()
        .iter()
        .map(|h| h["_source"]["entity_id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, vec!["d"], "only D has a vector now: {hits}");

    env.delete(&format!("/{index}")).await;
    env.delete(&format!("/{control}")).await;
}
