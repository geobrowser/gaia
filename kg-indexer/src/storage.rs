use sqlx::{postgres::PgPoolOptions, Postgres, QueryBuilder};
use uuid::Uuid;

use crate::error::IndexerError;
use crate::models::{
    entities::EntityItem,
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
            b.push_bind(&relation.id);
            b.push_bind(&relation.from_space_id);
            b.push_bind(&relation.from_version_id);
            b.push_bind(&relation.to_space_id);
            b.push_bind(&relation.to_version_id);
            b.push_bind(&relation.position);
            b.push_bind(&relation.verified);
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
            b.push_bind(&relation.id);
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
        let mut dao_addresses: Vec<String> = Vec::new();
        let mut space_addresses: Vec<String> = Vec::new();
        let mut main_voting_addresses: Vec<Option<String>> = Vec::new();
        let mut membership_addresses: Vec<Option<String>> = Vec::new();
        let mut personal_addresses: Vec<Option<String>> = Vec::new();

        for space in spaces {
            ids.push(space.id);
            types.push(match space.space_type {
                SpaceType::Personal => "Personal".to_string(),
                SpaceType::Public => "Public".to_string(),
            });
            dao_addresses.push(space.dao_address.clone());
            space_addresses.push(space.space_address.clone());
            main_voting_addresses.push(space.voting_address.clone());
            membership_addresses.push(space.membership_address.clone());
            personal_addresses.push(space.personal_address.clone());
        }

        sqlx::query!(
            r#"
            INSERT INTO spaces (id, type, dao_address, space_address, main_voting_address, membership_address, personal_address)
            SELECT id, type::"spaceTypes", dao_address, space_address, main_voting_address, membership_address, personal_address
            FROM UNNEST($1::uuid[], $2::text[], $3::text[], $4::text[], $5::text[], $6::text[], $7::text[])
            AS t(id, type, dao_address, space_address, main_voting_address, membership_address, personal_address)
            ON CONFLICT (id) DO NOTHING
            "#,
            &ids,
            &types,
            &dao_addresses,
            &space_addresses,
            &main_voting_addresses as &[Option<String>],
            &membership_addresses as &[Option<String>],
            &personal_addresses as &[Option<String>]
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

        let addresses: Vec<String> = members.iter().map(|m| m.address.clone()).collect();
        let space_ids: Vec<Uuid> = members.iter().map(|m| m.space_id).collect();

        sqlx::query!(
            r#"
            INSERT INTO members (address, space_id)
            SELECT address, space_id
            FROM UNNEST($1::text[], $2::uuid[])
            AS t(address, space_id)
            ON CONFLICT (address, space_id) DO NOTHING
            "#,
            &addresses,
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

        let addresses: Vec<String> = members.iter().map(|m| m.address.clone()).collect();
        let space_ids: Vec<Uuid> = members.iter().map(|m| m.space_id).collect();

        sqlx::query!(
            r#"
            DELETE FROM members
            WHERE (address, space_id) IN (
                SELECT address, space_id
                FROM UNNEST($1::text[], $2::uuid[])
                AS t(address, space_id)
            )
            "#,
            &addresses,
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

        let addresses: Vec<String> = editors.iter().map(|e| e.address.clone()).collect();
        let space_ids: Vec<Uuid> = editors.iter().map(|e| e.space_id).collect();

        sqlx::query!(
            r#"
            INSERT INTO editors (address, space_id)
            SELECT address, space_id
            FROM UNNEST($1::text[], $2::uuid[])
            AS t(address, space_id)
            ON CONFLICT (address, space_id) DO NOTHING
            "#,
            &addresses,
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

        let addresses: Vec<String> = editors.iter().map(|e| e.address.clone()).collect();
        let space_ids: Vec<Uuid> = editors.iter().map(|e| e.space_id).collect();

        sqlx::query!(
            r#"
            DELETE FROM editors
            WHERE (address, space_id) IN (
                SELECT address, space_id
                FROM UNNEST($1::text[], $2::uuid[])
                AS t(address, space_id)
            )
            "#,
            &addresses,
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
}

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
