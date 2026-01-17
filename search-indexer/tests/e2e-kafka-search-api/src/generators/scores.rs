use anyhow::Result;
use chrono::Utc;
use prost::Message;
use uuid::Uuid;

use hermes_schema::pb::scoring::{EntityScore, HermesScoresBatch, PerspectiveScore, SpaceScore};

/// Generate a comprehensive batch with all score types
pub fn create_mixed_score_batch(
    entity_scores: Vec<(Uuid, f64)>,
    space_scores: Vec<(Uuid, f64)>,
    perspective_scores: Vec<(Uuid, Uuid, f64)>,
    batch_sequence: u32,
    is_final: bool,
) -> Result<Vec<u8>> {
    let timestamp = Utc::now().timestamp() as u64;

    let entity_score_msgs: Vec<EntityScore> = entity_scores
        .into_iter()
        .map(|(entity_id, score)| EntityScore {
            entity_id: entity_id.as_bytes().to_vec(),
            score,
            updated_at: timestamp,
        })
        .collect();

    let space_score_msgs: Vec<SpaceScore> = space_scores
        .into_iter()
        .map(|(space_id, score)| SpaceScore {
            space_id: space_id.as_bytes().to_vec(),
            score,
            updated_at: timestamp,
        })
        .collect();

    let perspective_score_msgs: Vec<PerspectiveScore> = perspective_scores
        .into_iter()
        .map(|(entity_id, space_id, score)| PerspectiveScore {
            entity_id: entity_id.as_bytes().to_vec(),
            space_id: space_id.as_bytes().to_vec(),
            score,
            updated_at: timestamp,
        })
        .collect();

    let batch = HermesScoresBatch {
        entity_scores: entity_score_msgs,
        perspective_scores: perspective_score_msgs,
        space_scores: space_score_msgs,
        computed_at: timestamp,
        batch_sequence,
        is_final,
    };

    let mut buf = Vec::new();
    batch.encode(&mut buf)?;
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_mixed_score_batch() {
        let entity = Uuid::new_v4();
        let space = Uuid::new_v4();

        let result = create_mixed_score_batch(
            vec![(entity, 0.95)],
            vec![(space, 0.75)],
            vec![(entity, space, 0.85)],
            1,
            true,
        );

        assert!(result.is_ok());
        let bytes = result.unwrap();

        let decoded = HermesScoresBatch::decode(&bytes[..]);
        assert!(decoded.is_ok());
        let batch = decoded.unwrap();
        assert_eq!(batch.entity_scores.len(), 1);
        assert_eq!(batch.space_scores.len(), 1);
        assert_eq!(batch.perspective_scores.len(), 1);
    }
}
