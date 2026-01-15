use sqlx::{postgres::PgPoolOptions, Postgres, QueryBuilder};
use uuid::Uuid;

use crate::error::IndexerError;
use crate::models::{
    entities::EntityItem,
    governance::{
        ProposalActionItem, ProposalActionPayload, ProposalItem, ProposalVoteItem, VoteOption,
        VotingMode,
    },
    membership::{EditorItem, MemberItem},
    properties::{
        DataType, PropertyItem, DATA_TYPE_BOOLEAN, DATA_TYPE_NUMBER, DATA_TYPE_POINT,
        DATA_TYPE_RELATION, DATA_TYPE_STRING, DATA_TYPE_TIME,
    },
    relations::{SetRelationItem, UnsetRelationItem, UpdateRelationItem},
    spaces::{SpaceItem, SpaceType},
    subspaces::SubspaceItem,
    values::ValueOp,
};

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
        let mut text_values = Vec::with_capacity(values.len());
        let mut number_values = Vec::with_capacity(values.len());
        let mut boolean_values = Vec::with_capacity(values.len());
        let mut time_values = Vec::with_capacity(values.len());
        let mut point_values = Vec::with_capacity(values.len());

        for prop in values {
            ids.push(prop.id.to_string());
            entity_ids.push(&prop.entity_id);
            property_ids.push(&prop.property_id);
            space_ids.push(&prop.space_id);
            languages.push(&prop.language);
            units.push(&prop.unit);
            text_values.push(prop.string.as_deref());
            number_values.push(prop.number);
            boolean_values.push(prop.boolean);
            time_values.push(prop.time.as_deref());
            point_values.push(prop.point.as_deref());
        }

        let query = r#"
            INSERT INTO values (
                id, entity_id, property_id, space_id, language, unit,
                string, number, boolean, time, point
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
                $11::text[]
            )
            ON CONFLICT (id) DO UPDATE SET
                language = EXCLUDED.language,
                unit = EXCLUDED.unit,
                string = EXCLUDED.string,
                number = EXCLUDED.number,
                boolean = EXCLUDED.boolean,
                time = EXCLUDED.time,
                point = EXCLUDED.point
        "#;

        sqlx::query(query)
            .bind(&ids)
            .bind(&entity_ids)
            .bind(&property_ids)
            .bind(&space_ids)
            .bind(&languages)
            .bind(&units)
            .bind(&text_values)
            .bind(&number_values)
            .bind(&boolean_values)
            .bind(&time_values)
            .bind(&point_values)
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
        }

        let query = r#"
            INSERT INTO relations (
                id, space_id, entity_id, from_entity_id, from_space_id,
                to_entity_id, to_space_id, type_id, position, verified
            )
            SELECT * FROM UNNEST(
                $1::uuid[], $2::uuid[], $3::uuid[], $4::uuid[], $5::uuid[],
                $6::uuid[], $7::uuid[], $8::uuid[], $9::text[], $10::boolean[]
            )
            ON CONFLICT (id) DO UPDATE SET
                to_space_id = EXCLUDED.to_space_id,
                from_space_id = EXCLUDED.from_space_id,
                position = EXCLUDED.position,
                verified = EXCLUDED.verified
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
             FROM (VALUES ",
        );

        query_builder.push_values(relations, |mut b, relation| {
            b.push_bind(relation.id);
            b.push_bind(&relation.from_space_id);
            b.push_bind(&relation.from_version_id);
            b.push_bind(&relation.to_space_id);
            b.push_bind(&relation.to_version_id);
            b.push_bind(&relation.position);
            b.push_bind(relation.verified);
        });

        query_builder.push(
            ") AS v(id, from_space_id, from_version_id, to_space_id, to_version_id, position, verified)
             WHERE relations.id = v.id",
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
             FROM (VALUES ",
        );

        query_builder.push_values(relations, |mut b, relation| {
            b.push("(");
            b.push_bind(relation.id);
            b.push(", ");
            b.push_bind(relation.from_space_id.unwrap_or(false));
            b.push(", ");
            b.push_bind(relation.from_version_id.unwrap_or(false));
            b.push(", ");
            b.push_bind(relation.to_space_id.unwrap_or(false));
            b.push(", ");
            b.push_bind(relation.to_version_id.unwrap_or(false));
            b.push(", ");
            b.push_bind(relation.position.unwrap_or(false));
            b.push(", ");
            b.push_bind(relation.verified.unwrap_or(false));
            b.push(")");
        });

        query_builder.push(
            ") AS v(id, unset_from_space_id, unset_from_version_id, unset_to_space_id,
                   unset_to_version_id, unset_position, unset_verified)
             WHERE relations.id = v.id",
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
            )",
        )
        .bind(&ids)
        .bind(&space_ids)
        .execute(&mut **tx)
        .await?;

        Ok(())
    }

    pub async fn insert_properties(
        &self,
        properties: &[PropertyItem],
        tx: &mut sqlx::Transaction<'_, Postgres>,
    ) -> Result<(), IndexerError> {
        if properties.is_empty() {
            return Ok(());
        }

        let mut ids = Vec::with_capacity(properties.len());
        let mut types = Vec::with_capacity(properties.len());

        for property in properties {
            ids.push(&property.id);
            types.push(property.data_type.as_ref());
        }

        let query = r#"
            INSERT INTO properties (id, type)
            SELECT id, type::"dataTypes"
            FROM UNNEST($1::uuid[], $2::text[]) AS t(id, type)
            ON CONFLICT (id) DO NOTHING
        "#;

        sqlx::query(query)
            .bind(&ids)
            .bind(&types)
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

        sqlx::query!(
            r#"
            INSERT INTO subspaces (child_space_id, parent_space_id)
            SELECT child_space_id, parent_space_id
            FROM UNNEST($1::uuid[], $2::uuid[])
            AS t(child_space_id, parent_space_id)
            ON CONFLICT (parent_space_id, child_space_id) DO NOTHING
            "#,
            &subspace_ids,
            &parent_space_ids
        )
        .execute(&mut **tx)
        .await?;

        Ok(())
    }

    #[allow(dead_code)]
    pub async fn remove_subspaces(
        &self,
        subspaces: &[SubspaceItem],
        tx: &mut sqlx::Transaction<'_, Postgres>,
    ) -> Result<(), IndexerError> {
        if subspaces.is_empty() {
            return Ok(());
        }

        let subspace_ids: Vec<Uuid> = subspaces.iter().map(|s| s.subspace_id).collect();
        let parent_space_ids: Vec<Uuid> = subspaces.iter().map(|s| s.parent_space_id).collect();

        sqlx::query!(
            r#"
            DELETE FROM subspaces
            WHERE (child_space_id, parent_space_id) IN (
                SELECT child_space_id, parent_space_id
                FROM UNNEST($1::uuid[], $2::uuid[])
                AS t(child_space_id, parent_space_id)
            )
            "#,
            &subspace_ids,
            &parent_space_ids
        )
        .execute(&mut **tx)
        .await?;

        Ok(())
    }

    #[allow(dead_code)]
    pub async fn insert_proposals(
        &self,
        proposals: &[ProposalItem],
        tx: &mut sqlx::Transaction<'_, Postgres>,
    ) -> Result<(), IndexerError> {
        if proposals.is_empty() {
            return Ok(());
        }

        let mut ids = Vec::with_capacity(proposals.len());
        let mut space_ids = Vec::with_capacity(proposals.len());
        let mut proposed_bys = Vec::with_capacity(proposals.len());
        let mut voting_modes = Vec::with_capacity(proposals.len());
        let mut start_times = Vec::with_capacity(proposals.len());
        let mut end_times = Vec::with_capacity(proposals.len());
        let mut quorums = Vec::with_capacity(proposals.len());
        let mut thresholds = Vec::with_capacity(proposals.len());
        let mut executed_ats: Vec<Option<i64>> = Vec::with_capacity(proposals.len());
        let mut created_ats = Vec::with_capacity(proposals.len());
        let mut created_at_blocks = Vec::with_capacity(proposals.len());

        for proposal in proposals {
            ids.push(proposal.id);
            space_ids.push(proposal.space_id);
            proposed_bys.push(proposal.proposed_by);
            voting_modes.push(match proposal.voting_mode {
                VotingMode::Fast => "Fast",
                VotingMode::Slow => "Slow",
            });
            start_times.push(proposal.start_time);
            end_times.push(proposal.end_time);
            quorums.push(proposal.quorum);
            thresholds.push(proposal.threshold);
            executed_ats.push(proposal.executed_at);
            created_ats.push(proposal.created_at.to_string());
            created_at_blocks.push(proposal.created_at_block.to_string());
        }

        let query = r#"
            INSERT INTO proposals (
                id, space_id, proposed_by, voting_mode, start_time, end_time,
                quorum, threshold, executed_at, created_at, created_at_block
            )
            SELECT id, space_id, proposed_by, voting_mode::"votingMode", start_time, end_time,
                   quorum, threshold, executed_at, created_at, created_at_block
            FROM UNNEST(
                $1::uuid[], $2::uuid[], $3::uuid[], $4::text[], $5::bigint[], $6::bigint[],
                $7::bigint[], $8::bigint[], $9::bigint[], $10::text[], $11::text[]
            ) AS t(id, space_id, proposed_by, voting_mode, start_time, end_time,
                   quorum, threshold, executed_at, created_at, created_at_block)
            ON CONFLICT (id) DO UPDATE SET
                executed_at = COALESCE(EXCLUDED.executed_at, proposals.executed_at)
        "#;

        sqlx::query(query)
            .bind(&ids)
            .bind(&space_ids)
            .bind(&proposed_bys)
            .bind(&voting_modes)
            .bind(&start_times)
            .bind(&end_times)
            .bind(&quorums)
            .bind(&thresholds)
            .bind(&executed_ats)
            .bind(&created_ats)
            .bind(&created_at_blocks)
            .execute(&mut **tx)
            .await?;

        Ok(())
    }

    #[allow(dead_code)]
    pub async fn update_proposal(
        &self,
        proposal: &ProposalItem,
        tx: &mut sqlx::Transaction<'_, Postgres>,
    ) -> Result<(), IndexerError> {
        let voting_mode = match proposal.voting_mode {
            VotingMode::Fast => "Fast",
            VotingMode::Slow => "Slow",
        };

        let result = sqlx::query(
            r#"
            UPDATE proposals
            SET space_id = $2,
                proposed_by = $3,
                voting_mode = $4::"votingMode",
                start_time = $5,
                end_time = $6,
                quorum = $7,
                threshold = $8
            WHERE id = $1
            "#,
        )
        .bind(proposal.id)
        .bind(proposal.space_id)
        .bind(proposal.proposed_by)
        .bind(voting_mode)
        .bind(proposal.start_time)
        .bind(proposal.end_time)
        .bind(proposal.quorum)
        .bind(proposal.threshold)
        .execute(&mut **tx)
        .await?;

        if result.rows_affected() == 0 {
            self.insert_proposals(std::slice::from_ref(proposal), tx)
                .await?;
        }

        Ok(())
    }

    #[allow(dead_code)]
    pub async fn delete_proposal_actions(
        &self,
        proposal_id: Uuid,
        tx: &mut sqlx::Transaction<'_, Postgres>,
    ) -> Result<(), IndexerError> {
        sqlx::query(
            r#"
            DELETE FROM proposal_actions
            WHERE proposal_id = $1
            "#,
        )
        .bind(proposal_id)
        .execute(&mut **tx)
        .await?;

        Ok(())
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

        let mut ids = Vec::with_capacity(actions.len());
        let mut proposal_ids = Vec::with_capacity(actions.len());
        let mut action_types = Vec::with_capacity(actions.len());
        let mut target_addresses: Vec<Option<&str>> = Vec::with_capacity(actions.len());
        let mut content_uris: Vec<Option<&str>> = Vec::with_capacity(actions.len());
        let mut metadatas: Vec<Option<&[u8]>> = Vec::with_capacity(actions.len());
        let mut content_ids: Vec<Option<&[u8]>> = Vec::with_capacity(actions.len());
        let mut quorums: Vec<Option<i64>> = Vec::with_capacity(actions.len());
        let mut fast_thresholds: Vec<Option<i64>> = Vec::with_capacity(actions.len());
        let mut slow_thresholds: Vec<Option<i64>> = Vec::with_capacity(actions.len());
        let mut durations: Vec<Option<i64>> = Vec::with_capacity(actions.len());

        for action in actions {
            ids.push(action.id);
            proposal_ids.push(action.proposal_id);

            let mut target_address: Option<&str> = None;
            let mut content_uri: Option<&str> = None;
            let mut metadata: Option<&[u8]> = None;
            let mut content_id: Option<&[u8]> = None;
            let mut quorum: Option<i64> = None;
            let mut fast_threshold: Option<i64> = None;
            let mut slow_threshold: Option<i64> = None;
            let mut duration: Option<i64> = None;

            let action_type = match &action.payload {
                ProposalActionPayload::AddMember {
                    target_address: addr,
                } => {
                    target_address = Some(addr.as_str());
                    "AddMember"
                }
                ProposalActionPayload::RemoveMember {
                    target_address: addr,
                } => {
                    target_address = Some(addr.as_str());
                    "RemoveMember"
                }
                ProposalActionPayload::AddEditor {
                    target_address: addr,
                } => {
                    target_address = Some(addr.as_str());
                    "AddEditor"
                }
                ProposalActionPayload::RemoveEditor {
                    target_address: addr,
                } => {
                    target_address = Some(addr.as_str());
                    "RemoveEditor"
                }
                ProposalActionPayload::UnflagEditor {
                    target_address: addr,
                } => {
                    target_address = Some(addr.as_str());
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
                ProposalActionPayload::UpdateVotingSettings {
                    quorum: q,
                    fast_threshold: ft,
                    slow_threshold: st,
                    duration: d,
                } => {
                    quorum = Some(*q as i64);
                    fast_threshold = Some(*ft as i64);
                    slow_threshold = Some(*st as i64);
                    duration = Some(*d as i64);
                    "UpdateVotingSettings"
                }
                ProposalActionPayload::Unknown => "Unknown",
            };

            action_types.push(action_type);
            target_addresses.push(target_address);
            content_uris.push(content_uri);
            metadatas.push(metadata);
            content_ids.push(content_id);
            quorums.push(quorum);
            fast_thresholds.push(fast_threshold);
            slow_thresholds.push(slow_threshold);
            durations.push(duration);
        }

        let query = r#"
            INSERT INTO proposal_actions (
                id, proposal_id, action_type,
                target_address, content_uri, metadata, content_id,
                quorum, fast_threshold, slow_threshold, duration
            )
            SELECT id, proposal_id, action_type::"proposalActionType",
                   target_address, content_uri, metadata, content_id,
                   quorum, fast_threshold, slow_threshold, duration
            FROM UNNEST(
                $1::uuid[], $2::uuid[], $3::text[],
                $4::text[], $5::text[], $6::bytea[], $7::bytea[],
                $8::bigint[], $9::bigint[], $10::bigint[], $11::bigint[]
            ) AS t(id, proposal_id, action_type,
                   target_address, content_uri, metadata, content_id,
                   quorum, fast_threshold, slow_threshold, duration)
            ON CONFLICT (id) DO NOTHING
        "#;

        sqlx::query(query)
            .bind(&ids)
            .bind(&proposal_ids)
            .bind(&action_types)
            .bind(&target_addresses)
            .bind(&content_uris)
            .bind(&metadatas)
            .bind(&content_ids)
            .bind(&quorums)
            .bind(&fast_thresholds)
            .bind(&slow_thresholds)
            .bind(&durations)
            .execute(&mut **tx)
            .await?;

        Ok(())
    }

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
        let mut voter_ids = Vec::with_capacity(votes.len());
        let mut space_ids = Vec::with_capacity(votes.len());
        let mut vote_options = Vec::with_capacity(votes.len());
        let mut created_ats = Vec::with_capacity(votes.len());
        let mut created_at_blocks = Vec::with_capacity(votes.len());

        for vote in votes {
            proposal_ids.push(vote.proposal_id);
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
                proposal_id, voter_id, space_id, vote, created_at, created_at_block
            )
            SELECT proposal_id, voter_id, space_id, vote::"voteOption", created_at, created_at_block
            FROM UNNEST(
                $1::uuid[], $2::uuid[], $3::uuid[], $4::text[], $5::text[], $6::text[]
            ) AS t(proposal_id, voter_id, space_id, vote, created_at, created_at_block)
            ON CONFLICT (proposal_id, voter_id) DO UPDATE SET
                vote = EXCLUDED.vote::"voteOption",
                created_at = EXCLUDED.created_at,
                created_at_block = EXCLUDED.created_at_block
        "#;

        sqlx::query(query)
            .bind(&proposal_ids)
            .bind(&voter_ids)
            .bind(&space_ids)
            .bind(&vote_options)
            .bind(&created_ats)
            .bind(&created_at_blocks)
            .execute(&mut **tx)
            .await?;

        Ok(())
    }

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
}

#[allow(dead_code)]
fn string_to_data_type(s: &str) -> Option<DataType> {
    match s {
        DATA_TYPE_STRING => Some(DataType::String),
        DATA_TYPE_NUMBER => Some(DataType::Number),
        DATA_TYPE_BOOLEAN => Some(DataType::Boolean),
        DATA_TYPE_TIME => Some(DataType::Time),
        DATA_TYPE_POINT => Some(DataType::Point),
        DATA_TYPE_RELATION => Some(DataType::Relation),
        _ => None,
    }
}
