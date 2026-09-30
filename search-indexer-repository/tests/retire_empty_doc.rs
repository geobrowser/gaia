//! `RetireEmptyDoc` against a real OpenSearch (GEO-2548).
//!
//! Ignored by default because it needs a server. Run it with:
//!
//! ```text
//! docker run -d --name os -p 9298:9200 -e discovery.type=single-node \
//!   -e DISABLE_SECURITY_PLUGIN=true -e DISABLE_INSTALL_DEMO_CONFIG=true \
//!   opensearchproject/opensearch:2.19.1
//! OPENSEARCH_TEST_URL=http://localhost:9298 \
//!   cargo test -p search-indexer-repository --test retire_empty_doc -- --ignored
//! ```

use opensearch::http::transport::Transport;
use opensearch::indices::{IndicesCreateParts, IndicesDeleteParts, IndicesRefreshParts};
use opensearch::{GetParts, OpenSearch};
use search_indexer_repository::opensearch::{get_index_settings, IndexConfig};
use search_indexer_repository::{
    EntityOperation, OpenSearchProvider, RetireEmptyDocRequest, SearchIndexProvider,
    UpdateEntityRequest,
};
use serde_json::json;
use uuid::Uuid;

fn upsert(entity_id: Uuid, space_id: Uuid) -> UpdateEntityRequest {
    UpdateEntityRequest {
        entity_id: entity_id.to_string(),
        space_id: space_id.to_string(),
        name: None,
        description: None,
        avatar: None,
        cover: None,
        image_url: None,
        add_relation: None,
        entity_global_score: None,
        space_score: None,
        entity_space_score: None,
        deleted: None,
        space_topic_entity_id: None,
        in_canonical_graph: Some(true),
    }
}

fn retire(entity_id: Uuid, space_id: Uuid) -> EntityOperation {
    EntityOperation::RetireEmptyDoc(RetireEmptyDocRequest {
        doc_id: format!("{entity_id}_{space_id}"),
    })
}

async fn exists(client: &OpenSearch, index: &str, entity_id: Uuid, space_id: Uuid) -> bool {
    let id = format!("{entity_id}_{space_id}");
    let response = client
        .get(GetParts::IndexId(index, &id))
        .send()
        .await
        .expect("get");
    response.status_code().as_u16() == 200
}

#[tokio::test]
#[ignore = "needs OPENSEARCH_TEST_URL"]
async fn retires_empty_docs_but_keeps_tombstones_and_topic_stubs() {
    let url = std::env::var("OPENSEARCH_TEST_URL").expect("OPENSEARCH_TEST_URL");
    let alias = format!("retire_test_{}", Uuid::new_v4().simple());
    let client = OpenSearch::new(Transport::single_node(&url).expect("transport"));
    // The real mapping, created the way search-admin creates it, with the alias on top.
    let mut body = get_index_settings(Some(1));
    body["aliases"] = json!({ alias.clone(): {} });
    let created = client
        .indices()
        .create(IndicesCreateParts::Index(&format!("{alias}_v1")))
        .body(body)
        .send()
        .await
        .expect("create index");
    assert!(created.status_code().is_success(), "create index");
    let provider = OpenSearchProvider::new(&url, IndexConfig::new(alias.clone(), 1))
        .await
        .expect("provider");

    let space = Uuid::new_v4();
    let (emptied, tombstone, stub, missing) = (
        Uuid::new_v4(),
        Uuid::new_v4(),
        Uuid::new_v4(),
        Uuid::new_v4(),
    );

    let mut emptied_doc = upsert(emptied, space);
    emptied_doc.name = Some(String::new());
    let mut tombstone_doc = upsert(tombstone, space);
    tombstone_doc.deleted = Some(true);
    let mut stub_doc = upsert(stub, space);
    stub_doc.space_topic_entity_id = Some(stub.to_string());
    let summary = provider
        .bulk_operations(&[
            EntityOperation::Update(Box::new(emptied_doc)),
            EntityOperation::Update(Box::new(tombstone_doc)),
            EntityOperation::Update(Box::new(stub_doc)),
        ])
        .await
        .expect("seed");
    assert_eq!(summary.failed, 0, "{summary:?}");

    let summary = provider
        .bulk_operations(&[
            retire(emptied, space),
            retire(tombstone, space),
            retire(stub, space),
            retire(missing, space),
        ])
        .await
        .expect("retire");
    // Deleted, two no-ops, and a 404 for the doc that was never there: none is a failure,
    // which would NACK the batch.
    assert_eq!(summary.failed, 0, "{summary:?}");
    assert_eq!(summary.succeeded, 4);

    assert!(
        !exists(&client, &alias, emptied, space).await,
        "emptied doc"
    );
    assert!(exists(&client, &alias, tombstone, space).await, "tombstone");
    assert!(exists(&client, &alias, stub, space).await, "topic stub");
    assert!(!exists(&client, &alias, missing, space).await, "no ghost");

    // A retired entity that gains a value again is simply recreated: no tombstone is left
    // behind to swallow the update.
    let mut back = upsert(emptied, space);
    back.name = Some("Back again".to_string());
    let summary = provider
        .bulk_operations(&[EntityOperation::Update(Box::new(back))])
        .await
        .expect("recreate");
    assert_eq!(summary.failed, 0);
    client
        .indices()
        .refresh(IndicesRefreshParts::Index(&[&alias]))
        .send()
        .await
        .expect("refresh");
    assert!(exists(&client, &alias, emptied, space).await, "recreated");

    let _ = client
        .indices()
        .delete(IndicesDeleteParts::Index(&[&format!("{alias}_v1")]))
        .send()
        .await;
}
