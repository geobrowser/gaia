//! Integration test for `get_space_topic_mappings()`.
//!
//! Requires a running OpenSearch instance at localhost:9200.
//! Run with: cargo test -p search-indexer-repository --test space_topic_cache_integration -- --ignored

use opensearch::{
    http::transport::{SingleNodeConnectionPool, TransportBuilder},
    indices::{IndicesCreateParts, IndicesDeleteParts, IndicesRefreshParts},
    IndexParts, OpenSearch,
};
use search_indexer_repository::opensearch::IndexConfig;
use search_indexer_repository::OpenSearchProvider;
use serde_json::json;
use url::Url;
use uuid::Uuid;

/// Create a raw OpenSearch client for test setup/teardown.
async fn create_raw_client() -> OpenSearch {
    let url = Url::parse("http://localhost:9200").unwrap();
    let conn_pool = SingleNodeConnectionPool::new(url);
    let transport = TransportBuilder::new(conn_pool)
        .disable_proxy()
        .build()
        .unwrap();
    OpenSearch::new(transport)
}

/// Create the test index with minimal mappings.
async fn create_test_index(client: &OpenSearch, index_name: &str) {
    let body = json!({
        "settings": {
            "number_of_shards": 1,
            "number_of_replicas": 0
        },
        "mappings": {
            "properties": {
                "entity_id": { "type": "keyword" },
                "space_id": { "type": "keyword" },
                "space_topic_entity_id": { "type": "keyword" },
                "name": { "type": "text" }
            }
        }
    });

    let response = client
        .indices()
        .create(IndicesCreateParts::Index(index_name))
        .body(body)
        .send()
        .await
        .unwrap();

    assert!(
        response.status_code().is_success(),
        "Failed to create test index: {}",
        response.text().await.unwrap_or_default()
    );
}

/// Index a single test document.
async fn index_doc(
    client: &OpenSearch,
    index_name: &str,
    doc_id: &str,
    entity_id: &Uuid,
    space_id: &Uuid,
    space_topic_entity_id: Option<&Uuid>,
    name: Option<&str>,
) {
    let mut doc = json!({
        "entity_id": entity_id.to_string(),
        "space_id": space_id.to_string(),
    });

    if let Some(topic_id) = space_topic_entity_id {
        doc["space_topic_entity_id"] = json!(topic_id.to_string());
    }
    if let Some(name) = name {
        doc["name"] = json!(name);
    }

    let response = client
        .index(IndexParts::IndexId(index_name, doc_id))
        .body(doc)
        .send()
        .await
        .unwrap();

    assert!(
        response.status_code().is_success(),
        "Failed to index doc {}: {}",
        doc_id,
        response.text().await.unwrap_or_default()
    );
}

/// Delete the test index.
async fn delete_test_index(client: &OpenSearch, index_name: &str) {
    let _ = client
        .indices()
        .delete(IndicesDeleteParts::Index(&[index_name]))
        .send()
        .await;
}

/// Refresh the index so all documents are searchable.
async fn refresh_index(client: &OpenSearch, index_name: &str) {
    client
        .indices()
        .refresh(IndicesRefreshParts::Index(&[index_name]))
        .send()
        .await
        .unwrap();
}

/// Test that `get_space_topic_mappings()` returns only spaces that have
/// `space_topic_entity_id` set, with mixed and null data in the index.
///
/// Test data layout:
///
/// | Doc | Space   | Entity  | space_topic_entity_id | Notes                          |
/// |-----|---------|---------|-----------------------|--------------------------------|
/// | 1   | Space A | Ent 1   | Topic X               | Space with topic, 2 docs       |
/// | 2   | Space A | Ent 2   | Topic X               | Same space, same topic         |
/// | 3   | Space B | Ent 3   | Topic Y               | Different space, different topic|
/// | 4   | Space C | Ent 4   | Topic Z               | Third space with topic         |
/// | 5   | Space D | Ent 5   | (none)                | Space with NO topic            |
/// | 6   | Space D | Ent 6   | (none)                | Same space, still no topic     |
/// | 7   | Space E | Ent 7   | (none)                | Another space with no topic    |
///
/// Expected result: 3 mappings (A→X, B→Y, C→Z). Spaces D and E excluded.
#[tokio::test]
#[ignore]
async fn test_get_space_topic_mappings_with_mixed_data() {
    let test_id = Uuid::new_v4().to_string()[..8].to_string();
    let index_alias = format!("test_space_topic_{}", test_id);
    let index_name = format!("{}_v0", index_alias);

    let client = create_raw_client().await;

    // Cleanup in case of a previous failed run
    delete_test_index(&client, &index_name).await;

    // Create test index directly (not via alias, to keep it simple)
    create_test_index(&client, &index_name).await;

    // Define test UUIDs
    let space_a = Uuid::new_v4();
    let space_b = Uuid::new_v4();
    let space_c = Uuid::new_v4();
    let space_d = Uuid::new_v4();
    let space_e = Uuid::new_v4();

    let topic_x = Uuid::new_v4();
    let topic_y = Uuid::new_v4();
    let topic_z = Uuid::new_v4();

    let ent1 = Uuid::new_v4();
    let ent2 = Uuid::new_v4();
    let ent3 = Uuid::new_v4();
    let ent4 = Uuid::new_v4();
    let ent5 = Uuid::new_v4();
    let ent6 = Uuid::new_v4();
    let ent7 = Uuid::new_v4();

    // Index documents with mixed space_topic_entity_id values
    // Space A: 2 docs WITH topic
    index_doc(&client, &index_name, &format!("{}_{}", ent1, space_a), &ent1, &space_a, Some(&topic_x), Some("Entity 1")).await;
    index_doc(&client, &index_name, &format!("{}_{}", ent2, space_a), &ent2, &space_a, Some(&topic_x), Some("Entity 2")).await;

    // Space B: 1 doc WITH topic
    index_doc(&client, &index_name, &format!("{}_{}", ent3, space_b), &ent3, &space_b, Some(&topic_y), Some("Entity 3")).await;

    // Space C: 1 doc WITH topic
    index_doc(&client, &index_name, &format!("{}_{}", ent4, space_c), &ent4, &space_c, Some(&topic_z), None).await;

    // Space D: 2 docs WITHOUT topic (null)
    index_doc(&client, &index_name, &format!("{}_{}", ent5, space_d), &ent5, &space_d, None, Some("Entity 5")).await;
    index_doc(&client, &index_name, &format!("{}_{}", ent6, space_d), &ent6, &space_d, None, Some("Entity 6")).await;

    // Space E: 1 doc WITHOUT topic (null)
    index_doc(&client, &index_name, &format!("{}_{}", ent7, space_e), &ent7, &space_e, None, None).await;

    // Refresh so all docs are searchable
    refresh_index(&client, &index_name).await;

    // Create the provider pointing at the raw index name (used as alias)
    let config = IndexConfig::new(&index_name, 0);
    let provider = OpenSearchProvider::new("http://localhost:9200", config)
        .await
        .expect("Failed to create OpenSearchProvider");

    // Call the method under test
    let mappings = provider
        .get_space_topic_mappings()
        .await
        .expect("get_space_topic_mappings failed");

    // Verify: exactly 3 spaces with topics
    assert_eq!(
        mappings.len(),
        3,
        "Expected 3 space→topic mappings, got {}: {:?}",
        mappings.len(),
        mappings
    );

    // Verify each mapping
    assert_eq!(
        mappings.get(&space_a),
        Some(&topic_x),
        "Space A should map to Topic X"
    );
    assert_eq!(
        mappings.get(&space_b),
        Some(&topic_y),
        "Space B should map to Topic Y"
    );
    assert_eq!(
        mappings.get(&space_c),
        Some(&topic_z),
        "Space C should map to Topic Z"
    );

    // Verify spaces without topics are NOT in the result
    assert!(
        !mappings.contains_key(&space_d),
        "Space D (no topic) should not be in mappings"
    );
    assert!(
        !mappings.contains_key(&space_e),
        "Space E (no topic) should not be in mappings"
    );

    // Cleanup
    delete_test_index(&client, &index_name).await;
}
