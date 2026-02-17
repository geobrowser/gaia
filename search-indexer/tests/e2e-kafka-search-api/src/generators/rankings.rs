use anyhow::Result;
use serde::Deserialize;
use tracing::info;
use uuid::Uuid;

use crate::generators::{edits, scores};
use crate::kafka::KafkaProducer;

#[derive(Deserialize)]
pub struct RankingTestSuite {
    pub uniform_score: f64,
    pub space_id: String,
    pub space_id_prefix: String,
    pub uuid_prefix: String,
    pub test_cases: Vec<RankingTestCase>,
}

#[derive(Deserialize)]
pub struct RankingTestCase {
    pub name: String,
    pub query: String,
    pub scope: Option<String>,
    pub entities: Vec<RankingEntity>,
}

#[derive(Deserialize)]
pub struct RankingEntity {
    pub name: String,
    pub description: String,
    pub global_score: Option<f64>,
    pub entity_space_score: Option<f64>,
    pub space_score: Option<f64>,
}

impl RankingTestSuite {
    pub fn load() -> Result<Self> {
        let json = include_str!("../../ranking-tests/test-cases.json");
        let suite: RankingTestSuite = serde_json::from_str(json)?;
        Ok(suite)
    }

    /// Generate a deterministic UUID for a test entity.
    /// Format: `{uuid_prefix}{test_idx:02x}{entity_idx:02x}`
    pub fn entity_id(&self, test_idx: usize, entity_idx: usize) -> Result<Uuid> {
        let uuid_str = format!("{}{:02x}{:02x}", self.uuid_prefix, test_idx, entity_idx);
        Ok(Uuid::parse_str(&uuid_str)?)
    }

    /// Generate a deterministic space UUID for a test entity (used for space_score tests).
    /// Format: `{space_id_prefix}{test_idx:02x}{entity_idx:02x}`
    pub fn entity_space_id(&self, test_idx: usize, entity_idx: usize) -> Result<Uuid> {
        let uuid_str = format!("{}{:02x}{:02x}", self.space_id_prefix, test_idx, entity_idx);
        Ok(Uuid::parse_str(&uuid_str)?)
    }

    pub fn space_id(&self) -> Result<Uuid> {
        Ok(Uuid::parse_str(&self.space_id)?)
    }

    pub async fn publish_all(
        &self,
        producer: &KafkaProducer,
        edits_topic: &str,
        scores_topic: &str,
    ) -> Result<()> {
        let space_id = self.space_id()?;
        let mut all_entity_scores: Vec<(Uuid, f64)> = Vec::new();
        let mut all_space_scores: Vec<(Uuid, f64)> = Vec::new();
        let mut all_perspective_scores: Vec<(Uuid, Uuid, f64)> = Vec::new();

        for (test_idx, test_case) in self.test_cases.iter().enumerate() {
            info!(
                "  Ranking test {}: \"{}\" (query: \"{}\")",
                test_idx, test_case.name, test_case.query
            );

            for (entity_idx, entity) in test_case.entities.iter().enumerate() {
                let entity_id = self.entity_id(test_idx, entity_idx)?;

                // For space_score tests, each entity gets its own space so
                // different space_score values can be tested independently.
                let entity_space = if entity.space_score.is_some() {
                    self.entity_space_id(test_idx, entity_idx)?
                } else {
                    space_id
                };

                let payload = edits::create_entity_edit(
                    &format!("Ranking: {} entity {}", test_case.name, entity_idx),
                    entity_space,
                    entity_id,
                    Some(&entity.name),
                    Some(&entity.description),
                    None,
                )?;
                producer.send(edits_topic, None, payload).await?;

                let score = entity.global_score.unwrap_or(self.uniform_score);
                all_entity_scores.push((entity_id, score));

                if let Some(ss) = entity.space_score {
                    all_space_scores.push((entity_space, ss));
                }

                if let Some(esp) = entity.entity_space_score {
                    all_perspective_scores.push((entity_id, space_id, esp));
                }

                info!(
                    "    Entity {}: {} (id: {})",
                    entity_idx, entity.name, entity_id
                );
            }
        }

        // Publish all scores in a single batch
        let score_payload = scores::create_mixed_score_batch(
            all_entity_scores,
            all_space_scores,
            all_perspective_scores,
            2, // batch_sequence (1 is used by existing tests)
            true,
        )?;
        producer.send(scores_topic, None, score_payload).await?;

        info!(
            "  Published {} ranking test entities with uniform score {}",
            self.test_cases.iter().map(|tc| tc.entities.len()).sum::<usize>(),
            self.uniform_score
        );

        Ok(())
    }
}
