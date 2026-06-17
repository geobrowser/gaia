use hermes_instrumentation::info;
use sqlx::{postgres::PgPoolOptions, Postgres, QueryBuilder};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use uuid::Uuid;

use crate::error::{HandlerError, IndexerError};
use crate::models::{
    entities::EntityItem,
    governance::{
        ProposalActionItem, ProposalActionPayload, ProposalIdentity, ProposalVersionItem,
        ProposalVoteItem, SpaceVotingSettingsItem, VoteOption, VotingMode,
    },
    membership::{EditorItem, MemberItem},
    relations::{RelationOp, SetRelationItem, UnsetRelationItem, UpdateRelationItem},
    spaces::{SpaceItem, SpaceType},
    subspaces::{SubspaceItem, SubspaceTopicItem},
    values::{ValueChangeType, ValueOp},
};

#[derive(Clone)]
pub struct Storage {
    pub pool: sqlx::Pool<Postgres>,
}

impl Storage {
    pub async fn new(database_url: &str) -> Result<Self, IndexerError> {
        let pool = PgPoolOptions::new()
            .max_connections(20)
            .connect(database_url)
            .await?;

        Ok(Storage { pool })
    }

    pub async fn insert_entities(
        &self,
        entities: &[EntityItem],
        tx: &mut sqlx::Transaction<'_, Postgres>,
    ) -> Result<(), IndexerError> {
        if entities.is_empty() {
            return Ok(());
        }

        let ids: Vec<Uuid> = entities.iter().map(|x| x.id).collect();
        let created_ats: Vec<String> = entities.iter().map(|x| x.created_at.clone()).collect();
        let created_at_blocks: Vec<String> = entities
            .iter()
            .map(|x| x.created_at_block.clone())
            .collect();
        let updated_ats: Vec<String> = entities.iter().map(|x| x.updated_at.clone()).collect();
        let updated_at_blocks: Vec<String> = entities
            .iter()
            .map(|x| x.updated_at_block.clone())
            .collect();

        sqlx::query!(
            r#"
            INSERT INTO entities (id, created_at, created_at_block, updated_at, updated_at_block)
            SELECT * FROM UNNEST($1::uuid[], $2::text[], $3::text[], $4::text[], $5::text[])
            ON CONFLICT (id)
            DO UPDATE SET updated_at = EXCLUDED.updated_at, updated_at_block = EXCLUDED.updated_at_block
            "#,
            &ids,
            &created_ats,
            &created_at_blocks,
            &updated_ats,
            &updated_at_blocks
        )
        .execute(&mut **tx)
        .await?;

        Ok(())
    }

    pub async fn insert_values(
        &self,
        values: &[ValueOp],
        tx: &mut sqlx::Transaction<'_, Postgres>,
    ) -> Result<(), IndexerError> {
        if values.is_empty() {
            return Ok(());
        }

        let mut ids = Vec::with_capacity(values.len());
        let mut entity_ids = Vec::with_capacity(values.len());
        let mut property_ids = Vec::with_capacity(values.len());
        let mut space_ids = Vec::with_capacity(values.len());
        let mut languages = Vec::with_capacity(values.len());
        let mut units = Vec::with_capacity(values.len());
        // Value columns
        let mut text_values = Vec::with_capacity(values.len());
        let mut decimal_values = Vec::with_capacity(values.len());
        let mut boolean_values = Vec::with_capacity(values.len());
        let mut time_values = Vec::with_capacity(values.len());
        let mut point_values = Vec::with_capacity(values.len());
        let mut rect_values = Vec::with_capacity(values.len());
        let mut integer_values = Vec::with_capacity(values.len());
        let mut float_values = Vec::with_capacity(values.len());
        let mut bytes_values: Vec<Option<&[u8]>> = Vec::with_capacity(values.len());
        let mut date_values = Vec::with_capacity(values.len());
        let mut datetime_values = Vec::with_capacity(values.len());
        let mut schedule_values = Vec::with_capacity(values.len());
        let mut embedding_values = Vec::with_capacity(values.len());
        let mut time_utc_values = Vec::with_capacity(values.len());
        let mut datetime_utc_values = Vec::with_capacity(values.len());

        for prop in values {
            ids.push(prop.id.to_string());
            entity_ids.push(&prop.entity_id);
            property_ids.push(&prop.property_id);
            space_ids.push(&prop.space_id);
            languages.push(&prop.language);
            units.push(&prop.unit);
            text_values.push(prop.text.as_deref());
            decimal_values.push(prop.decimal.as_deref());
            boolean_values.push(prop.boolean);
            time_values.push(prop.time.as_deref());
            point_values.push(prop.point.as_deref());
            rect_values.push(prop.rect.as_deref());
            integer_values.push(prop.integer);
            float_values.push(prop.float);
            bytes_values.push(prop.bytes.as_deref());
            date_values.push(prop.date.as_deref());
            datetime_values.push(prop.datetime.as_deref());
            schedule_values.push(&prop.schedule);
            embedding_values.push(&prop.embedding);
            time_utc_values.push(prop.time_utc.as_deref());
            datetime_utc_values.push(prop.datetime_utc.as_deref());
        }

        let query = r#"
            INSERT INTO values (
                id, entity_id, property_id, space_id, language, unit,
                text, decimal, boolean, time, point, rect,
                integer, float, bytes, date, datetime, schedule, embedding,
                time_utc, datetime_utc
            )
            SELECT * FROM UNNEST(
                $1::text[],
                $2::uuid[],
                $3::uuid[],
                $4::uuid[],
                $5::text[],
                $6::text[],
                $7::text[],
                $8::numeric[],
                $9::boolean[],
                $10::text[],
                $11::text[],
                $12::text[],
                $13::bigint[],
                $14::double precision[],
                $15::bytea[],
                $16::text[],
                $17::text[],
                $18::jsonb[],
                $19::jsonb[],
                $20::timetz[],
                $21::timestamptz[]
            )
            ON CONFLICT (id) DO UPDATE SET
                language = EXCLUDED.language,
                unit = EXCLUDED.unit,
                text = EXCLUDED.text,
                decimal = EXCLUDED.decimal,
                boolean = EXCLUDED.boolean,
                time = EXCLUDED.time,
                point = EXCLUDED.point,
                rect = EXCLUDED.rect,
                integer = EXCLUDED.integer,
                float = EXCLUDED.float,
                bytes = EXCLUDED.bytes,
                date = EXCLUDED.date,
                datetime = EXCLUDED.datetime,
                schedule = EXCLUDED.schedule,
                embedding = EXCLUDED.embedding,
                time_utc = EXCLUDED.time_utc,
                datetime_utc = EXCLUDED.datetime_utc
        "#;

        sqlx::query(query)
            .bind(&ids)
            .bind(&entity_ids)
            .bind(&property_ids)
            .bind(&space_ids)
            .bind(&languages)
            .bind(&units)
            .bind(&text_values)
            .bind(&decimal_values)
            .bind(&boolean_values)
            .bind(&time_values)
            .bind(&point_values)
            .bind(&rect_values)
            .bind(&integer_values)
            .bind(&float_values)
            .bind(&bytes_values)
            .bind(&date_values)
            .bind(&datetime_values)
            .bind(&schedule_values)
            .bind(&embedding_values)
            .bind(&time_utc_values)
            .bind(&datetime_utc_values)
            .execute(&mut **tx)
            .await?;

        Ok(())
    }

    pub async fn delete_values(
        &self,
        deletes: &[(Uuid, Uuid)], // (value_id, space_id)
        tx: &mut sqlx::Transaction<'_, Postgres>,
    ) -> Result<(), IndexerError> {
        if deletes.is_empty() {
            return Ok(());
        }

        let ids: Vec<String> = deletes.iter().map(|(id, _)| id.to_string()).collect();
        let space_ids: Vec<Uuid> = deletes.iter().map(|(_, sid)| *sid).collect();

        sqlx::query(
            "DELETE FROM values WHERE (id, space_id) IN (
                SELECT id, space_id FROM UNNEST($1::text[], $2::uuid[]) AS t(id, space_id)
            )",
        )
        .bind(&ids)
        .bind(&space_ids)
        .execute(&mut **tx)
        .await?;

        Ok(())
    }

    pub async fn insert_relations(
        &self,
        relations: &[SetRelationItem],
        tx: &mut sqlx::Transaction<'_, Postgres>,
    ) -> Result<(), IndexerError> {
        if relations.is_empty() {
            return Ok(());
        }

        let mut ids = Vec::with_capacity(relations.len());
        let mut space_ids = Vec::with_capacity(relations.len());
        let mut entity_ids = Vec::with_capacity(relations.len());
        let mut from_ids = Vec::with_capacity(relations.len());
        let mut from_space_ids = Vec::with_capacity(relations.len());
        let mut to_ids = Vec::with_capacity(relations.len());
        let mut to_space_ids = Vec::with_capacity(relations.len());
        let mut type_ids = Vec::with_capacity(relations.len());
        let mut positions = Vec::with_capacity(relations.len());
        let mut verified = Vec::with_capacity(relations.len());
        let mut is_system = Vec::with_capacity(relations.len());

        for rel in relations {
            ids.push(&rel.id);
            space_ids.push(&rel.space_id);
            entity_ids.push(&rel.entity_id);
            from_ids.push(&rel.from_id);
            from_space_ids.push(&rel.from_space_id);
            to_ids.push(&rel.to_id);
            to_space_ids.push(&rel.to_space_id);
            type_ids.push(&rel.type_id);
            positions.push(&rel.position);
            verified.push(&rel.verified);
            is_system.push(rel.is_system);
        }

        let query = r#"
            INSERT INTO relations (
                id, space_id, entity_id, from_entity_id, from_space_id,
                to_entity_id, to_space_id, type_id, position, verified, is_system
            )
            SELECT * FROM UNNEST(
                $1::uuid[], $2::uuid[], $3::uuid[], $4::uuid[], $5::uuid[],
                $6::uuid[], $7::uuid[], $8::uuid[], $9::text[], $10::boolean[],
                $11::boolean[]
            )
            ON CONFLICT (id) DO UPDATE SET
                to_space_id = EXCLUDED.to_space_id,
                from_space_id = EXCLUDED.from_space_id,
                position = EXCLUDED.position,
                verified = EXCLUDED.verified
            WHERE relations.is_system = false
        "#;

        sqlx::query(query)
            .bind(&ids)
            .bind(&space_ids)
            .bind(&entity_ids)
            .bind(&from_ids)
            .bind(&from_space_ids)
            .bind(&to_ids)
            .bind(&to_space_ids)
            .bind(&type_ids)
            .bind(&positions)
            .bind(&verified)
            .bind(&is_system)
            .execute(&mut **tx)
            .await?;

        Ok(())
    }

    pub async fn update_relations(
        &self,
        relations: &[UpdateRelationItem],
        tx: &mut sqlx::Transaction<'_, Postgres>,
    ) -> Result<(), IndexerError> {
        if relations.is_empty() {
            return Ok(());
        }

        let mut query_builder: QueryBuilder<Postgres> = QueryBuilder::new(
            "UPDATE relations SET
             from_space_id = COALESCE(v.from_space_id, relations.from_space_id),
             from_version_id = COALESCE(v.from_version_id, relations.from_version_id),
             to_space_id = COALESCE(v.to_space_id, relations.to_space_id),
             to_version_id = COALESCE(v.to_version_id, relations.to_version_id),
             position = COALESCE(v.position, relations.position),
             verified = COALESCE(v.verified, relations.verified)
             FROM (",
        );

        query_builder.push_values(relations, |mut b, relation| {
            b.push_bind(relation.id);
            b.push_bind(&relation.from_space_id)
                .push_unseparated("::uuid");
            b.push_bind(&relation.from_version_id)
                .push_unseparated("::uuid");
            b.push_bind(&relation.to_space_id)
                .push_unseparated("::uuid");
            b.push_bind(&relation.to_version_id)
                .push_unseparated("::uuid");
            b.push_bind(&relation.position);
            b.push_bind(relation.verified);
        });

        query_builder.push(
            ") AS v(id, from_space_id, from_version_id, to_space_id, to_version_id, position, verified)
             WHERE relations.id = v.id AND relations.is_system = false",
        );

        query_builder.build().execute(&mut **tx).await?;

        Ok(())
    }

    pub async fn unset_relation_fields(
        &self,
        relations: &[UnsetRelationItem],
        tx: &mut sqlx::Transaction<'_, Postgres>,
    ) -> Result<(), IndexerError> {
        if relations.is_empty() {
            return Ok(());
        }

        let mut query_builder: QueryBuilder<Postgres> = QueryBuilder::new(
            "UPDATE relations SET
             from_space_id = CASE WHEN v.unset_from_space_id THEN NULL ELSE from_space_id END,
             from_version_id = CASE WHEN v.unset_from_version_id THEN NULL ELSE from_version_id END,
             to_space_id = CASE WHEN v.unset_to_space_id THEN NULL ELSE to_space_id END,
             to_version_id = CASE WHEN v.unset_to_version_id THEN NULL ELSE to_version_id END,
             position = CASE WHEN v.unset_position THEN NULL ELSE position END,
             verified = CASE WHEN v.unset_verified THEN NULL ELSE verified END
             FROM (",
        );

        query_builder.push_values(relations, |mut b, relation| {
            b.push_bind(relation.id);
            b.push_bind(relation.from_space_id.unwrap_or(false));
            b.push_bind(relation.from_version_id.unwrap_or(false));
            b.push_bind(relation.to_space_id.unwrap_or(false));
            b.push_bind(relation.to_version_id.unwrap_or(false));
            b.push_bind(relation.position.unwrap_or(false));
            b.push_bind(relation.verified.unwrap_or(false));
        });

        query_builder.push(
            ") AS v(id, unset_from_space_id, unset_from_version_id, unset_to_space_id,
                   unset_to_version_id, unset_position, unset_verified)
             WHERE relations.id = v.id AND relations.is_system = false",
        );

        query_builder.build().execute(&mut **tx).await?;

        Ok(())
    }

    pub async fn delete_relations(
        &self,
        deletes: &[(Uuid, Uuid)], // (relation_id, space_id)
        tx: &mut sqlx::Transaction<'_, Postgres>,
    ) -> Result<(), IndexerError> {
        if deletes.is_empty() {
            return Ok(());
        }

        let ids: Vec<Uuid> = deletes.iter().map(|(id, _)| *id).collect();
        let space_ids: Vec<Uuid> = deletes.iter().map(|(_, sid)| *sid).collect();

        sqlx::query(
            "DELETE FROM relations WHERE (id, space_id) IN (
                SELECT id, space_id FROM UNNEST($1::uuid[], $2::uuid[]) AS t(id, space_id)
            ) AND relations.is_system = false",
        )
        .bind(&ids)
        .bind(&space_ids)
        .execute(&mut **tx)
        .await?;

        Ok(())
    }

    pub async fn insert_spaces(
        &self,
        spaces: &[SpaceItem],
        tx: &mut sqlx::Transaction<'_, Postgres>,
    ) -> Result<(), IndexerError> {
        if spaces.is_empty() {
            return Ok(());
        }

        let mut ids: Vec<Uuid> = Vec::new();
        let mut types: Vec<String> = Vec::new();
        let mut addresses: Vec<String> = Vec::new();
        let mut topic_ids: Vec<Option<Uuid>> = Vec::new();

        for space in spaces {
            ids.push(space.id);
            types.push(match space.space_type {
                SpaceType::Personal => "Personal".to_string(),
                SpaceType::Dao => "DAO".to_string(),
            });
            addresses.push(space.address.clone());
            topic_ids.push(space.topic_id);
        }

        sqlx::query!(
            r#"
            INSERT INTO spaces (id, type, address, topic_id)
            SELECT id, type::"spaceTypes", address, topic_id
            FROM UNNEST($1::uuid[], $2::text[], $3::text[], $4::uuid[])
            AS t(id, type, address, topic_id)
            ON CONFLICT (id) DO NOTHING
            "#,
            &ids,
            &types,
            &addresses,
            &topic_ids as &[Option<Uuid>]
        )
        .execute(&mut **tx)
        .await?;

        Ok(())
    }

    pub async fn update_space_topic(
        &self,
        space_id: Uuid,
        topic_id: Uuid,
        tx: &mut sqlx::Transaction<'_, Postgres>,
    ) -> Result<(), IndexerError> {
        sqlx::query("UPDATE spaces SET topic_id = $2 WHERE id = $1")
            .bind(space_id)
            .bind(topic_id)
            .execute(&mut **tx)
            .await?;

        Ok(())
    }

    /// Clear the topic assignment from a space.
    ///
    /// Only clears when the removed `topic_id` matches the currently-set topic,
    /// guarding against a stale/out-of-order unset clearing a newer topic.
    pub async fn clear_space_topic(
        &self,
        space_id: Uuid,
        topic_id: Uuid,
        tx: &mut sqlx::Transaction<'_, Postgres>,
    ) -> Result<(), IndexerError> {
        sqlx::query("UPDATE spaces SET topic_id = NULL WHERE id = $1 AND topic_id = $2")
            .bind(space_id)
            .bind(topic_id)
            .execute(&mut **tx)
            .await?;

        Ok(())
    }

    pub async fn insert_members(
        &self,
        members: &[MemberItem],
        tx: &mut sqlx::Transaction<'_, Postgres>,
    ) -> Result<(), IndexerError> {
        if members.is_empty() {
            return Ok(());
        }

        let member_space_ids: Vec<Uuid> = members.iter().map(|m| m.member_space_id).collect();
        let space_ids: Vec<Uuid> = members.iter().map(|m| m.space_id).collect();

        sqlx::query!(
            r#"
            INSERT INTO members (member_space_id, space_id)
            SELECT member_space_id, space_id
            FROM UNNEST($1::uuid[], $2::uuid[])
            AS t(member_space_id, space_id)
            ON CONFLICT (member_space_id, space_id) DO NOTHING
            "#,
            &member_space_ids,
            &space_ids
        )
        .execute(&mut **tx)
        .await?;

        Ok(())
    }

    pub async fn remove_members(
        &self,
        members: &[MemberItem],
        tx: &mut sqlx::Transaction<'_, Postgres>,
    ) -> Result<(), IndexerError> {
        if members.is_empty() {
            return Ok(());
        }

        let member_space_ids: Vec<Uuid> = members.iter().map(|m| m.member_space_id).collect();
        let space_ids: Vec<Uuid> = members.iter().map(|m| m.space_id).collect();

        sqlx::query!(
            r#"
            DELETE FROM members
            WHERE (member_space_id, space_id) IN (
                SELECT member_space_id, space_id
                FROM UNNEST($1::uuid[], $2::uuid[])
                AS t(member_space_id, space_id)
            )
            "#,
            &member_space_ids,
            &space_ids
        )
        .execute(&mut **tx)
        .await?;

        Ok(())
    }

    pub async fn insert_editors(
        &self,
        editors: &[EditorItem],
        tx: &mut sqlx::Transaction<'_, Postgres>,
    ) -> Result<(), IndexerError> {
        if editors.is_empty() {
            return Ok(());
        }

        let member_space_ids: Vec<Uuid> = editors.iter().map(|e| e.member_space_id).collect();
        let space_ids: Vec<Uuid> = editors.iter().map(|e| e.space_id).collect();

        sqlx::query!(
            r#"
            INSERT INTO editors (member_space_id, space_id)
            SELECT member_space_id, space_id
            FROM UNNEST($1::uuid[], $2::uuid[])
            AS t(member_space_id, space_id)
            ON CONFLICT (member_space_id, space_id) DO NOTHING
            "#,
            &member_space_ids,
            &space_ids
        )
        .execute(&mut **tx)
        .await?;

        Ok(())
    }

    pub async fn remove_editors(
        &self,
        editors: &[EditorItem],
        tx: &mut sqlx::Transaction<'_, Postgres>,
    ) -> Result<(), IndexerError> {
        if editors.is_empty() {
            return Ok(());
        }

        let member_space_ids: Vec<Uuid> = editors.iter().map(|e| e.member_space_id).collect();
        let space_ids: Vec<Uuid> = editors.iter().map(|e| e.space_id).collect();

        sqlx::query!(
            r#"
            DELETE FROM editors
            WHERE (member_space_id, space_id) IN (
                SELECT member_space_id, space_id
                FROM UNNEST($1::uuid[], $2::uuid[])
                AS t(member_space_id, space_id)
            )
            "#,
            &member_space_ids,
            &space_ids
        )
        .execute(&mut **tx)
        .await?;

        Ok(())
    }

    pub async fn insert_subspaces(
        &self,
        subspaces: &[SubspaceItem],
        tx: &mut sqlx::Transaction<'_, Postgres>,
    ) -> Result<(), IndexerError> {
        if subspaces.is_empty() {
            return Ok(());
        }

        let subspace_ids: Vec<Uuid> = subspaces.iter().map(|s| s.subspace_id).collect();
        let parent_space_ids: Vec<Uuid> = subspaces.iter().map(|s| s.parent_space_id).collect();
        let types: Vec<String> = subspaces
            .iter()
            .map(|s| s.subspace_type.as_str().to_string())
            .collect();

        let query = r#"
            INSERT INTO subspaces (child_space_id, parent_space_id, type)
            SELECT child_space_id, parent_space_id, type::"subspaceType"
            FROM UNNEST($1::uuid[], $2::uuid[], $3::text[])
            AS t(child_space_id, parent_space_id, type)
            ON CONFLICT (parent_space_id, child_space_id, type) DO NOTHING
        "#;

        sqlx::query(query)
            .bind(&subspace_ids)
            .bind(&parent_space_ids)
            .bind(&types)
            .execute(&mut **tx)
            .await?;

        Ok(())
    }

    pub async fn remove_subspaces(
        &self,
        subspaces: &[SubspaceItem],
        tx: &mut sqlx::Transaction<'_, Postgres>,
    ) -> Result<u64, IndexerError> {
        if subspaces.is_empty() {
            return Ok(0);
        }

        let subspace_ids: Vec<Uuid> = subspaces.iter().map(|s| s.subspace_id).collect();
        let parent_space_ids: Vec<Uuid> = subspaces.iter().map(|s| s.parent_space_id).collect();
        let types: Vec<String> = subspaces
            .iter()
            .map(|s| s.subspace_type.as_str().to_string())
            .collect();

        let query = r#"
            DELETE FROM subspaces
            WHERE (child_space_id, parent_space_id, type) IN (
                SELECT child_space_id, parent_space_id, type::"subspaceType"
                FROM UNNEST($1::uuid[], $2::uuid[], $3::text[])
                AS t(child_space_id, parent_space_id, type)
            )
        "#;

        let result = sqlx::query(query)
            .bind(&subspace_ids)
            .bind(&parent_space_ids)
            .bind(&types)
            .execute(&mut **tx)
            .await?;

        Ok(result.rows_affected())
    }

    pub async fn insert_subspace_topics(
        &self,
        topics: &[SubspaceTopicItem],
        tx: &mut sqlx::Transaction<'_, Postgres>,
    ) -> Result<(), IndexerError> {
        if topics.is_empty() {
            return Ok(());
        }

        let space_ids: Vec<Uuid> = topics.iter().map(|t| t.space_id).collect();
        let topic_ids: Vec<Uuid> = topics.iter().map(|t| t.topic_id).collect();

        let query = r#"
            INSERT INTO subspace_topics (space_id, topic_id)
            SELECT space_id, topic_id
            FROM UNNEST($1::uuid[], $2::uuid[])
            AS t(space_id, topic_id)
            ON CONFLICT (space_id, topic_id) DO NOTHING
        "#;

        sqlx::query(query)
            .bind(&space_ids)
            .bind(&topic_ids)
            .execute(&mut **tx)
            .await?;

        Ok(())
    }

    pub async fn remove_subspace_topics(
        &self,
        topics: &[SubspaceTopicItem],
        tx: &mut sqlx::Transaction<'_, Postgres>,
    ) -> Result<u64, IndexerError> {
        if topics.is_empty() {
            return Ok(0);
        }

        let space_ids: Vec<Uuid> = topics.iter().map(|t| t.space_id).collect();
        let topic_ids: Vec<Uuid> = topics.iter().map(|t| t.topic_id).collect();

        let query = r#"
            DELETE FROM subspace_topics
            WHERE (space_id, topic_id) IN (
                SELECT space_id, topic_id
                FROM UNNEST($1::uuid[], $2::uuid[])
                AS t(space_id, topic_id)
            )
        "#;

        let result = sqlx::query(query)
            .bind(&space_ids)
            .bind(&topic_ids)
            .execute(&mut **tx)
            .await?;

        Ok(result.rows_affected())
    }

    /// Insert the immutable identity row for a new proposal.
    ///
    /// Idempotent via `ON CONFLICT (id) DO NOTHING` — a replay of the CREATE
    /// event must not clobber an existing row (notably `current_version`,
    /// which a subsequent UPDATE may have already bumped).
    #[allow(dead_code)]
    pub async fn insert_proposal_identity(
        &self,
        identity: &ProposalIdentity,
        tx: &mut sqlx::Transaction<'_, Postgres>,
    ) -> Result<(), IndexerError> {
        sqlx::query(
            r#"
            INSERT INTO proposals (id, space_id, proposed_by, created_at, created_at_block)
            VALUES ($1, $2, $3, $4, $5)
            ON CONFLICT (id) DO NOTHING
            "#,
        )
        .bind(identity.id)
        .bind(identity.space_id)
        .bind(identity.proposed_by)
        .bind(identity.created_at.to_string())
        .bind(identity.created_at_block.to_string())
        .execute(&mut **tx)
        .await?;

        Ok(())
    }

    /// Insert version 1 of a newly-created proposal.
    ///
    /// Paired with [`insert_proposal_identity`] on the CREATE path: identity
    /// row goes in first (idempotent), then the v1 version row. Also
    /// idempotent via `ON CONFLICT DO NOTHING` so CREATE replay doesn't
    /// clobber an existing row (which could be a v2+ that arrived first).
    #[allow(dead_code)]
    pub async fn insert_proposal_version_initial(
        &self,
        proposal_id: Uuid,
        version: &ProposalVersionItem,
        tx: &mut sqlx::Transaction<'_, Postgres>,
    ) -> Result<(), IndexerError> {
        let voting_mode = match version.voting_mode {
            VotingMode::Fast => "Fast",
            VotingMode::Slow => "Slow",
        };

        sqlx::query(
            r#"
            INSERT INTO proposal_versions (
                proposal_id, proposal_version, voting_mode, start_time, end_time,
                quorum, threshold,
                partial_percentage_support_threshold,
                universal_percentage_support_threshold,
                flat_support_threshold, execute_by, name,
                version_created_at, version_created_at_block
            )
            VALUES (
                $1, 1, $2::"votingMode", $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13
            )
            ON CONFLICT (proposal_id, proposal_version) DO NOTHING
            "#,
        )
        .bind(proposal_id)
        .bind(voting_mode)
        .bind(version.start_time)
        .bind(version.end_time)
        .bind(version.quorum)
        .bind(version.threshold)
        .bind(version.partial_percentage_support_threshold)
        .bind(version.universal_percentage_support_threshold)
        .bind(version.flat_support_threshold)
        .bind(version.execute_by)
        .bind(version.name.as_ref())
        .bind(version.version_created_at.to_string())
        .bind(version.version_created_at_block.to_string())
        .execute(&mut **tx)
        .await?;

        Ok(())
    }

    /// Append a new proposal version, atomically bumping `proposals.current_version`.
    ///
    /// Returns the assigned `proposal_version` number so the caller can stamp it
    /// on accompanying action rows.
    ///
    /// **Idempotent on Kafka replay.** Kafka delivery is at-least-once, so the
    /// same `ProposalUpdated` event can be redelivered. The INSERT uses
    /// `ON CONFLICT (proposal_id, version_created_at_block) DO NOTHING` against
    /// the `proposal_versions_idempotency_key` unique constraint. On replay:
    ///   * the INSERT is a no-op (the row already exists),
    ///   * `proposals.current_version` is NOT bumped (guarded by
    ///     `EXISTS (SELECT 1 FROM inserted)`), and
    ///   * we return the already-assigned `proposal_version` from the existing
    ///     row so the caller stamps actions with the same version — which also
    ///     hit their own `ON CONFLICT DO NOTHING` on
    ///     `(proposal_id, proposal_version, index)`.
    ///
    /// Tally counts on a freshly-inserted version row default to 0; the tally
    /// worker recomputes from scratch using votes scoped to this new version.
    /// Votes for prior versions remain in `proposal_votes` as history (PK
    /// includes `proposal_version`), so no delete is needed.
    ///
    /// If the proposal identity row doesn't exist yet (UPDATE arrived before
    /// CREATE — pre-mainnet this shouldn't happen since events are ordered,
    /// but the kafka consumer can be replayed from any offset), an error is
    /// returned via `HandlerError::MissingPayload`. Callers can catch and
    /// fall back to inserting the identity + this version; that's left to
    /// main.rs since the orphan case is vanishingly rare.
    #[allow(dead_code)]
    pub async fn insert_new_proposal_version(
        &self,
        proposal_id: Uuid,
        version: &ProposalVersionItem,
        tx: &mut sqlx::Transaction<'_, Postgres>,
    ) -> Result<i32, IndexerError> {
        let voting_mode = match version.voting_mode {
            VotingMode::Fast => "Fast",
            VotingMode::Slow => "Slow",
        };
        let version_created_at_block = version.version_created_at_block.to_string();

        // The INSERT is the source of truth: it either wins (fresh event) or
        // hits the idempotency-key ON CONFLICT (replay). current_version is
        // bumped only when the INSERT actually inserts a row, and we always
        // return the version stamped on the row for (proposal_id,
        // version_created_at_block) — whether newly inserted or pre-existing.
        //
        // The outer SELECT uses COALESCE and returns exactly one row; its
        // column is NULL only in the orphan case where the identity row
        // doesn't exist AND no prior version row for this idempotency key
        // exists (so both sub-selects are empty).
        let row: (Option<i32>,) = sqlx::query_as(
            r#"
            WITH
                next_version AS (
                    SELECT id, current_version + 1 AS candidate_version
                    FROM proposals
                    WHERE id = $1
                ),
                inserted AS (
                    INSERT INTO proposal_versions (
                        proposal_id, proposal_version, voting_mode, start_time, end_time,
                        quorum, threshold,
                        partial_percentage_support_threshold,
                        universal_percentage_support_threshold,
                        flat_support_threshold, execute_by, name,
                        version_created_at, version_created_at_block
                    )
                    SELECT nv.id, nv.candidate_version,
                           $2::"votingMode", $3, $4, $5, $6,
                           $7, $8, $9, $10, $11,
                           $12, $13
                    FROM next_version nv
                    ON CONFLICT (proposal_id, version_created_at_block) DO NOTHING
                    RETURNING proposal_version
                ),
                bumped AS (
                    UPDATE proposals
                    SET current_version = current_version + 1
                    WHERE id = $1
                      AND EXISTS (SELECT 1 FROM inserted)
                    RETURNING current_version
                )
            SELECT COALESCE(
                (SELECT proposal_version FROM inserted),
                (SELECT proposal_version FROM proposal_versions
                 WHERE proposal_id = $1 AND version_created_at_block = $13)
            ) AS proposal_version
            "#,
        )
        .bind(proposal_id)
        .bind(voting_mode)
        .bind(version.start_time)
        .bind(version.end_time)
        .bind(version.quorum)
        .bind(version.threshold)
        .bind(version.partial_percentage_support_threshold)
        .bind(version.universal_percentage_support_threshold)
        .bind(version.flat_support_threshold)
        .bind(version.execute_by)
        .bind(version.name.as_ref())
        .bind(version.version_created_at.to_string())
        .bind(&version_created_at_block)
        .fetch_one(&mut **tx)
        .await?;

        match row.0 {
            Some(active_version) => {
                info!(
                    proposal_id = %proposal_id,
                    proposal_version = active_version,
                    "Proposal version appended or replay-reconciled (prior versions preserved as history)"
                );
                Ok(active_version)
            }
            // Both COALESCE branches are empty → the identity row doesn't
            // exist yet AND there's no prior version for this idempotency
            // key. This is the orphan UPDATE case; caller decides how to
            // recover.
            None => Err(HandlerError::MissingPayload.into()),
        }
    }

    #[allow(dead_code)]
    pub async fn insert_proposal_actions(
        &self,
        actions: &[ProposalActionItem],
        tx: &mut sqlx::Transaction<'_, Postgres>,
    ) -> Result<(), IndexerError> {
        if actions.is_empty() {
            return Ok(());
        }

        let mut proposal_ids = Vec::with_capacity(actions.len());
        let mut proposal_versions = Vec::with_capacity(actions.len());
        let mut indexes = Vec::with_capacity(actions.len());
        let mut action_types = Vec::with_capacity(actions.len());
        let mut target_ids: Vec<Option<Uuid>> = Vec::with_capacity(actions.len());
        let mut content_uris: Vec<Option<&str>> = Vec::with_capacity(actions.len());
        let mut metadatas: Vec<Option<&[u8]>> = Vec::with_capacity(actions.len());
        let mut content_ids: Vec<Option<&[u8]>> = Vec::with_capacity(actions.len());
        // Legacy V1 columns — kept populated for backward compat with existing API queries.
        let mut quorums: Vec<Option<i64>> = Vec::with_capacity(actions.len());
        let mut fast_thresholds: Vec<Option<i64>> = Vec::with_capacity(actions.len());
        let mut slow_thresholds: Vec<Option<i64>> = Vec::with_capacity(actions.len());
        let mut durations: Vec<Option<i64>> = Vec::with_capacity(actions.len());
        // V2 columns (GEO-478 schema + GEO-481 writes).
        let mut partial_thresholds: Vec<Option<i64>> = Vec::with_capacity(actions.len());
        let mut universal_thresholds: Vec<Option<i64>> = Vec::with_capacity(actions.len());
        let mut flat_thresholds: Vec<Option<i64>> = Vec::with_capacity(actions.len());
        let mut disable_fast_path_flags: Vec<Option<bool>> = Vec::with_capacity(actions.len());
        let mut execution_grace_periods: Vec<Option<i64>> = Vec::with_capacity(actions.len());

        for action in actions {
            proposal_ids.push(action.proposal_id);
            proposal_versions.push(action.proposal_version);
            indexes.push(action.index);

            let mut target_id: Option<Uuid> = None;
            let mut content_uri: Option<&str> = None;
            let mut metadata: Option<&[u8]> = None;
            let mut content_id: Option<&[u8]> = None;
            let mut quorum: Option<i64> = None;
            let mut fast_threshold: Option<i64> = None;
            let mut slow_threshold: Option<i64> = None;
            let mut duration: Option<i64> = None;
            // V2 per-action fields
            let mut partial_threshold: Option<i64> = None;
            let mut universal_threshold: Option<i64> = None;
            let mut flat_threshold: Option<i64> = None;
            let mut disable_fast_path: Option<bool> = None;
            let mut execution_grace_period: Option<i64> = None;

            let action_type = match &action.payload {
                ProposalActionPayload::AddMember { target_id: id } => {
                    target_id = Some(*id);
                    "AddMember"
                }
                ProposalActionPayload::RemoveMember { target_id: id } => {
                    target_id = Some(*id);
                    "RemoveMember"
                }
                ProposalActionPayload::AddEditor { target_id: id } => {
                    target_id = Some(*id);
                    "AddEditor"
                }
                ProposalActionPayload::RemoveEditor { target_id: id } => {
                    target_id = Some(*id);
                    "RemoveEditor"
                }
                ProposalActionPayload::UnflagEditor { target_id: id } => {
                    target_id = Some(*id);
                    "UnflagEditor"
                }
                ProposalActionPayload::Publish {
                    content_uri: uri,
                    metadata: meta,
                } => {
                    content_uri = Some(uri.as_str());
                    if !meta.is_empty() {
                        metadata = Some(meta.as_slice());
                    }
                    "Publish"
                }
                ProposalActionPayload::Flag { content_id: id } => {
                    content_id = Some(id.as_slice());
                    "Flag"
                }
                ProposalActionPayload::Unflag { content_id: id } => {
                    content_id = Some(id.as_slice());
                    "Unflag"
                }
                // V2 UpdateVotingSettings. Populate the 5 V2 columns directly
                // (GEO-478 schema) AND keep the legacy V1 columns populated for
                // backward compat with existing API queries (fast=flat, slow=partial).
                ProposalActionPayload::UpdateVotingSettings {
                    partial_percentage_support_threshold: partial,
                    universal_percentage_support_threshold: universal,
                    flat_support_threshold: flat,
                    quorum: q,
                    duration: d,
                    disable_fast_path_access_for_new_members: disable_fp,
                    execution_grace_period: grace,
                } => {
                    // V1 bridge
                    quorum = Some(*q as i64);
                    fast_threshold = Some(*flat as i64);
                    slow_threshold = Some(*partial as i64);
                    duration = Some(*d as i64);
                    // V2 columns
                    partial_threshold = Some(*partial as i64);
                    universal_threshold = Some(*universal as i64);
                    flat_threshold = Some(*flat as i64);
                    disable_fast_path = Some(*disable_fp);
                    execution_grace_period = Some(*grace as i64);
                    "UpdateVotingSettings"
                }
                ProposalActionPayload::SubspaceVerified {
                    target_space_id: id,
                } => {
                    target_id = Some(*id);
                    "SubspaceVerified"
                }
                ProposalActionPayload::SubspaceUnverified {
                    target_space_id: id,
                } => {
                    target_id = Some(*id);
                    "SubspaceUnverified"
                }
                ProposalActionPayload::SubspaceRelated {
                    target_space_id: id,
                } => {
                    target_id = Some(*id);
                    "SubspaceRelated"
                }
                ProposalActionPayload::SubspaceUnrelated {
                    target_space_id: id,
                } => {
                    target_id = Some(*id);
                    "SubspaceUnrelated"
                }
                ProposalActionPayload::SubspaceTopicDeclared {
                    target_topic_id: id,
                } => {
                    target_id = Some(*id);
                    "SubspaceTopicDeclared"
                }
                ProposalActionPayload::SubspaceTopicRemoved {
                    target_topic_id: id,
                } => {
                    target_id = Some(*id);
                    "SubspaceTopicRemoved"
                }
                ProposalActionPayload::SetTopic {
                    target_topic_id: id,
                } => {
                    target_id = Some(*id);
                    "SetTopic"
                }
                ProposalActionPayload::UnsetTopic {
                    target_topic_id: id,
                } => {
                    target_id = Some(*id);
                    "UnsetTopic"
                }
                ProposalActionPayload::Unknown => "Unknown",
            };

            action_types.push(action_type);
            target_ids.push(target_id);
            content_uris.push(content_uri);
            metadatas.push(metadata);
            content_ids.push(content_id);
            quorums.push(quorum);
            fast_thresholds.push(fast_threshold);
            slow_thresholds.push(slow_threshold);
            durations.push(duration);
            partial_thresholds.push(partial_threshold);
            universal_thresholds.push(universal_threshold);
            flat_thresholds.push(flat_threshold);
            disable_fast_path_flags.push(disable_fast_path);
            execution_grace_periods.push(execution_grace_period);
        }

        let query = r#"
            INSERT INTO proposal_actions (
                proposal_id, proposal_version, index, action_type,
                target_id, content_uri, metadata, content_id,
                quorum, fast_threshold, slow_threshold, duration,
                partial_percentage_support_threshold,
                universal_percentage_support_threshold,
                flat_support_threshold,
                disable_fast_path_access_for_new_members,
                execution_grace_period
            )
            SELECT proposal_id, proposal_version, index, action_type::"proposalActionType",
                   target_id, content_uri, metadata, content_id,
                   quorum, fast_threshold, slow_threshold, duration,
                   partial_percentage_support_threshold,
                   universal_percentage_support_threshold,
                   flat_support_threshold,
                   disable_fast_path_access_for_new_members,
                   execution_grace_period
            FROM UNNEST(
                $1::uuid[], $2::integer[], $3::integer[], $4::text[],
                $5::uuid[], $6::text[], $7::bytea[], $8::bytea[],
                $9::bigint[], $10::bigint[], $11::bigint[], $12::bigint[],
                $13::bigint[], $14::bigint[], $15::bigint[],
                $16::boolean[], $17::bigint[]
            ) AS t(proposal_id, proposal_version, index, action_type,
                   target_id, content_uri, metadata, content_id,
                   quorum, fast_threshold, slow_threshold, duration,
                   partial_percentage_support_threshold,
                   universal_percentage_support_threshold,
                   flat_support_threshold,
                   disable_fast_path_access_for_new_members,
                   execution_grace_period)
            ON CONFLICT (proposal_id, proposal_version, index) DO NOTHING
        "#;

        sqlx::query(query)
            .bind(&proposal_ids)
            .bind(&proposal_versions)
            .bind(&indexes)
            .bind(&action_types)
            .bind(&target_ids)
            .bind(&content_uris)
            .bind(&metadatas)
            .bind(&content_ids)
            .bind(&quorums)
            .bind(&fast_thresholds)
            .bind(&slow_thresholds)
            .bind(&durations)
            .bind(&partial_thresholds)
            .bind(&universal_thresholds)
            .bind(&flat_thresholds)
            .bind(&disable_fast_path_flags)
            .bind(&execution_grace_periods)
            .execute(&mut **tx)
            .await?;

        Ok(())
    }

    /// Insert proposal votes, scoped by `proposal_version`.
    ///
    /// V2 (GEO-481): votes are non-destructive across proposal updates. The
    /// primary key is `(proposal_id, proposal_version, voter_id)` — votes from
    /// prior versions live forever as history, and tally queries filter by
    /// `proposal_version = proposals.proposal_version` to count only the
    /// current version.
    ///
    /// A vote event with a `proposal_version` that doesn't match the current
    /// proposal row simply lands in its own version's bucket. This is not an
    /// error — it's the mechanism that makes vote-vs-update event ordering
    /// safe: if a vote event arrives before the UPDATE that bumps the version
    /// (or after, referencing a newer version than we've processed), the row
    /// exists in the right bucket when tally computation catches up.
    #[allow(dead_code)]
    pub async fn insert_proposal_votes(
        &self,
        votes: &[ProposalVoteItem],
        tx: &mut sqlx::Transaction<'_, Postgres>,
    ) -> Result<(), IndexerError> {
        if votes.is_empty() {
            return Ok(());
        }

        let mut proposal_ids = Vec::with_capacity(votes.len());
        let mut proposal_versions = Vec::with_capacity(votes.len());
        let mut voter_ids = Vec::with_capacity(votes.len());
        let mut space_ids = Vec::with_capacity(votes.len());
        let mut vote_options = Vec::with_capacity(votes.len());
        let mut created_ats = Vec::with_capacity(votes.len());
        let mut created_at_blocks = Vec::with_capacity(votes.len());

        for vote in votes {
            proposal_ids.push(vote.proposal_id);
            proposal_versions.push(vote.proposal_version);
            voter_ids.push(vote.voter_id);
            space_ids.push(vote.space_id);
            vote_options.push(match vote.vote {
                VoteOption::Yes => "Yes",
                VoteOption::No => "No",
                VoteOption::Abstain => "Abstain",
            });
            created_ats.push(vote.created_at.to_string());
            created_at_blocks.push(vote.created_at_block.to_string());
        }

        let query = r#"
            INSERT INTO proposal_votes (
                proposal_id, proposal_version, voter_id, space_id, vote, created_at, created_at_block
            )
            SELECT proposal_id, proposal_version, voter_id, space_id,
                   vote::"voteOption", created_at, created_at_block
            FROM UNNEST(
                $1::uuid[], $2::integer[], $3::uuid[], $4::uuid[],
                $5::text[], $6::text[], $7::text[]
            ) AS t(proposal_id, proposal_version, voter_id, space_id, vote,
                   created_at, created_at_block)
            ON CONFLICT (proposal_id, proposal_version, voter_id) DO UPDATE SET
                vote = EXCLUDED.vote::"voteOption",
                created_at = EXCLUDED.created_at,
                created_at_block = EXCLUDED.created_at_block
        "#;

        sqlx::query(query)
            .bind(&proposal_ids)
            .bind(&proposal_versions)
            .bind(&voter_ids)
            .bind(&space_ids)
            .bind(&vote_options)
            .bind(&created_ats)
            .bind(&created_at_blocks)
            .execute(&mut **tx)
            .await?;

        Ok(())
    }

    /// Stamp `executed_at` on a proposal.
    ///
    /// Identity-level: execution applies to the proposal as a whole, not to
    /// any specific version. Only one execution per proposal is possible by
    /// contract design.
    #[allow(dead_code)]
    pub async fn update_proposal_executed(
        &self,
        proposal_id: Uuid,
        executed_at: i64,
        tx: &mut sqlx::Transaction<'_, Postgres>,
    ) -> Result<(), IndexerError> {
        sqlx::query(
            r#"
            UPDATE proposals
            SET executed_at = $1
            WHERE id = $2
            "#,
        )
        .bind(executed_at)
        .bind(proposal_id)
        .execute(&mut **tx)
        .await?;

        Ok(())
    }

    /// Update voting settings on the current version of a proposal in place
    /// (fast→slow escalation).
    ///
    /// Escalation does NOT create a new version per the contract semantics —
    /// it rewrites settings on the current version's row. Votes cast under
    /// this version are retained; tally counts are NOT reset.
    #[allow(dead_code, clippy::too_many_arguments)]
    pub async fn update_proposal_settings(
        &self,
        proposal_id: Uuid,
        voting_mode: &str,
        start_time: i64,
        end_time: i64,
        quorum: i64,
        threshold: i64,
        partial_percentage_support_threshold: i64,
        universal_percentage_support_threshold: i64,
        flat_support_threshold: i64,
        execute_by: Option<i64>,
        tx: &mut sqlx::Transaction<'_, Postgres>,
    ) -> Result<(), IndexerError> {
        sqlx::query(
            r#"
            UPDATE proposal_versions pv
            SET voting_mode = $2::"votingMode",
                start_time = $3,
                end_time = $4,
                quorum = $5,
                threshold = $6,
                partial_percentage_support_threshold = $7,
                universal_percentage_support_threshold = $8,
                flat_support_threshold = $9,
                execute_by = $10
            FROM proposals p
            WHERE p.id = pv.proposal_id
              AND pv.proposal_version = p.current_version
              AND p.id = $1
            "#,
        )
        .bind(proposal_id)
        .bind(voting_mode)
        .bind(start_time)
        .bind(end_time)
        .bind(quorum)
        .bind(threshold)
        .bind(partial_percentage_support_threshold)
        .bind(universal_percentage_support_threshold)
        .bind(flat_support_threshold)
        .bind(execute_by)
        .execute(&mut **tx)
        .await?;

        Ok(())
    }

    /// Upsert DAO-global voting settings for a space.
    ///
    /// On conflict, all settings fields are overwritten with the new values.
    /// Editor count is not stored here — compute it via the `editors` table
    /// (see the `space_editor_counts` view, GEO-514).
    pub async fn upsert_space_voting_settings(
        &self,
        settings: &SpaceVotingSettingsItem,
        tx: &mut sqlx::Transaction<'_, Postgres>,
    ) -> Result<(), IndexerError> {
        sqlx::query(
            r#"
            INSERT INTO space_voting_settings (
                space_id,
                partial_percentage_support_threshold,
                universal_percentage_support_threshold,
                flat_support_threshold,
                quorum,
                duration,
                disable_fast_path_access_for_new_members,
                execution_grace_period,
                updated_at,
                updated_at_block
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
            ON CONFLICT (space_id) DO UPDATE SET
                partial_percentage_support_threshold =
                    EXCLUDED.partial_percentage_support_threshold,
                universal_percentage_support_threshold =
                    EXCLUDED.universal_percentage_support_threshold,
                flat_support_threshold = EXCLUDED.flat_support_threshold,
                quorum = EXCLUDED.quorum,
                duration = EXCLUDED.duration,
                disable_fast_path_access_for_new_members =
                    EXCLUDED.disable_fast_path_access_for_new_members,
                execution_grace_period = EXCLUDED.execution_grace_period,
                updated_at = EXCLUDED.updated_at,
                updated_at_block = EXCLUDED.updated_at_block
            "#,
        )
        .bind(settings.space_id)
        .bind(settings.partial_percentage_support_threshold)
        .bind(settings.universal_percentage_support_threshold)
        .bind(settings.flat_support_threshold)
        .bind(settings.quorum)
        .bind(settings.duration)
        .bind(settings.disable_fast_path_access_for_new_members)
        .bind(settings.execution_grace_period)
        .bind(settings.updated_at.to_string())
        .bind(settings.updated_at_block.to_string())
        .execute(&mut **tx)
        .await?;

        Ok(())
    }

    /// Queue a proposal for vote tally update.
    /// Uses INSERT ON CONFLICT DO NOTHING to avoid duplicate entries.
    /// The background worker will process the queue and update denormalized vote counts.
    #[allow(dead_code)]
    pub async fn queue_tally_update(
        &self,
        proposal_id: Uuid,
        tx: &mut sqlx::Transaction<'_, Postgres>,
    ) -> Result<(), IndexerError> {
        sqlx::query(
            r#"
            INSERT INTO proposal_tally_queue (proposal_id)
            VALUES ($1)
            ON CONFLICT (proposal_id) DO NOTHING
            "#,
        )
        .bind(proposal_id)
        .execute(&mut **tx)
        .await?;

        Ok(())
    }

    /// Process the proposal tally queue: fetch queued proposal IDs, update their
    /// denormalized vote counts, and remove them from the queue.
    /// Returns the number of proposals processed.
    pub async fn process_tally_queue(&self, batch_size: i64) -> Result<usize, IndexerError> {
        // Use a transaction to atomically:
        // 1. Grab and lock a batch of queued proposals
        // 2. Update their vote counts
        // 3. Remove them from the queue
        let mut tx = self.pool.begin().await?;

        // Grab a batch of proposal IDs from the queue with FOR UPDATE SKIP LOCKED
        // This allows multiple workers to process concurrently without conflicts
        let queued: Vec<(Uuid,)> = sqlx::query_as(
            r#"
            SELECT proposal_id
            FROM proposal_tally_queue
            ORDER BY queued_at ASC
            LIMIT $1
            FOR UPDATE SKIP LOCKED
            "#,
        )
        .bind(batch_size)
        .fetch_all(&mut *tx)
        .await?;

        if queued.is_empty() {
            return Ok(0);
        }

        let proposal_ids: Vec<Uuid> = queued.into_iter().map(|(id,)| id).collect();
        let count = proposal_ids.len();

        // Update vote counts on the current `proposal_versions` row for each
        // queued proposal.
        //
        // V2 full lifecycle versioning: all mutable state (including tally
        // counts) lives on `proposal_versions`, scoped by `proposal_version`.
        // The aggregate below filters votes to the current version only
        // (`pv.proposal_version = p.current_version`) — prior-version votes
        // remain as history. LEFT JOIN handles proposals with zero
        // current-version votes (e.g. right after a version bump).
        sqlx::query(
            r#"
            UPDATE proposal_versions pvs
            SET
                yes_count = COALESCE(vc.yes_count, 0),
                no_count = COALESCE(vc.no_count, 0),
                abstain_count = COALESCE(vc.abstain_count, 0)
            FROM UNNEST($1::uuid[]) AS queued(proposal_id)
            JOIN proposals p ON p.id = queued.proposal_id
            LEFT JOIN (
                SELECT
                    pv.proposal_id,
                    pv.proposal_version,
                    COUNT(*) FILTER (WHERE pv.vote = 'Yes') as yes_count,
                    COUNT(*) FILTER (WHERE pv.vote = 'No') as no_count,
                    COUNT(*) FILTER (WHERE pv.vote = 'Abstain') as abstain_count
                FROM proposal_votes pv
                JOIN proposals cp
                    ON cp.id = pv.proposal_id
                    AND cp.current_version = pv.proposal_version
                WHERE pv.proposal_id = ANY($1)
                GROUP BY pv.proposal_id, pv.proposal_version
            ) vc ON vc.proposal_id = p.id AND vc.proposal_version = p.current_version
            WHERE pvs.proposal_id = p.id
              AND pvs.proposal_version = p.current_version
            "#,
        )
        .bind(&proposal_ids)
        .execute(&mut *tx)
        .await?;

        // Remove processed proposals from the queue
        sqlx::query(
            r#"
            DELETE FROM proposal_tally_queue
            WHERE proposal_id = ANY($1)
            "#,
        )
        .bind(&proposal_ids)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;

        Ok(count)
    }

    /// Get the current depth of the proposal tally queue.
    /// Useful for monitoring worker health and detecting backlogs.
    pub async fn get_tally_queue_depth(&self) -> Result<i64, IndexerError> {
        let result: (i64,) = sqlx::query_as(r#"SELECT COUNT(*) FROM proposal_tally_queue"#)
            .fetch_one(&self.pool)
            .await?;

        Ok(result.0)
    }

    // ==================== Versioned writes ====================

    /// Derive a deterministic version ID from entity, property, space, and version_key.
    /// This ensures idempotency - the same inputs always produce the same ID.
    fn derive_value_version_id(
        entity_id: &Uuid,
        property_id: &Uuid,
        space_id: &Uuid,
        version_key: i64,
    ) -> Uuid {
        let mut hasher = DefaultHasher::new();
        entity_id.hash(&mut hasher);
        property_id.hash(&mut hasher);
        space_id.hash(&mut hasher);
        version_key.hash(&mut hasher);
        let hash_value = hasher.finish();

        let mut bytes = [0u8; 16];
        bytes[0..8].copy_from_slice(&hash_value.to_be_bytes());
        // Use a different seed for the second half to avoid patterns
        let mut hasher2 = DefaultHasher::new();
        hash_value.hash(&mut hasher2);
        bytes[8..16].copy_from_slice(&hasher2.finish().to_be_bytes());

        Uuid::from_bytes(bytes)
    }

    /// Derive a deterministic version ID for relation versions.
    fn derive_relation_version_id(relation_id: &Uuid, space_id: &Uuid, version_key: i64) -> Uuid {
        let mut hasher = DefaultHasher::new();
        relation_id.hash(&mut hasher);
        space_id.hash(&mut hasher);
        version_key.hash(&mut hasher);
        let hash_value = hasher.finish();

        let mut bytes = [0u8; 16];
        bytes[0..8].copy_from_slice(&hash_value.to_be_bytes());
        let mut hasher2 = DefaultHasher::new();
        hash_value.hash(&mut hasher2);
        bytes[8..16].copy_from_slice(&hasher2.finish().to_be_bytes());

        Uuid::from_bytes(bytes)
    }

    /// Insert an edit version record and return the computed version_key.
    /// version_key = (block_number << 32) | sequence
    ///
    /// Returns `Some(version_key)` if the edit was inserted, `None` if it already existed.
    ///
    /// # Idempotency
    ///
    /// Callers MUST skip `insert_value_versions` and `insert_relation_versions` when this
    /// returns `None`. Re-processing an edit would otherwise corrupt temporal data:
    ///
    /// 1. First processing: inserts value_version with valid_from_key=X, valid_to_key=NULL
    /// 2. Re-processing: close query sets valid_to_key=X on that row, then inserts duplicate
    /// 3. Result: original row has valid_from_key=X, valid_to_key=X (zero-length interval)
    ///
    /// The ON CONFLICT on edit_versions detects re-processing, but value_versions has no
    /// such protection - it relies on callers checking this return value.
    #[allow(clippy::too_many_arguments)]
    pub async fn insert_edit_version(
        &self,
        edit_id: Uuid,
        block_number: i64,
        sequence: i64,
        created_at: i64,
        name: Option<&str>,
        created_by_id: Option<Uuid>,
        tx: &mut sqlx::Transaction<'_, Postgres>,
    ) -> Result<Option<i64>, IndexerError> {
        let version_key = (block_number << 32) | sequence;

        let result = sqlx::query(
            r#"
            INSERT INTO edit_versions (edit_id, block_number, sequence, version_key, created_at, name, created_by_id)
            VALUES ($1, $2, $3, $4, to_timestamp($5), $6, $7)
            ON CONFLICT (edit_id) DO NOTHING
            "#,
        )
        .bind(edit_id)
        .bind(block_number)
        .bind(sequence)
        .bind(version_key)
        .bind(created_at as f64)
        .bind(name)
        .bind(created_by_id)
        .execute(&mut **tx)
        .await?;

        if result.rows_affected() == 0 {
            Ok(None)
        } else {
            Ok(Some(version_key))
        }
    }

    /// Insert versioned value records with temporal range tracking.
    /// For each value:
    /// 1. Close any open version (SET valid_to_key WHERE valid_to_key IS NULL)
    /// 2. Insert new version row (only for SET operations, not DELETE)
    pub async fn insert_value_versions(
        &self,
        values: &[ValueOp],
        version_key: i64,
        tx: &mut sqlx::Transaction<'_, Postgres>,
    ) -> Result<(), IndexerError> {
        if values.is_empty() {
            return Ok(());
        }

        // Collect keys for closing open versions
        let entity_ids: Vec<Uuid> = values.iter().map(|v| v.entity_id).collect();
        let property_ids: Vec<Uuid> = values.iter().map(|v| v.property_id).collect();
        let space_ids: Vec<Uuid> = values.iter().map(|v| v.space_id).collect();

        // Close all open versions for these (entity, property, space) tuples
        sqlx::query(
            r#"
            UPDATE value_versions
            SET valid_to_key = $1
            WHERE valid_to_key IS NULL
              AND (entity_id, property_id, space_id) IN (
                SELECT entity_id, property_id, space_id
                FROM UNNEST($2::uuid[], $3::uuid[], $4::uuid[])
                AS t(entity_id, property_id, space_id)
              )
            "#,
        )
        .bind(version_key)
        .bind(&entity_ids)
        .bind(&property_ids)
        .bind(&space_ids)
        .execute(&mut **tx)
        .await?;

        // Filter to only SET operations for inserting new versions
        let set_values: Vec<&ValueOp> = values
            .iter()
            .filter(|v| matches!(v.change_type, ValueChangeType::Set))
            .collect();

        if set_values.is_empty() {
            return Ok(());
        }

        // Prepare arrays for bulk insert
        let mut ids = Vec::with_capacity(set_values.len());
        let mut v_entity_ids = Vec::with_capacity(set_values.len());
        let mut v_property_ids = Vec::with_capacity(set_values.len());
        let mut v_space_ids = Vec::with_capacity(set_values.len());
        let mut valid_from_keys = Vec::with_capacity(set_values.len());
        let mut languages = Vec::with_capacity(set_values.len());
        let mut units = Vec::with_capacity(set_values.len());
        let mut texts = Vec::with_capacity(set_values.len());
        let mut booleans = Vec::with_capacity(set_values.len());
        let mut decimals = Vec::with_capacity(set_values.len());
        let mut times = Vec::with_capacity(set_values.len());
        let mut points = Vec::with_capacity(set_values.len());
        let mut rects = Vec::with_capacity(set_values.len());
        let mut integers = Vec::with_capacity(set_values.len());
        let mut floats = Vec::with_capacity(set_values.len());
        let mut bytes_values: Vec<Option<&[u8]>> = Vec::with_capacity(set_values.len());
        let mut dates = Vec::with_capacity(set_values.len());
        let mut datetimes = Vec::with_capacity(set_values.len());
        let mut schedules = Vec::with_capacity(set_values.len());
        let mut embeddings = Vec::with_capacity(set_values.len());
        let mut time_utcs = Vec::with_capacity(set_values.len());
        let mut datetime_utcs = Vec::with_capacity(set_values.len());
        let mut context_root_ids = Vec::with_capacity(set_values.len());
        let mut context_edge_type_ids = Vec::with_capacity(set_values.len());
        let mut context_last_to_entity_ids = Vec::with_capacity(set_values.len());

        for v in &set_values {
            // Derive deterministic ID for idempotency
            ids.push(Self::derive_value_version_id(
                &v.entity_id,
                &v.property_id,
                &v.space_id,
                version_key,
            ));
            v_entity_ids.push(v.entity_id);
            v_property_ids.push(v.property_id);
            v_space_ids.push(v.space_id);
            valid_from_keys.push(version_key);
            languages.push(v.language.as_deref());
            units.push(v.unit.as_deref());
            texts.push(v.text.as_deref());
            booleans.push(v.boolean);
            decimals.push(v.decimal.as_deref());
            times.push(v.time.as_deref());
            points.push(v.point.as_deref());
            rects.push(v.rect.as_deref());
            integers.push(v.integer);
            floats.push(v.float);
            bytes_values.push(v.bytes.as_deref());
            dates.push(v.date.as_deref());
            datetimes.push(v.datetime.as_deref());
            schedules.push(&v.schedule);
            embeddings.push(&v.embedding);
            time_utcs.push(v.time_utc.as_deref());
            datetime_utcs.push(v.datetime_utc.as_deref());
            context_root_ids.push(v.context_root_id);
            context_edge_type_ids.push(v.context_edge_type_id);
            context_last_to_entity_ids.push(v.context_last_to_entity_id);
        }

        sqlx::query(
            r#"
            INSERT INTO value_versions (
                id, entity_id, property_id, space_id, valid_from_key,
                language, unit, text, boolean, decimal, time, point, rect,
                integer, float, bytes, date, datetime, schedule, embedding,
                time_utc, datetime_utc, context_root_id, context_edge_type_id,
                context_last_to_entity_id
            )
            SELECT * FROM UNNEST(
                $1::uuid[], $2::uuid[], $3::uuid[], $4::uuid[], $5::bigint[],
                $6::text[], $7::text[], $8::text[], $9::boolean[], $10::numeric[],
                $11::text[], $12::text[], $13::text[], $14::bigint[], $15::double precision[],
                $16::bytea[], $17::text[], $18::text[], $19::jsonb[], $20::jsonb[],
                $21::timetz[], $22::timestamptz[], $23::uuid[], $24::uuid[],
                $25::uuid[]
            )
            ON CONFLICT (id) DO NOTHING
            "#,
        )
        .bind(&ids)
        .bind(&v_entity_ids)
        .bind(&v_property_ids)
        .bind(&v_space_ids)
        .bind(&valid_from_keys)
        .bind(&languages)
        .bind(&units)
        .bind(&texts)
        .bind(&booleans)
        .bind(&decimals)
        .bind(&times)
        .bind(&points)
        .bind(&rects)
        .bind(&integers)
        .bind(&floats)
        .bind(&bytes_values)
        .bind(&dates)
        .bind(&datetimes)
        .bind(&schedules)
        .bind(&embeddings)
        .bind(&time_utcs)
        .bind(&datetime_utcs)
        .bind(&context_root_ids)
        .bind(&context_edge_type_ids)
        .bind(&context_last_to_entity_ids)
        .execute(&mut **tx)
        .await?;

        Ok(())
    }

    /// Insert versioned relation records with temporal range tracking.
    /// For each relation operation:
    /// 1. Close any open version (SET valid_to_key WHERE valid_to_key IS NULL)
    /// 2. Insert new version row (only for Create operations)
    pub async fn insert_relation_versions(
        &self,
        relations: &[RelationOp],
        version_key: i64,
        tx: &mut sqlx::Transaction<'_, Postgres>,
    ) -> Result<(), IndexerError> {
        if relations.is_empty() {
            return Ok(());
        }

        // Collect relation IDs and space IDs for closing open versions
        let relation_ids: Vec<Uuid> = relations.iter().map(|r| r.id()).collect();
        let space_ids: Vec<Uuid> = relations.iter().map(|r| r.space_id()).collect();

        // Close all open versions for these relations
        sqlx::query(
            r#"
            UPDATE relation_versions
            SET valid_to_key = $1
            WHERE valid_to_key IS NULL
              AND (relation_id, space_id) IN (
                SELECT relation_id, space_id
                FROM UNNEST($2::uuid[], $3::uuid[])
                AS t(relation_id, space_id)
              )
            "#,
        )
        .bind(version_key)
        .bind(&relation_ids)
        .bind(&space_ids)
        .execute(&mut **tx)
        .await?;

        // Filter to only Create operations for inserting new versions. The Create
        // row carries the context columns (incl. context_last_to_entity_id)
        // synthesized in-memory — no reads.
        //
        // NOTE: versioning Update/Unset ops (so an in-place relation field change
        // is reflected in the temporal table rather than reading as deleted at the
        // new version_key) is a separate write-path correctness fix tracked in its
        // own PR — intentionally not bundled here to keep this change read-free.
        let creates: Vec<&SetRelationItem> = relations
            .iter()
            .filter_map(|r| match r {
                RelationOp::Create(item) => Some(item),
                _ => None,
            })
            .collect();

        if creates.is_empty() {
            return Ok(());
        }

        // Prepare arrays for bulk insert
        let mut ids = Vec::with_capacity(creates.len());
        let mut r_relation_ids = Vec::with_capacity(creates.len());
        let mut r_entity_ids = Vec::with_capacity(creates.len());
        let mut r_type_ids = Vec::with_capacity(creates.len());
        let mut r_from_entity_ids = Vec::with_capacity(creates.len());
        let mut r_from_space_ids: Vec<Option<Uuid>> = Vec::with_capacity(creates.len());
        let mut r_to_entity_ids = Vec::with_capacity(creates.len());
        let mut r_to_space_ids: Vec<Option<Uuid>> = Vec::with_capacity(creates.len());
        let mut r_positions = Vec::with_capacity(creates.len());
        let mut r_space_ids = Vec::with_capacity(creates.len());
        let mut r_verified = Vec::with_capacity(creates.len());
        let mut valid_from_keys = Vec::with_capacity(creates.len());
        let mut context_root_ids = Vec::with_capacity(creates.len());
        let mut context_edge_type_ids = Vec::with_capacity(creates.len());
        let mut context_last_to_entity_ids = Vec::with_capacity(creates.len());

        for r in &creates {
            // Derive deterministic ID for idempotency
            ids.push(Self::derive_relation_version_id(
                &r.id,
                &r.space_id,
                version_key,
            ));
            r_relation_ids.push(r.id);
            r_entity_ids.push(r.entity_id);
            r_type_ids.push(r.type_id);
            r_from_entity_ids.push(r.from_id);
            r_from_space_ids.push(r.from_space_id.as_ref().and_then(|s| s.parse().ok()));
            r_to_entity_ids.push(r.to_id);
            r_to_space_ids.push(r.to_space_id.as_ref().and_then(|s| s.parse().ok()));
            r_positions.push(r.position.as_deref());
            r_space_ids.push(r.space_id);
            r_verified.push(r.verified);
            valid_from_keys.push(version_key);
            context_root_ids.push(r.context_root_id);
            context_edge_type_ids.push(r.context_edge_type_id);
            context_last_to_entity_ids.push(r.context_last_to_entity_id);
        }

        sqlx::query(
            r#"
            INSERT INTO relation_versions (
                id, relation_id, entity_id, type_id, from_entity_id, from_space_id,
                to_entity_id, to_space_id, position, space_id, verified, valid_from_key,
                context_root_id, context_edge_type_id, context_last_to_entity_id
            )
            SELECT * FROM UNNEST(
                $1::uuid[], $2::uuid[], $3::uuid[], $4::uuid[], $5::uuid[], $6::uuid[],
                $7::uuid[], $8::uuid[], $9::text[], $10::uuid[], $11::boolean[], $12::bigint[],
                $13::uuid[], $14::uuid[], $15::uuid[]
            )
            ON CONFLICT (id) DO NOTHING
            "#,
        )
        .bind(&ids)
        .bind(&r_relation_ids)
        .bind(&r_entity_ids)
        .bind(&r_type_ids)
        .bind(&r_from_entity_ids)
        .bind(&r_from_space_ids)
        .bind(&r_to_entity_ids)
        .bind(&r_to_space_ids)
        .bind(&r_positions)
        .bind(&r_space_ids)
        .bind(&r_verified)
        .bind(&valid_from_keys)
        .bind(&context_root_ids)
        .bind(&context_edge_type_ids)
        .bind(&context_last_to_entity_ids)
        .execute(&mut **tx)
        .await?;

        Ok(())
    }
}
