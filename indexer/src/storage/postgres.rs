use async_trait::async_trait;

use sqlx::{postgres::PgPoolOptions, Postgres, QueryBuilder, Row};
use tracing::error;
use uuid::Uuid;

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
    values::{ValueChangeType, ValueOp},
};

use super::{StorageBackend, StorageError};

#[derive(sqlx::FromRow)]
struct EntityRow {
    id: Uuid,
    created_at: String,
    created_at_block: String,
    updated_at: String,
    updated_at_block: String,
}

#[derive(sqlx::FromRow)]
struct RelationRow {
    id: Uuid,
    type_id: Uuid,
    entity_id: Uuid,
    space_id: Uuid,
    from_entity_id: Uuid,
    from_space_id: Option<Uuid>,
    from_version_id: Option<Uuid>,
    to_entity_id: Uuid,
    to_space_id: Option<Uuid>,
    to_version_id: Option<Uuid>,
    verified: Option<bool>,
    position: Option<String>,
}

pub struct PostgresStorage {
    pub pool: sqlx::Pool<Postgres>,
}

impl PostgresStorage {
    pub async fn new(database_url: &String) -> Result<Self, StorageError> {
        let pool = PgPoolOptions::new()
            .max_connections(20)
            .connect(database_url.as_str())
            .await?;

        return Ok(PostgresStorage { pool });
    }

    pub async fn get_entity(&self, entity_id: &String) -> Result<EntityItem, StorageError> {
        let entity_uuid = Uuid::parse_str(entity_id)
            .map_err(|e| sqlx::Error::Decode(format!("Invalid UUID format: {}", e).into()))?;

        let query = sqlx::query_as!(
            EntityRow,
            "SELECT id, created_at, created_at_block, updated_at, updated_at_block FROM entities WHERE id = $1",
            entity_uuid
        )
        .fetch_one(&self.pool)
        .await?;

        Ok(EntityItem {
            id: query.id,
            created_at: query.created_at,
            created_at_block: query.created_at_block,
            updated_at: query.updated_at,
            updated_at_block: query.updated_at_block,
        })
    }

    pub async fn get_value(&self, triple_id: &String) -> Result<ValueOp, StorageError> {
        // Use the generic query instead of query_as to avoid type conversion issues
        let row = sqlx::query(
            r#"SELECT
                id, property_id, entity_id, space_id,
                language, unit, string,
                number::float8 as number, boolean, time, point
                FROM values WHERE id = $1"#,
        )
        .bind(triple_id)
        .fetch_one(&self.pool)
        .await?;

        let id = Uuid::parse_str(row.try_get::<&str, _>("id")?).map_err(|e| {
            sqlx::Error::Decode(format!("Invalid UUID format for id: {}", e).into())
        })?;

        let property_id: Uuid = row.try_get("property_id")?;
        let entity_id: Uuid = row.try_get("entity_id")?;

        let space_id: Uuid = row.try_get("space_id")?;

        let language: Option<String> = row.try_get("language")?;
        let unit: Option<String> = row.try_get("unit")?;
        let text: Option<String> = row.try_get("string")?;

        let number: Option<f64> = row.try_get("number")?;

        let boolean: Option<bool> = row.try_get("boolean")?;
        let time: Option<String> = row.try_get("time")?;
        let point: Option<String> = row.try_get("point")?;

        Ok(ValueOp {
            id,
            property_id,
            entity_id,
            space_id,

            language,
            unit,
            string: text,
            number,
            boolean,
            time,
            point,
            change_type: ValueChangeType::SET,
        })
    }

    pub async fn get_relation(
        &self,
        relation_id: &String,
    ) -> Result<SetRelationItem, StorageError> {
        let relation_uuid = Uuid::parse_str(relation_id)
            .map_err(|e| sqlx::Error::Decode(format!("Invalid UUID format: {}", e).into()))?;

        let query = sqlx::query_as!(
            RelationRow,
            "SELECT id, type_id, entity_id, space_id, from_entity_id, from_space_id, from_version_id, to_entity_id, to_space_id, to_version_id, verified, position FROM relations WHERE id = $1",
            relation_uuid
        )
        .fetch_one(&self.pool)
        .await?;

        Ok(SetRelationItem {
            id: query.id,
            type_id: query.type_id,
            entity_id: query.entity_id,
            space_id: query.space_id,
            from_id: query.from_entity_id,
            from_space_id: query.from_space_id.map(|id| id.to_string()),
            from_version_id: query.from_version_id.map(|id| id.to_string()),
            to_id: query.to_entity_id,
            to_space_id: query.to_space_id.map(|id| id.to_string()),
            to_version_id: query.to_version_id.map(|id| id.to_string()),
            verified: query.verified,
            position: query.position,
        })
    }

    pub async fn get_property(&self, property_id: &String) -> Result<PropertyItem, StorageError> {
        let property_uuid = Uuid::parse_str(property_id)
            .map_err(|e| sqlx::Error::Decode(format!("Invalid UUID format: {}", e).into()))?;

        let row = sqlx::query("SELECT id, type::text as type FROM properties WHERE id = $1")
            .bind(property_uuid)
            .fetch_one(&self.pool)
            .await?;

        let id: Uuid = row.get("id");
        let type_value: String = row.get("type");

        let property_type = string_to_data_type(&type_value).ok_or_else(|| {
            sqlx::Error::Decode(
                format!("Invalid enum value '{}' for dataTypes enum", type_value).into(),
            )
        })?;

        Ok(PropertyItem {
            id,
            data_type: property_type,
        })
    }

    pub async fn get_all_properties(&self) -> Result<Vec<PropertyItem>, StorageError> {
        let rows = sqlx::query("SELECT id, type::text as type FROM properties")
            .fetch_all(&self.pool)
            .await?;

        let mut properties = Vec::new();
        for row in rows {
            let id: Uuid = row.get("id");
            let type_value: String = row.get("type");

            let property_type = string_to_data_type(&type_value).ok_or_else(|| {
                sqlx::Error::Decode(
                    format!("Invalid enum value '{}' for dataTypes enum", type_value).into(),
                )
            })?;

            properties.push(PropertyItem {
                id,
                data_type: property_type,
            });
        }

        Ok(properties)
    }

    pub async fn get_member(
        &self,
        address: &str,
        space_id: &Uuid,
    ) -> Result<MemberItem, StorageError> {
        let query = sqlx::query!(
            "SELECT address, space_id FROM members WHERE address = $1 AND space_id = $2",
            address,
            space_id
        )
        .fetch_one(&self.pool)
        .await?;

        Ok(MemberItem {
            address: query.address,
            space_id: query.space_id,
        })
    }

    pub async fn get_editor(
        &self,
        address: &str,
        space_id: &Uuid,
    ) -> Result<EditorItem, StorageError> {
        let query = sqlx::query!(
            "SELECT address, space_id FROM editors WHERE address = $1 AND space_id = $2",
            address,
            space_id
        )
        .fetch_one(&self.pool)
        .await?;

        Ok(EditorItem {
            address: query.address,
            space_id: query.space_id,
        })
    }

    pub async fn load_cursor(&self, id: &str) -> Result<Option<String>, StorageError> {
        let result = sqlx::query!("SELECT cursor FROM meta WHERE id = $1", id)
            .fetch_optional(&self.pool)
            .await?;

        Ok(result.map(|row| row.cursor))
    }

    pub async fn persist_cursor(
        &self,
        id: &str,
        cursor: &str,
        block: &u64,
    ) -> Result<(), StorageError> {
        sqlx::query!(
            "INSERT INTO meta (id, cursor, block_number) VALUES ($1, $2, $3) ON CONFLICT (id) DO UPDATE SET cursor = $2, block_number = $3",
            id,
            cursor,
            block.to_string()
        )
        .execute(&self.pool)
        .await?;

        Ok(())
    }
}

#[async_trait]
impl StorageBackend for PostgresStorage {
    fn get_pool(&self) -> &sqlx::Pool<Postgres> {
        &self.pool
    }

    async fn insert_entities(
        &self,
        entities: &Vec<EntityItem>,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    ) -> Result<(), StorageError> {
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
        ).execute(&mut **tx).await?;

        Ok(())
    }

    async fn insert_values(
        &self,
        values: &Vec<ValueOp>,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    ) -> Result<(), StorageError> {
        if values.is_empty() {
            return Ok(());
        }

        // Prepare column-wise vectors
        let mut ids = Vec::with_capacity(values.len());
        let mut entity_ids = Vec::with_capacity(values.len());
        let mut property_ids = Vec::with_capacity(values.len());
        let mut space_ids = Vec::with_capacity(values.len());
        let mut languages = Vec::with_capacity(values.len());
        let mut units = Vec::with_capacity(values.len());
        // Type-specific columns
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

            // Add type-specific values
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

    async fn delete_values(
        &self,
        value_ids: &Vec<Uuid>,
        space_id: &Uuid,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    ) -> Result<(), StorageError> {
        if value_ids.is_empty() {
            return Ok(());
        }

        let ids: Vec<String> = value_ids.iter().map(|id| id.to_string()).collect();

        sqlx::query(
            "DELETE FROM values
                     WHERE space_id = $1 AND id IN
                     (SELECT * FROM UNNEST($2::text[]))",
        )
        .bind(space_id)
        .bind(&ids)
        .execute(&mut **tx)
        .await?;

        Ok(())
    }

    async fn insert_relations(
        &self,
        relations: &Vec<SetRelationItem>,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    ) -> Result<(), StorageError> {
        if relations.is_empty() {
            return Ok(());
        }

        // Build column vectors
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

    async fn update_relations(
        &self,
        relations: &Vec<UpdateRelationItem>,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    ) -> Result<(), StorageError> {
        if relations.is_empty() {
            return Ok(());
        }

        // @TODO:
        // This is tricky since we only want to update if the values are actually set,
        // not if they're None

        // Create a query builder for PostgreSQL
        let mut query_builder: QueryBuilder<Postgres> = QueryBuilder::new(
            "UPDATE relations (id, from_space_id, to_space_id, position, verified) ",
        );

        // Start the VALUES section
        query_builder.push_values(relations, |mut b, relation| {
            b.push_bind(&relation.id);
            b.push_bind(&relation.from_space_id);
            b.push_bind(&relation.to_space_id);
            b.push_bind(&relation.position);
            b.push_bind(&relation.verified);
        });

        // Execute the query
        let result = query_builder.build().execute(&mut **tx).await;

        if let Err(error) = result {
            error!("Error writing relations: {}", error);
        }

        Ok(())
    }

    async fn unset_relation_fields(
        &self,
        relations: &Vec<UnsetRelationItem>,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    ) -> Result<(), StorageError> {
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
              FROM (VALUES "
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

    async fn delete_relations(
        &self,
        relation_ids: &Vec<Uuid>,
        space_id: &Uuid,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    ) -> Result<(), StorageError> {
        if relation_ids.is_empty() {
            return Ok(());
        }

        sqlx::query(
            "DELETE FROM relations
                     WHERE space_id = $1 AND id IN
                     (SELECT * FROM UNNEST($2::uuid[]))",
        )
        .bind(space_id)
        .bind(relation_ids)
        .execute(&mut **tx)
        .await?;

        Ok(())
    }

    /// Properties are a special, knowledge-graph wide concept. A property
    /// is a semantic representation of values. e.g., a value might be
    /// "Byron", but without any further context we don't know what "Byron"
    /// represents. Properties are entities which provide semantic meaning,
    /// so there might be a Property called "Name". This Property has a
    /// Data Type of "Text". By associating the value "Byron" with the Property
    /// "Name", we provide semantic meaning to the pair.
    ///
    /// The knowledge graph engine validates that all values associated with
    /// a property correctly conform to the property's Data Type. Additionally,
    /// changing the Property's Data Type is not allowed.
    async fn insert_properties(
        &self,
        properties: &Vec<PropertyItem>,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    ) -> Result<(), StorageError> {
        if properties.is_empty() {
            return Ok(());
        }

        // Prepare column-wise vectors
        let mut ids = Vec::with_capacity(properties.len());
        let mut types = Vec::with_capacity(properties.len());

        for property in properties {
            ids.push(&property.id);
            types.push(property.data_type.as_ref());
        }

        // We don't allow changing an already-created property's value type.
        // Rather than filtering already-created properties ahead of time we
        // let the database engine handle it.
        let query = r#"
                INSERT INTO properties (
                    id, type
                )
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

    async fn insert_spaces(
        &self,
        spaces: &Vec<SpaceItem>,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    ) -> Result<(), StorageError> {
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

    async fn insert_members(
        &self,
        members: &Vec<MemberItem>,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    ) -> Result<(), StorageError> {
        if members.is_empty() {
            return Ok(());
        }

        let mut addresses: Vec<String> = Vec::new();
        let mut space_ids: Vec<Uuid> = Vec::new();

        for member in members {
            addresses.push(member.address.clone());
            space_ids.push(member.space_id);
        }

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

    async fn remove_members(
        &self,
        members: &Vec<MemberItem>,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    ) -> Result<(), StorageError> {
        if members.is_empty() {
            return Ok(());
        }

        let mut addresses: Vec<String> = Vec::new();
        let mut space_ids: Vec<Uuid> = Vec::new();

        for member in members {
            addresses.push(member.address.clone());
            space_ids.push(member.space_id);
        }

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

    async fn insert_editors(
        &self,
        editors: &Vec<EditorItem>,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    ) -> Result<(), StorageError> {
        if editors.is_empty() {
            return Ok(());
        }

        let mut addresses: Vec<String> = Vec::new();
        let mut space_ids: Vec<Uuid> = Vec::new();

        for editor in editors {
            addresses.push(editor.address.clone());
            space_ids.push(editor.space_id);
        }

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

    async fn remove_editors(
        &self,
        editors: &Vec<EditorItem>,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    ) -> Result<(), StorageError> {
        if editors.is_empty() {
            return Ok(());
        }

        let mut addresses: Vec<String> = Vec::new();
        let mut space_ids: Vec<Uuid> = Vec::new();

        for editor in editors {
            addresses.push(editor.address.clone());
            space_ids.push(editor.space_id);
        }

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

    async fn insert_subspaces(
        &self,
        subspaces: &Vec<SubspaceItem>,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    ) -> Result<(), StorageError> {
        if subspaces.is_empty() {
            return Ok(());
        }

        let mut subspace_ids: Vec<Uuid> = Vec::new();
        let mut parent_space_ids: Vec<Uuid> = Vec::new();

        for subspace in subspaces {
            subspace_ids.push(subspace.subspace_id);
            parent_space_ids.push(subspace.parent_space_id);
        }

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

    async fn remove_subspaces(
        &self,
        subspaces: &Vec<SubspaceItem>,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    ) -> Result<(), StorageError> {
        if subspaces.is_empty() {
            return Ok(());
        }

        let mut subspace_ids: Vec<Uuid> = Vec::new();
        let mut parent_space_ids: Vec<Uuid> = Vec::new();

        for subspace in subspaces {
            subspace_ids.push(subspace.subspace_id);
            parent_space_ids.push(subspace.parent_space_id);
        }

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
