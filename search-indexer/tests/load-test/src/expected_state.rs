use std::collections::HashMap;
use uuid::Uuid;

/// A relation stored on a document.
#[derive(Debug, Clone, PartialEq)]
pub struct ExpectedRelation {
    pub relation_id: Uuid,
    pub relation_type: Uuid,
    pub to_entity_id: Uuid,
}

/// The expected state of a single OpenSearch document keyed by (entity_id, space_id).
#[derive(Debug, Clone)]
pub struct ExpectedDocument {
    pub entity_id: Uuid,
    pub space_id: Uuid,
    pub name: Option<String>,
    pub description: Option<String>,
    pub image_url: Option<String>,
    pub deleted: bool,
    pub relations: Vec<ExpectedRelation>,
    pub entity_global_score: Option<f64>,
    pub space_score: Option<f64>,
    pub entity_space_score: Option<f64>,
    pub space_topic_entity_id: Option<Uuid>,
}

impl ExpectedDocument {
    pub fn new(entity_id: Uuid, space_id: Uuid) -> Self {
        Self {
            entity_id,
            space_id,
            name: None,
            description: None,
            image_url: None,
            deleted: false,
            relations: Vec::new(),
            entity_global_score: None,
            space_score: None,
            entity_space_score: None,
            space_topic_entity_id: None,
        }
    }
}

/// In-memory model of the correct final OpenSearch state.
///
/// Methods mirror the indexer's tombstone semantics so that after replaying all
/// events in order, the expected state matches what OpenSearch should contain.
pub struct ExpectedState {
    /// (entity_id, space_id) -> document
    pub documents: HashMap<(Uuid, Uuid), ExpectedDocument>,
    /// space_id -> topic_entity_id
    pub space_topics: HashMap<Uuid, Uuid>,
    /// All relations ever created (including those later deleted).
    /// Used to seed the Postgres `relations` table for the lookup fast path.
    pub all_created_relations: Vec<(Uuid, Uuid, Uuid)>, // (relation_id, entity_id, space_id)
}

impl ExpectedState {
    pub fn new() -> Self {
        Self {
            documents: HashMap::new(),
            space_topics: HashMap::new(),
            all_created_relations: Vec::new(),
        }
    }

    /// Ensure a document exists for (entity_id, space_id), returning a mutable ref.
    fn ensure_doc(&mut self, entity_id: Uuid, space_id: Uuid) -> &mut ExpectedDocument {
        self.documents
            .entry((entity_id, space_id))
            .or_insert_with(|| {
                let mut doc = ExpectedDocument::new(entity_id, space_id);
                // Inherit space topic if already known
                if let Some(&topic_id) = self.space_topics.get(&space_id) {
                    doc.space_topic_entity_id = Some(topic_id);
                }
                doc
            })
    }

    /// Upsert entity fields. On a deleted doc this is a noop (tombstone dominance)
    /// unless explicitly setting deleted.
    pub fn upsert_entity(
        &mut self,
        entity_id: Uuid,
        space_id: Uuid,
        name: Option<String>,
        description: Option<String>,
        image_url: Option<String>,
    ) {
        let doc = self.ensure_doc(entity_id, space_id);
        // Tombstone dominance: if deleted, non-delete updates are noop
        if doc.deleted {
            // Even when deleted, if this is a CreateEntity/UpdateEntity,
            // the indexer will upsert and the image_url, name, description
            // go through. But the deleted flag stays true. However the
            // actual indexer treats deleted docs as tombstoned and skips
            // upsert fields — so we also skip here.
            return;
        }
        if let Some(n) = name {
            doc.name = Some(n);
        }
        if let Some(d) = description {
            doc.description = Some(d);
        }
        if let Some(u) = image_url {
            doc.image_url = Some(u);
        }
    }

    /// Unset specific properties on an entity. Noop if deleted.
    pub fn unset_properties(
        &mut self,
        entity_id: Uuid,
        space_id: Uuid,
        property_keys: &[&str],
    ) {
        let doc = self.ensure_doc(entity_id, space_id);
        if doc.deleted {
            return;
        }
        for key in property_keys {
            match *key {
                "name" => doc.name = None,
                "description" => doc.description = None,
                "image_url" => doc.image_url = None,
                _ => {}
            }
        }
    }

    /// Delete an entity (set tombstone). The document remains but is marked deleted.
    pub fn delete_entity(&mut self, entity_id: Uuid, space_id: Uuid) {
        let doc = self.ensure_doc(entity_id, space_id);
        doc.deleted = true;
    }

    /// Restore a deleted entity (clear tombstone).
    pub fn restore_entity(&mut self, entity_id: Uuid, space_id: Uuid) {
        let doc = self.ensure_doc(entity_id, space_id);
        doc.deleted = false;
    }

    /// Add a relation. Noop if the document is deleted (tombstone dominance).
    /// Idempotent: if the relation_id already exists, this is a noop.
    pub fn add_relation(
        &mut self,
        entity_id: Uuid,
        space_id: Uuid,
        relation_id: Uuid,
        relation_type: Uuid,
        to_entity_id: Uuid,
    ) {
        // Track all created relations for Postgres seeding
        self.all_created_relations
            .push((relation_id, entity_id, space_id));

        let doc = self.ensure_doc(entity_id, space_id);
        if doc.deleted {
            return;
        }
        // Check for duplicate
        if doc.relations.iter().any(|r| r.relation_id == relation_id) {
            return;
        }
        doc.relations.push(ExpectedRelation {
            relation_id,
            relation_type,
            to_entity_id,
        });
    }

    /// Remove a relation by relation_id. Noop if deleted.
    pub fn remove_relation(
        &mut self,
        entity_id: Uuid,
        space_id: Uuid,
        relation_id: Uuid,
    ) {
        let doc = self.ensure_doc(entity_id, space_id);
        if doc.deleted {
            return;
        }
        doc.relations.retain(|r| r.relation_id != relation_id);
    }

    /// Update entity global score — applies to ALL documents with this entity_id.
    pub fn update_entity_global_score(&mut self, entity_id: Uuid, score: f64) {
        let keys: Vec<(Uuid, Uuid)> = self
            .documents
            .keys()
            .filter(|(eid, _)| *eid == entity_id)
            .cloned()
            .collect();
        for key in keys {
            self.documents
                .get_mut(&key)
                .unwrap()
                .entity_global_score = Some(score);
        }
    }

    /// Update space score — applies to ALL documents in this space.
    pub fn update_space_score(&mut self, space_id: Uuid, score: f64) {
        let keys: Vec<(Uuid, Uuid)> = self
            .documents
            .keys()
            .filter(|(_, sid)| *sid == space_id)
            .cloned()
            .collect();
        for key in keys {
            self.documents.get_mut(&key).unwrap().space_score = Some(score);
        }
    }

    /// Update entity-space score (perspective score).
    /// Creates the document if needed (doc_as_upsert: true in the indexer).
    pub fn update_entity_space_score(
        &mut self,
        entity_id: Uuid,
        space_id: Uuid,
        score: f64,
    ) {
        let doc = self.ensure_doc(entity_id, space_id);
        doc.entity_space_score = Some(score);
    }

    /// Declare a space topic. Updates all existing docs in that space and
    /// will be inherited by future documents.
    pub fn declare_space_topic(&mut self, space_id: Uuid, topic_entity_id: Uuid) {
        self.space_topics.insert(space_id, topic_entity_id);
        // Update all existing docs in this space
        let keys: Vec<(Uuid, Uuid)> = self
            .documents
            .keys()
            .filter(|(_, sid)| *sid == space_id)
            .cloned()
            .collect();
        for key in keys {
            self.documents
                .get_mut(&key)
                .unwrap()
                .space_topic_entity_id = Some(topic_entity_id);
        }
    }

    /// Count of non-deleted documents (what should be searchable).
    pub fn live_doc_count(&self) -> usize {
        self.documents.values().filter(|d| !d.deleted).count()
    }

    /// Total document count (including deleted).
    pub fn total_doc_count(&self) -> usize {
        self.documents.len()
    }
}
