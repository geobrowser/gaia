use anyhow::Result;
use rand::prelude::*;
use rand::rngs::SmallRng;
use uuid::Uuid;

use sdk::core::ids::{AVATAR_RELATION_TYPE_ID, COVER_RELATION_TYPE_ID, NAME_PROPERTY_ID, TYPE_RELATION_TYPE_ID};

use crate::config::LoadTestConfig;
use crate::expected_state::ExpectedState;
use crate::generators::{edits, relations, scores, space_topics};
use crate::sender::KafkaEvent;

/// Result of scenario generation.
pub struct GeneratedScenario {
    pub events: Vec<KafkaEvent>,
    /// Late score events sent after the indexer processes all main events.
    /// These target entities guaranteed to exist, so scores will always apply.
    pub late_score_events: Vec<KafkaEvent>,
    pub expected_state: ExpectedState,
    pub stats: GenerationStats,
}

#[derive(Debug, Default)]
pub struct GenerationStats {
    pub add_delete_relation: usize,
    pub create_delete_entity: usize,
    pub delete_restore_entity: usize,
    pub score_overwrite: usize,
    pub interleaved_space: usize,
    pub early_space_topic: usize,
    pub unset_then_set: usize,
    pub bulk_relation_churn: usize,
    pub avatar_cover_relations: usize,
    pub space_scores: usize,
    pub perspective_scores: usize,
    pub filler: usize,
    pub late_scores: usize,
    pub total_events: usize,
}

/// An event with ordering metadata for interleaving.
struct OrderedEvent {
    ordering_group: usize,
    sequence: usize,
    event: KafkaEvent,
}

fn get_topic_prefix() -> &'static str {
    match std::env::var("ENVIRONMENT").as_deref() {
        Ok("staging") => "staging.",
        Ok("production") => "",
        Ok(other) => panic!("Invalid ENVIRONMENT: {}", other),
        Err(_) => panic!("ENVIRONMENT must be set"),
    }
}

fn prefixed_topic(topic: &str) -> String {
    format!("{}{}", get_topic_prefix(), topic)
}

/// Generate a deterministic UUID from the RNG.
fn rand_uuid(rng: &mut SmallRng) -> Uuid {
    let mut bytes = [0u8; 16];
    rng.fill_bytes(&mut bytes);
    // Set version 4 and variant bits
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Uuid::from_bytes(bytes)
}

/// Generate a random name string.
fn rand_name(rng: &mut SmallRng, prefix: &str, idx: usize) -> String {
    format!("{}_{:05}_{:04x}", prefix, idx, rng.r#gen::<u16>())
}

/// Generate the full load test scenario.
pub fn generate(config: &LoadTestConfig) -> Result<GeneratedScenario> {
    let mut rng = SmallRng::seed_from_u64(config.seed);
    let mut state = ExpectedState::new();
    let mut ordered_events: Vec<OrderedEvent> = Vec::new();
    let mut group_counter = 0usize;
    let mut stats = GenerationStats::default();

    let edits_topic = prefixed_topic("knowledge.edits");
    let scores_topic = prefixed_topic("curation.scores");
    let space_topics_topic = prefixed_topic("space.topics");

    // Collect (entity_id, final_score) for late scoring phase.
    // Interleaved scores are fire-and-forget (no expected state update).
    // Late scores target entities guaranteed to exist.
    let mut late_score_entries: Vec<(Uuid, f64)> = Vec::new();
    let mut late_space_score_entries: Vec<(Uuid, Uuid, f64)> = Vec::new();
    let mut late_perspective_entries: Vec<(Uuid, Uuid, f64)> = Vec::new();

    // A shared type entity used by filler entities — create it as a real document
    // so the API can resolve its name when looking up type relations.
    let type_entity_id = rand_uuid(&mut rng);
    let type_entity_space_id = rand_uuid(&mut rng);
    let type_relation_type = Uuid::parse_str(TYPE_RELATION_TYPE_ID)?;
    {
        let group = group_counter;
        group_counter += 1;
        let payload = edits::create_entity_edit(
            "create-type-entity",
            type_entity_space_id,
            type_entity_id,
            Some("Shared Type"),
            Some("The type entity used by filler and other patterns"),
            None,
        )?;
        ordered_events.push(OrderedEvent {
            ordering_group: group,
            sequence: 0,
            event: KafkaEvent {
                topic: edits_topic.clone(),
                key: Some(type_entity_id.to_string()),
                payload,
            },
        });
        state.upsert_entity(
            type_entity_id,
            type_entity_space_id,
            Some("Shared Type".into()),
            Some("The type entity used by filler and other patterns".into()),
            None,
        );
    }

    // =========================================================================
    // Pattern 0: Showcase entities (10 fully-loaded entities for API testing)
    //
    // Each entity has: name, description, image_url, type relation (to named
    // type entity), avatar relation (to image entity with image_url), cover
    // relation (to image entity with image_url), space topic, and all three
    // score types applied in the late phase.
    // =========================================================================
    let showcase_space_id = rand_uuid(&mut rng);
    let showcase_topic_entity_id = rand_uuid(&mut rng);

    // Declare space topic first
    {
        let group = group_counter;
        group_counter += 1;

        let payload = space_topics::create_topic_declared(showcase_space_id, showcase_topic_entity_id)?;
        ordered_events.push(OrderedEvent {
            ordering_group: group,
            sequence: 0,
            event: KafkaEvent {
                topic: space_topics_topic.clone(),
                key: Some(showcase_space_id.to_string()),
                payload,
            },
        });
        state.declare_space_topic(showcase_space_id, showcase_topic_entity_id);

        // Create the topic entity itself
        let payload = edits::create_entity_edit(
            "create-showcase-topic",
            showcase_space_id,
            showcase_topic_entity_id,
            Some("Showcase Space"),
            Some("The showcase space for load test API testing"),
            None,
        )?;
        ordered_events.push(OrderedEvent {
            ordering_group: group,
            sequence: 1,
            event: KafkaEvent {
                topic: edits_topic.clone(),
                key: Some(showcase_topic_entity_id.to_string()),
                payload,
            },
        });
        state.upsert_entity(
            showcase_topic_entity_id,
            showcase_space_id,
            Some("Showcase Space".into()),
            Some("The showcase space for load test API testing".into()),
            None,
        );
    }

    // Create a named type entity for showcase entities
    let showcase_type_entity_id = rand_uuid(&mut rng);
    {
        let group = group_counter;
        group_counter += 1;

        let payload = edits::create_entity_edit(
            "create-showcase-type",
            showcase_space_id,
            showcase_type_entity_id,
            Some("Person"),
            Some("A person type entity"),
            None,
        )?;
        ordered_events.push(OrderedEvent {
            ordering_group: group,
            sequence: 0,
            event: KafkaEvent {
                topic: edits_topic.clone(),
                key: Some(showcase_type_entity_id.to_string()),
                payload,
            },
        });
        state.upsert_entity(
            showcase_type_entity_id,
            showcase_space_id,
            Some("Person".into()),
            Some("A person type entity".into()),
            None,
        );
    }

    let avatar_relation_type = Uuid::parse_str(AVATAR_RELATION_TYPE_ID)?;
    let cover_relation_type = Uuid::parse_str(COVER_RELATION_TYPE_ID)?;

    for i in 0..10usize {
        let entity_id = rand_uuid(&mut rng);
        let avatar_img_entity_id = rand_uuid(&mut rng);
        let cover_img_entity_id = rand_uuid(&mut rng);
        let type_rel_id = rand_uuid(&mut rng);
        let avatar_rel_id = rand_uuid(&mut rng);
        let cover_rel_id = rand_uuid(&mut rng);
        let group = group_counter;
        group_counter += 1;

        let name = format!("Loadtest Showcase {}", i);
        let desc = format!("A fully-loaded showcase entity for API testing ({})", i);
        let img_url = format!("https://example.com/showcase/{}.png", i);
        let avatar_url = format!("https://example.com/showcase/avatar_{}.png", i);
        let cover_url = format!("https://example.com/showcase/cover_{}.png", i);

        // seq 0: Create the main entity
        let payload = edits::create_entity_edit(
            "create-showcase",
            showcase_space_id,
            entity_id,
            Some(&name),
            Some(&desc),
            Some(&img_url),
        )?;
        ordered_events.push(OrderedEvent {
            ordering_group: group,
            sequence: 0,
            event: KafkaEvent {
                topic: edits_topic.clone(),
                key: Some(entity_id.to_string()),
                payload,
            },
        });
        state.upsert_entity(
            entity_id,
            showcase_space_id,
            Some(name),
            Some(desc),
            Some(img_url),
        );

        // seq 1: Create avatar image entity (with image_url for API resolution)
        let payload = edits::create_entity_edit(
            "create-showcase-avatar-img",
            showcase_space_id,
            avatar_img_entity_id,
            Some(&format!("Avatar {}", i)),
            None,
            Some(&avatar_url),
        )?;
        ordered_events.push(OrderedEvent {
            ordering_group: group,
            sequence: 1,
            event: KafkaEvent {
                topic: edits_topic.clone(),
                key: Some(avatar_img_entity_id.to_string()),
                payload,
            },
        });
        state.upsert_entity(
            avatar_img_entity_id,
            showcase_space_id,
            Some(format!("Avatar {}", i)),
            None,
            Some(avatar_url),
        );

        // seq 2: Create cover image entity (with image_url for API resolution)
        let payload = edits::create_entity_edit(
            "create-showcase-cover-img",
            showcase_space_id,
            cover_img_entity_id,
            Some(&format!("Cover {}", i)),
            None,
            Some(&cover_url),
        )?;
        ordered_events.push(OrderedEvent {
            ordering_group: group,
            sequence: 2,
            event: KafkaEvent {
                topic: edits_topic.clone(),
                key: Some(cover_img_entity_id.to_string()),
                payload,
            },
        });
        state.upsert_entity(
            cover_img_entity_id,
            showcase_space_id,
            Some(format!("Cover {}", i)),
            None,
            Some(cover_url),
        );

        // seq 3: Type relation → showcase_type_entity_id ("Person")
        let payload = relations::create_type_relation_with_id(
            "showcase-type-rel",
            showcase_space_id,
            type_rel_id,
            entity_id,
            showcase_type_entity_id,
        )?;
        ordered_events.push(OrderedEvent {
            ordering_group: group,
            sequence: 3,
            event: KafkaEvent {
                topic: edits_topic.clone(),
                key: Some(entity_id.to_string()),
                payload,
            },
        });
        state.add_relation(
            entity_id,
            showcase_space_id,
            type_rel_id,
            type_relation_type,
            showcase_type_entity_id,
        );

        // seq 4: Avatar relation → avatar_img_entity_id
        let payload = relations::create_avatar_relation(
            "showcase-avatar-rel",
            showcase_space_id,
            avatar_rel_id,
            entity_id,
            avatar_img_entity_id,
        )?;
        ordered_events.push(OrderedEvent {
            ordering_group: group,
            sequence: 4,
            event: KafkaEvent {
                topic: edits_topic.clone(),
                key: Some(entity_id.to_string()),
                payload,
            },
        });
        state.add_relation(
            entity_id,
            showcase_space_id,
            avatar_rel_id,
            avatar_relation_type,
            avatar_img_entity_id,
        );

        // seq 5: Cover relation → cover_img_entity_id
        let payload = relations::create_cover_relation(
            "showcase-cover-rel",
            showcase_space_id,
            cover_rel_id,
            entity_id,
            cover_img_entity_id,
        )?;
        ordered_events.push(OrderedEvent {
            ordering_group: group,
            sequence: 5,
            event: KafkaEvent {
                topic: edits_topic.clone(),
                key: Some(entity_id.to_string()),
                payload,
            },
        });
        state.add_relation(
            entity_id,
            showcase_space_id,
            cover_rel_id,
            cover_relation_type,
            cover_img_entity_id,
        );

        // Defer all three score types to late phase
        let entity_score: f64 = (i as f64 + 1.0) / 10.0;
        late_score_entries.push((entity_id, entity_score));
        late_space_score_entries.push((entity_id, showcase_space_id, 0.95));
        late_perspective_entries.push((entity_id, showcase_space_id, entity_score * 0.8));
    }

    // =========================================================================
    // Pattern 1: Add+Delete relation (same rel ID)
    // =========================================================================
    let count = config.scaled(2_000);
    for i in 0..count {
        let entity_id = rand_uuid(&mut rng);
        let space_id = rand_uuid(&mut rng);
        let relation_id = rand_uuid(&mut rng);
        let to_entity_id = rand_uuid(&mut rng);
        let name = rand_name(&mut rng, "adddelrel", i);
        let group = group_counter;
        group_counter += 1;

        // Create entity
        let payload = edits::create_entity_edit(
            "create",
            space_id,
            entity_id,
            Some(&name),
            None,
            None,
        )?;
        ordered_events.push(OrderedEvent {
            ordering_group: group,
            sequence: 0,
            event: KafkaEvent {
                topic: edits_topic.clone(),
                key: Some(entity_id.to_string()),
                payload,
            },
        });
        state.upsert_entity(entity_id, space_id, Some(name), None, None);

        // Add relation
        let payload = relations::create_type_relation_with_id(
            "add-rel",
            space_id,
            relation_id,
            entity_id,
            to_entity_id,
        )?;
        ordered_events.push(OrderedEvent {
            ordering_group: group,
            sequence: 1,
            event: KafkaEvent {
                topic: edits_topic.clone(),
                key: Some(entity_id.to_string()),
                payload,
            },
        });
        state.add_relation(
            entity_id,
            space_id,
            relation_id,
            type_relation_type,
            to_entity_id,
        );

        // Delete that same relation
        let payload = relations::delete_relation("del-rel", space_id, relation_id)?;
        ordered_events.push(OrderedEvent {
            ordering_group: group,
            sequence: 2,
            event: KafkaEvent {
                topic: edits_topic.clone(),
                key: Some(entity_id.to_string()),
                payload,
            },
        });
        state.remove_relation(entity_id, space_id, relation_id);

        stats.add_delete_relation += 3;
    }

    // =========================================================================
    // Pattern 2: Create+Delete entity
    // =========================================================================
    let count = config.scaled(1_500);
    for i in 0..count {
        let entity_id = rand_uuid(&mut rng);
        let space_id = rand_uuid(&mut rng);
        let name = rand_name(&mut rng, "createdel", i);
        let group = group_counter;
        group_counter += 1;

        // Create
        let payload = edits::create_entity_edit(
            "create",
            space_id,
            entity_id,
            Some(&name),
            None,
            None,
        )?;
        ordered_events.push(OrderedEvent {
            ordering_group: group,
            sequence: 0,
            event: KafkaEvent {
                topic: edits_topic.clone(),
                key: Some(entity_id.to_string()),
                payload,
            },
        });
        state.upsert_entity(entity_id, space_id, Some(name), None, None);

        // Delete
        let payload = edits::delete_entity("delete", space_id, entity_id)?;
        ordered_events.push(OrderedEvent {
            ordering_group: group,
            sequence: 1,
            event: KafkaEvent {
                topic: edits_topic.clone(),
                key: Some(entity_id.to_string()),
                payload,
            },
        });
        state.delete_entity(entity_id, space_id);

        stats.create_delete_entity += 2;
    }

    // =========================================================================
    // Pattern 3: Delete+Restore entity
    // =========================================================================
    let count = config.scaled(1_000);
    for i in 0..count {
        let entity_id = rand_uuid(&mut rng);
        let space_id = rand_uuid(&mut rng);
        let name = rand_name(&mut rng, "delrestore", i);
        let group = group_counter;
        group_counter += 1;

        // Create
        let payload = edits::create_entity_edit(
            "create",
            space_id,
            entity_id,
            Some(&name),
            Some("desc"),
            None,
        )?;
        ordered_events.push(OrderedEvent {
            ordering_group: group,
            sequence: 0,
            event: KafkaEvent {
                topic: edits_topic.clone(),
                key: Some(entity_id.to_string()),
                payload,
            },
        });
        state.upsert_entity(
            entity_id,
            space_id,
            Some(name.clone()),
            Some("desc".into()),
            None,
        );

        // Delete
        let payload = edits::delete_entity("delete", space_id, entity_id)?;
        ordered_events.push(OrderedEvent {
            ordering_group: group,
            sequence: 1,
            event: KafkaEvent {
                topic: edits_topic.clone(),
                key: Some(entity_id.to_string()),
                payload,
            },
        });
        state.delete_entity(entity_id, space_id);

        // Try to update while deleted (should be noop due to tombstone)
        let payload = edits::create_entity_edit(
            "update-while-deleted",
            space_id,
            entity_id,
            Some("should-not-apply"),
            None,
            None,
        )?;
        ordered_events.push(OrderedEvent {
            ordering_group: group,
            sequence: 2,
            event: KafkaEvent {
                topic: edits_topic.clone(),
                key: Some(entity_id.to_string()),
                payload,
            },
        });
        state.upsert_entity(
            entity_id,
            space_id,
            Some("should-not-apply".into()),
            None,
            None,
        );

        // Restore
        let payload = edits::restore_entity("restore", space_id, entity_id)?;
        ordered_events.push(OrderedEvent {
            ordering_group: group,
            sequence: 3,
            event: KafkaEvent {
                topic: edits_topic.clone(),
                key: Some(entity_id.to_string()),
                payload,
            },
        });
        state.restore_entity(entity_id, space_id);

        stats.delete_restore_entity += 4;
    }

    // =========================================================================
    // Pattern 4: Score overwrite race (5-10 updates, same entity)
    // =========================================================================
    let count = config.scaled(1_000);
    for i in 0..count {
        let entity_id = rand_uuid(&mut rng);
        let space_id = rand_uuid(&mut rng);
        let name = rand_name(&mut rng, "scorerace", i);
        let group = group_counter;
        group_counter += 1;
        let num_updates: usize = rng.gen_range(5..=10);

        // Create entity first
        let payload = edits::create_entity_edit(
            "create",
            space_id,
            entity_id,
            Some(&name),
            None,
            None,
        )?;
        ordered_events.push(OrderedEvent {
            ordering_group: group,
            sequence: 0,
            event: KafkaEvent {
                topic: edits_topic.clone(),
                key: Some(entity_id.to_string()),
                payload,
            },
        });
        state.upsert_entity(entity_id, space_id, Some(name), None, None);

        // Send multiple score updates (interleaved, fire-and-forget — no expected state update).
        // These may arrive before the entity is created in the index, so we don't assert on them.
        let mut final_score = 0.0;
        for j in 0..num_updates {
            let score: f64 = rng.gen_range(-1.0..1.0);
            final_score = score;
            let payload = scores::create_entity_scores(vec![(entity_id, score)])?;
            ordered_events.push(OrderedEvent {
                ordering_group: group,
                sequence: j + 1,
                event: KafkaEvent {
                    topic: scores_topic.clone(),
                    key: None,
                    payload,
                },
            });
        }
        // Defer to late scoring phase (entity guaranteed to exist by then)
        late_score_entries.push((entity_id, final_score));

        stats.score_overwrite += 1 + num_updates;
    }

    // =========================================================================
    // Pattern 5: Interleaved space updates (same entity, 3 spaces)
    // =========================================================================
    let count = config.scaled(500);
    for i in 0..count {
        let entity_id = rand_uuid(&mut rng);
        let group = group_counter;
        group_counter += 1;

        for s in 0..3usize {
            let space_id = rand_uuid(&mut rng);
            let name = format!("intrlv_{:05}_s{}", i, s);

            // Create in this space
            let payload = edits::create_entity_edit(
                "create",
                space_id,
                entity_id,
                Some(&name),
                None,
                None,
            )?;
            ordered_events.push(OrderedEvent {
                ordering_group: group,
                sequence: s * 2,
                event: KafkaEvent {
                    topic: edits_topic.clone(),
                    key: Some(entity_id.to_string()),
                    payload,
                },
            });
            state.upsert_entity(entity_id, space_id, Some(name), None, None);

            // Update in this space
            let desc = format!("desc_s{}", s);
            let payload = edits::create_entity_edit(
                "update",
                space_id,
                entity_id,
                None,
                Some(&desc),
                None,
            )?;
            ordered_events.push(OrderedEvent {
                ordering_group: group,
                sequence: s * 2 + 1,
                event: KafkaEvent {
                    topic: edits_topic.clone(),
                    key: Some(entity_id.to_string()),
                    payload,
                },
            });
            state.upsert_entity(entity_id, space_id, None, Some(desc), None);
        }

        stats.interleaved_space += 6;
    }

    // =========================================================================
    // Pattern 6: Early space topic (topic before entities)
    // =========================================================================
    let count = config.scaled(100);
    for i in 0..count {
        let space_id = rand_uuid(&mut rng);
        let topic_entity_id = rand_uuid(&mut rng);
        let group = group_counter;
        group_counter += 1;

        // Declare topic FIRST (before any entities in this space)
        let payload = space_topics::create_topic_declared(space_id, topic_entity_id)?;
        ordered_events.push(OrderedEvent {
            ordering_group: group,
            sequence: 0,
            event: KafkaEvent {
                topic: space_topics_topic.clone(),
                key: Some(space_id.to_string()),
                payload,
            },
        });
        state.declare_space_topic(space_id, topic_entity_id);

        // Create the topic entity itself
        let topic_name = format!("topic_{:05}", i);
        let payload = edits::create_entity_edit(
            "create-topic-entity",
            space_id,
            topic_entity_id,
            Some(&topic_name),
            None,
            None,
        )?;
        ordered_events.push(OrderedEvent {
            ordering_group: group,
            sequence: 1,
            event: KafkaEvent {
                topic: edits_topic.clone(),
                key: Some(topic_entity_id.to_string()),
                payload,
            },
        });
        state.upsert_entity(topic_entity_id, space_id, Some(topic_name), None, None);

        // Create entities AFTER the topic declaration
        for j in 0..3usize {
            let entity_id = rand_uuid(&mut rng);
            let name = format!("earlytopic_{:05}_e{}", i, j);
            let payload = edits::create_entity_edit(
                "create-after-topic",
                space_id,
                entity_id,
                Some(&name),
                None,
                None,
            )?;
            ordered_events.push(OrderedEvent {
                ordering_group: group,
                sequence: 2 + j,
                event: KafkaEvent {
                    topic: edits_topic.clone(),
                    key: Some(entity_id.to_string()),
                    payload,
                },
            });
            state.upsert_entity(entity_id, space_id, Some(name), None, None);
        }

        stats.early_space_topic += 5;
    }

    // =========================================================================
    // Pattern 7: Unset then set property
    // =========================================================================
    let count = config.scaled(1_000);
    for i in 0..count {
        let entity_id = rand_uuid(&mut rng);
        let space_id = rand_uuid(&mut rng);
        let original_name = rand_name(&mut rng, "unsetset_orig", i);
        let final_name = rand_name(&mut rng, "unsetset_final", i);
        let group = group_counter;
        group_counter += 1;

        // Create with name
        let payload = edits::create_entity_edit(
            "create",
            space_id,
            entity_id,
            Some(&original_name),
            Some("desc"),
            None,
        )?;
        ordered_events.push(OrderedEvent {
            ordering_group: group,
            sequence: 0,
            event: KafkaEvent {
                topic: edits_topic.clone(),
                key: Some(entity_id.to_string()),
                payload,
            },
        });
        state.upsert_entity(
            entity_id,
            space_id,
            Some(original_name),
            Some("desc".into()),
            None,
        );

        // Unset name
        let payload = edits::unset_entity_properties(
            "unset",
            space_id,
            entity_id,
            vec![NAME_PROPERTY_ID],
        )?;
        ordered_events.push(OrderedEvent {
            ordering_group: group,
            sequence: 1,
            event: KafkaEvent {
                topic: edits_topic.clone(),
                key: Some(entity_id.to_string()),
                payload,
            },
        });
        state.unset_properties(entity_id, space_id, &["name"]);

        // Set name again
        let payload = edits::create_entity_edit(
            "set-again",
            space_id,
            entity_id,
            Some(&final_name),
            None,
            None,
        )?;
        ordered_events.push(OrderedEvent {
            ordering_group: group,
            sequence: 2,
            event: KafkaEvent {
                topic: edits_topic.clone(),
                key: Some(entity_id.to_string()),
                payload,
            },
        });
        state.upsert_entity(entity_id, space_id, Some(final_name), None, None);

        stats.unset_then_set += 3;
    }

    // =========================================================================
    // Pattern 8: Bulk relation churn (20 add, 15 delete)
    // =========================================================================
    let count = config.scaled(200);
    for i in 0..count {
        let entity_id = rand_uuid(&mut rng);
        let space_id = rand_uuid(&mut rng);
        let name = rand_name(&mut rng, "bulkchurn", i);
        let group = group_counter;
        group_counter += 1;

        // Create entity
        let payload = edits::create_entity_edit(
            "create",
            space_id,
            entity_id,
            Some(&name),
            None,
            None,
        )?;
        ordered_events.push(OrderedEvent {
            ordering_group: group,
            sequence: 0,
            event: KafkaEvent {
                topic: edits_topic.clone(),
                key: Some(entity_id.to_string()),
                payload,
            },
        });
        state.upsert_entity(entity_id, space_id, Some(name), None, None);

        // Add 20 relations
        let mut relation_ids = Vec::new();
        let mut to_entity_ids = Vec::new();
        for j in 0..20usize {
            let relation_id = rand_uuid(&mut rng);
            let to_entity_id = rand_uuid(&mut rng);
            relation_ids.push(relation_id);
            to_entity_ids.push(to_entity_id);

            let payload = relations::create_type_relation_with_id(
                "add-rel",
                space_id,
                relation_id,
                entity_id,
                to_entity_id,
            )?;
            ordered_events.push(OrderedEvent {
                ordering_group: group,
                sequence: 1 + j,
                event: KafkaEvent {
                    topic: edits_topic.clone(),
                    key: Some(entity_id.to_string()),
                    payload,
                },
            });
            state.add_relation(
                entity_id,
                space_id,
                relation_id,
                type_relation_type,
                to_entity_id,
            );
        }

        // Delete 15 of the 20 relations
        for j in 0..15usize {
            let relation_id = relation_ids[j];
            let payload = relations::delete_relation("del-rel", space_id, relation_id)?;
            ordered_events.push(OrderedEvent {
                ordering_group: group,
                sequence: 21 + j,
                event: KafkaEvent {
                    topic: edits_topic.clone(),
                    key: Some(entity_id.to_string()),
                    payload,
                },
            });
            state.remove_relation(entity_id, space_id, relation_id);
        }

        stats.bulk_relation_churn += 1 + 20 + 15;
    }

    // =========================================================================
    // Pattern 9: Avatar + Cover relations
    // =========================================================================
    let count = config.scaled(500);
    for i in 0..count {
        let entity_id = rand_uuid(&mut rng);
        let space_id = rand_uuid(&mut rng);
        let avatar_image_entity_id = rand_uuid(&mut rng);
        let cover_image_entity_id = rand_uuid(&mut rng);
        let avatar_relation_id = rand_uuid(&mut rng);
        let cover_relation_id = rand_uuid(&mut rng);
        let name = rand_name(&mut rng, "avatarcover", i);
        let img_url = format!("https://example.com/img/{}.png", i);
        let avatar_img_url = format!("https://example.com/avatar/{}.png", i);
        let cover_img_url = format!("https://example.com/cover/{}.png", i);
        let group = group_counter;
        group_counter += 1;

        // Create entity with image_url
        let payload = edits::create_entity_edit(
            "create",
            space_id,
            entity_id,
            Some(&name),
            None,
            Some(&img_url),
        )?;
        ordered_events.push(OrderedEvent {
            ordering_group: group,
            sequence: 0,
            event: KafkaEvent {
                topic: edits_topic.clone(),
                key: Some(entity_id.to_string()),
                payload,
            },
        });
        state.upsert_entity(entity_id, space_id, Some(name), None, Some(img_url));

        // Create the avatar image entity (so the API can resolve the avatar URL)
        let payload = edits::create_entity_edit(
            "create-avatar-img",
            space_id,
            avatar_image_entity_id,
            Some(&format!("Avatar Image {}", i)),
            None,
            Some(&avatar_img_url),
        )?;
        ordered_events.push(OrderedEvent {
            ordering_group: group,
            sequence: 1,
            event: KafkaEvent {
                topic: edits_topic.clone(),
                key: Some(avatar_image_entity_id.to_string()),
                payload,
            },
        });
        state.upsert_entity(
            avatar_image_entity_id,
            space_id,
            Some(format!("Avatar Image {}", i)),
            None,
            Some(avatar_img_url),
        );

        // Create the cover image entity (so the API can resolve the cover URL)
        let payload = edits::create_entity_edit(
            "create-cover-img",
            space_id,
            cover_image_entity_id,
            Some(&format!("Cover Image {}", i)),
            None,
            Some(&cover_img_url),
        )?;
        ordered_events.push(OrderedEvent {
            ordering_group: group,
            sequence: 2,
            event: KafkaEvent {
                topic: edits_topic.clone(),
                key: Some(cover_image_entity_id.to_string()),
                payload,
            },
        });
        state.upsert_entity(
            cover_image_entity_id,
            space_id,
            Some(format!("Cover Image {}", i)),
            None,
            Some(cover_img_url),
        );

        // Add avatar relation
        let payload = relations::create_avatar_relation(
            "add-avatar",
            space_id,
            avatar_relation_id,
            entity_id,
            avatar_image_entity_id,
        )?;
        ordered_events.push(OrderedEvent {
            ordering_group: group,
            sequence: 3,
            event: KafkaEvent {
                topic: edits_topic.clone(),
                key: Some(entity_id.to_string()),
                payload,
            },
        });
        state.add_relation(
            entity_id,
            space_id,
            avatar_relation_id,
            avatar_relation_type,
            avatar_image_entity_id,
        );

        // Add cover relation
        let payload = relations::create_cover_relation(
            "add-cover",
            space_id,
            cover_relation_id,
            entity_id,
            cover_image_entity_id,
        )?;
        ordered_events.push(OrderedEvent {
            ordering_group: group,
            sequence: 4,
            event: KafkaEvent {
                topic: edits_topic.clone(),
                key: Some(entity_id.to_string()),
                payload,
            },
        });
        state.add_relation(
            entity_id,
            space_id,
            cover_relation_id,
            cover_relation_type,
            cover_image_entity_id,
        );

        stats.avatar_cover_relations += 5;
    }

    // =========================================================================
    // Pattern 10: Space scores
    // =========================================================================
    // Create entities in shared spaces, then send space score updates.
    // Space scores apply to ALL documents in that space via update_by_query.
    let count = config.scaled(200);
    for i in 0..count {
        let space_id = rand_uuid(&mut rng);
        let group = group_counter;
        group_counter += 1;

        // Create 3 entities in the same space
        let mut entity_ids = Vec::new();
        for j in 0..3usize {
            let entity_id = rand_uuid(&mut rng);
            entity_ids.push(entity_id);
            let name = rand_name(&mut rng, "spacescore", i * 3 + j);
            let payload = edits::create_entity_edit(
                "create",
                space_id,
                entity_id,
                Some(&name),
                None,
                None,
            )?;
            ordered_events.push(OrderedEvent {
                ordering_group: group,
                sequence: j,
                event: KafkaEvent {
                    topic: edits_topic.clone(),
                    key: Some(entity_id.to_string()),
                    payload,
                },
            });
            state.upsert_entity(entity_id, space_id, Some(name), None, None);
        }

        // Send interleaved space score (fire-and-forget, may arrive before entities indexed)
        let score: f64 = rng.gen_range(0.0..1.0);
        let payload = scores::create_space_scores(vec![(space_id, score)])?;
        ordered_events.push(OrderedEvent {
            ordering_group: group,
            sequence: 3,
            event: KafkaEvent {
                topic: scores_topic.clone(),
                key: None,
                payload,
            },
        });

        // Defer to late scoring phase (entities guaranteed to exist by then)
        for eid in &entity_ids {
            late_space_score_entries.push((*eid, space_id, score));
        }

        stats.space_scores += 4;
    }

    // =========================================================================
    // Pattern 11: Perspective scores (entity-space scores)
    // =========================================================================
    let count = config.scaled(300);
    for i in 0..count {
        let entity_id = rand_uuid(&mut rng);
        let space_id = rand_uuid(&mut rng);
        let name = rand_name(&mut rng, "perspective", i);
        let group = group_counter;
        group_counter += 1;

        // Create entity
        let payload = edits::create_entity_edit(
            "create",
            space_id,
            entity_id,
            Some(&name),
            None,
            None,
        )?;
        ordered_events.push(OrderedEvent {
            ordering_group: group,
            sequence: 0,
            event: KafkaEvent {
                topic: edits_topic.clone(),
                key: Some(entity_id.to_string()),
                payload,
            },
        });
        state.upsert_entity(entity_id, space_id, Some(name), None, None);

        // Send interleaved perspective score (fire-and-forget)
        let score: f64 = rng.gen_range(-1.0..1.0);
        let payload = scores::create_perspective_scores(vec![(entity_id, space_id, score)])?;
        ordered_events.push(OrderedEvent {
            ordering_group: group,
            sequence: 1,
            event: KafkaEvent {
                topic: scores_topic.clone(),
                key: None,
                payload,
            },
        });

        // Defer to late scoring phase
        late_perspective_entries.push((entity_id, space_id, score));

        stats.perspective_scores += 2;
    }

    // =========================================================================
    // Pattern 12: Filler (create + type relation + score)
    //
    // Every 5th filler is "enriched": it gets avatar/cover relations pointing
    // to shared image entities, and lives in a space with a declared topic.
    // This ensures a good number of random entities are fully resolvable by
    // the search API (type name, avatar, cover, space metadata).
    // =========================================================================

    // Shared avatar/cover image entities reused by enriched fillers
    let filler_avatar_img_id = rand_uuid(&mut rng);
    let filler_cover_img_id = rand_uuid(&mut rng);
    {
        let group = group_counter;
        group_counter += 1;
        let payload = edits::create_entity_edit(
            "create-filler-avatar-img",
            type_entity_space_id,
            filler_avatar_img_id,
            Some("Filler Avatar Image"),
            None,
            Some("https://example.com/filler/avatar.png"),
        )?;
        ordered_events.push(OrderedEvent {
            ordering_group: group,
            sequence: 0,
            event: KafkaEvent {
                topic: edits_topic.clone(),
                key: Some(filler_avatar_img_id.to_string()),
                payload,
            },
        });
        state.upsert_entity(
            filler_avatar_img_id,
            type_entity_space_id,
            Some("Filler Avatar Image".into()),
            None,
            Some("https://example.com/filler/avatar.png".into()),
        );

        let group = group_counter;
        group_counter += 1;
        let payload = edits::create_entity_edit(
            "create-filler-cover-img",
            type_entity_space_id,
            filler_cover_img_id,
            Some("Filler Cover Image"),
            None,
            Some("https://example.com/filler/cover.png"),
        )?;
        ordered_events.push(OrderedEvent {
            ordering_group: group,
            sequence: 0,
            event: KafkaEvent {
                topic: edits_topic.clone(),
                key: Some(filler_cover_img_id.to_string()),
                payload,
            },
        });
        state.upsert_entity(
            filler_cover_img_id,
            type_entity_space_id,
            Some("Filler Cover Image".into()),
            None,
            Some("https://example.com/filler/cover.png".into()),
        );
    }

    // Pool of spaces with declared topics for enriched fillers
    let num_filler_spaces = 5usize;
    let mut filler_space_ids: Vec<Uuid> = Vec::new();
    for s in 0..num_filler_spaces {
        let space_id = rand_uuid(&mut rng);
        let topic_entity_id = rand_uuid(&mut rng);
        filler_space_ids.push(space_id);

        let group = group_counter;
        group_counter += 1;

        // Declare topic
        let payload = space_topics::create_topic_declared(space_id, topic_entity_id)?;
        ordered_events.push(OrderedEvent {
            ordering_group: group,
            sequence: 0,
            event: KafkaEvent {
                topic: space_topics_topic.clone(),
                key: Some(space_id.to_string()),
                payload,
            },
        });
        state.declare_space_topic(space_id, topic_entity_id);

        // Create topic entity with name
        let topic_name = format!("Filler Space {}", s);
        let payload = edits::create_entity_edit(
            "create-filler-topic",
            space_id,
            topic_entity_id,
            Some(&topic_name),
            Some(&format!("Topic entity for filler space {}", s)),
            None,
        )?;
        ordered_events.push(OrderedEvent {
            ordering_group: group,
            sequence: 1,
            event: KafkaEvent {
                topic: edits_topic.clone(),
                key: Some(topic_entity_id.to_string()),
                payload,
            },
        });
        state.upsert_entity(
            topic_entity_id,
            space_id,
            Some(topic_name),
            Some(format!("Topic entity for filler space {}", s)),
            None,
        );
    }

    let current_events = ordered_events.len();
    let target_total = config.scaled(100_000);
    // Average ~3.4 events per filler (every 5th has 2 extra for avatar/cover)
    let events_per_filler = 3;
    let filler_count = if target_total > current_events {
        (target_total - current_events) / events_per_filler
    } else {
        config.scaled(1_000)
    };

    for i in 0..filler_count {
        let entity_id = rand_uuid(&mut rng);
        let relation_id = rand_uuid(&mut rng);
        let name = rand_name(&mut rng, "filler", i);
        let desc = format!("Filler entity {}", i);
        let score: f64 = rng.gen_range(0.0..1.0);
        let group = group_counter;
        group_counter += 1;

        let enriched = i % 5 == 0;

        // Enriched fillers use a topic'd space; others get a random space
        let space_id = if enriched {
            filler_space_ids[(i / 5) % num_filler_spaces]
        } else {
            rand_uuid(&mut rng)
        };

        // Create entity
        let payload = edits::create_entity_edit(
            "create",
            space_id,
            entity_id,
            Some(&name),
            Some(&desc),
            None,
        )?;
        ordered_events.push(OrderedEvent {
            ordering_group: group,
            sequence: 0,
            event: KafkaEvent {
                topic: edits_topic.clone(),
                key: Some(entity_id.to_string()),
                payload,
            },
        });
        state.upsert_entity(entity_id, space_id, Some(name), Some(desc), None);

        // Type relation (with known ID so we can track it)
        let payload = relations::create_type_relation_with_id(
            "type-rel",
            space_id,
            relation_id,
            entity_id,
            type_entity_id,
        )?;
        ordered_events.push(OrderedEvent {
            ordering_group: group,
            sequence: 1,
            event: KafkaEvent {
                topic: edits_topic.clone(),
                key: Some(entity_id.to_string()),
                payload,
            },
        });
        state.add_relation(
            entity_id,
            space_id,
            relation_id,
            type_relation_type,
            type_entity_id,
        );

        // Entity score (interleaved, fire-and-forget — no expected state update)
        let payload = scores::create_entity_scores(vec![(entity_id, score)])?;
        ordered_events.push(OrderedEvent {
            ordering_group: group,
            sequence: 2,
            event: KafkaEvent {
                topic: scores_topic.clone(),
                key: None,
                payload,
            },
        });
        // Defer to late scoring phase (entity guaranteed to exist by then)
        late_score_entries.push((entity_id, score));

        stats.filler += 3;

        // Enriched fillers: add avatar + cover relations to shared image entities
        if enriched {
            let avatar_rel_id = rand_uuid(&mut rng);
            let payload = relations::create_avatar_relation(
                "filler-avatar-rel",
                space_id,
                avatar_rel_id,
                entity_id,
                filler_avatar_img_id,
            )?;
            ordered_events.push(OrderedEvent {
                ordering_group: group,
                sequence: 3,
                event: KafkaEvent {
                    topic: edits_topic.clone(),
                    key: Some(entity_id.to_string()),
                    payload,
                },
            });
            state.add_relation(
                entity_id,
                space_id,
                avatar_rel_id,
                avatar_relation_type,
                filler_avatar_img_id,
            );

            let cover_rel_id = rand_uuid(&mut rng);
            let payload = relations::create_cover_relation(
                "filler-cover-rel",
                space_id,
                cover_rel_id,
                entity_id,
                filler_cover_img_id,
            )?;
            ordered_events.push(OrderedEvent {
                ordering_group: group,
                sequence: 4,
                event: KafkaEvent {
                    topic: edits_topic.clone(),
                    key: Some(entity_id.to_string()),
                    payload,
                },
            });
            state.add_relation(
                entity_id,
                space_id,
                cover_rel_id,
                cover_relation_type,
                filler_cover_img_id,
            );

            stats.filler += 2;
        }
    }

    stats.total_events = ordered_events.len();

    // =========================================================================
    // Late scoring phase: generate score events for entities guaranteed to exist
    // =========================================================================
    let mut late_score_events: Vec<KafkaEvent> = Vec::new();

    // Entity global scores
    for (entity_id, score) in &late_score_entries {
        let payload = scores::create_entity_scores(vec![(*entity_id, *score)])?;
        late_score_events.push(KafkaEvent {
            topic: scores_topic.clone(),
            key: None,
            payload,
        });
        state.update_entity_global_score(*entity_id, *score);
    }

    // Space scores (apply to all docs in the space)
    // Group by space_id to send one score event per space
    let mut space_score_map: std::collections::HashMap<Uuid, f64> = std::collections::HashMap::new();
    for &(_, space_id, score) in &late_space_score_entries {
        space_score_map.insert(space_id, score);
    }
    for (space_id, score) in &space_score_map {
        let payload = scores::create_space_scores(vec![(*space_id, *score)])?;
        late_score_events.push(KafkaEvent {
            topic: scores_topic.clone(),
            key: None,
            payload,
        });
        state.update_space_score(*space_id, *score);
    }

    // Perspective scores (entity-space combinations)
    for (entity_id, space_id, score) in &late_perspective_entries {
        let payload = scores::create_perspective_scores(vec![(*entity_id, *space_id, *score)])?;
        late_score_events.push(KafkaEvent {
            topic: scores_topic.clone(),
            key: None,
            payload,
        });
        state.update_entity_space_score(*entity_id, *space_id, *score);
    }

    stats.late_scores = late_score_events.len();

    // =========================================================================
    // Interleave events across ordering groups
    // =========================================================================
    let all_events = interleave_events(&mut rng, ordered_events);

    Ok(GeneratedScenario {
        events: all_events,
        late_score_events,
        expected_state: state,
        stats,
    })
}

/// Interleave events across ordering groups while preserving causal order within each group.
fn interleave_events(rng: &mut SmallRng, ordered_events: Vec<OrderedEvent>) -> Vec<KafkaEvent> {
    // Group events by ordering_group, sort each group by sequence
    let mut groups: std::collections::HashMap<usize, Vec<KafkaEvent>> =
        std::collections::HashMap::new();

    // Sort all events by (ordering_group, sequence) first
    let mut sorted = ordered_events;
    sorted.sort_by_key(|e| (e.ordering_group, e.sequence));

    for event in sorted {
        groups
            .entry(event.ordering_group)
            .or_default()
            .push(event.event);
    }

    // Build cursor list: (group_id, index_into_group)
    let mut group_ids: Vec<usize> = groups.keys().cloned().collect();
    group_ids.sort();
    let mut cursors: Vec<(usize, usize)> = group_ids.into_iter().map(|g| (g, 0)).collect();

    let total: usize = groups.values().map(|v| v.len()).sum();
    let mut result = Vec::with_capacity(total);

    while !cursors.is_empty() {
        let idx = rng.gen_range(0..cursors.len());
        let (group_id, cursor) = cursors[idx];
        let group = groups.get_mut(&group_id).unwrap();

        // Take event from group (replace with empty to avoid clone)
        let event = std::mem::replace(
            &mut group[cursor],
            KafkaEvent {
                topic: String::new(),
                key: None,
                payload: Vec::new(),
            },
        );
        result.push(event);

        cursors[idx].1 += 1;
        if cursors[idx].1 >= groups[&group_id].len() {
            cursors.swap_remove(idx);
        }
    }

    result
}
