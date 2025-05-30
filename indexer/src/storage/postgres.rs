use async_trait::async_trait;

use sqlx::{postgres::PgPoolOptions, Postgres, QueryBuilder, Row};

use crate::models::{
    entities::EntityItem,
    properties::{
        DataType, PropertyItem, DATA_TYPE_CHECKBOX, DATA_TYPE_NUMBER, DATA_TYPE_POINT,
        DATA_TYPE_RELATION, DATA_TYPE_TEXT, DATA_TYPE_TIME,
    },
    relations::{SetRelationItem, UnsetRelationItem, UpdateRelationItem},
    values::{ValueChangeType, ValueOp},
};

use super::{StorageBackend, StorageError};

pub struct PostgresStorage {
    pool: sqlx::Pool<Postgres>,
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
        let query = sqlx::query!("SELECT * FROM entities WHERE id = $1", entity_id)
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
        let query = sqlx::query!("SELECT * FROM values WHERE id = $1", triple_id)
            .fetch_one(&self.pool)
            .await?;

        Ok(ValueOp {
            id: query.id,
            property_id: query.property_id,
            entity_id: query.entity_id,
            space_id: query.space_id,
            value: query.value,
            language: query.language,
            unit: query.unit,
            change_type: ValueChangeType::SET,
        })
    }

    pub async fn get_relation(
        &self,
        relation_id: &String,
    ) -> Result<SetRelationItem, StorageError> {
        let query = sqlx::query!("SELECT * FROM relations WHERE id = $1", relation_id)
            .fetch_one(&self.pool)
            .await?;

        Ok(SetRelationItem {
            id: query.id,
            type_id: query.type_id,
            entity_id: query.entity_id,
            space_id: query.space_id,
            from_id: query.from_entity_id,
            from_space_id: None,
            from_version_id: None,
            to_id: query.to_entity_id,
            to_space_id: query.to_space_id,
            to_version_id: query.to_version_id,
            verified: query.verified,
            position: None,
        })
    }

    pub async fn get_property(&self, property_id: &String) -> Result<PropertyItem, StorageError> {
        let row = sqlx::query("SELECT id, type::text as type FROM properties WHERE id = $1")
            .bind(property_id)
            .fetch_one(&self.pool)
            .await?;

        let id: String = row.get("id");
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
}

#[async_trait]
impl StorageBackend for PostgresStorage {
    async fn insert_entities(&self, entities: &Vec<EntityItem>) -> Result<(), StorageError> {
        let ids: Vec<String> = entities.iter().map(|x| x.id.clone()).collect();
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
            SELECT * FROM UNNEST($1::text[], $2::text[], $3::text[], $4::text[], $5::text[])
            ON CONFLICT (id)
            DO UPDATE SET updated_at = EXCLUDED.updated_at, updated_at_block = EXCLUDED.updated_at_block
            "#,
            &ids,
            &created_ats,
            &created_at_blocks,
            &updated_ats,
            &updated_at_blocks
        )
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn insert_values(&self, values: &Vec<ValueOp>) -> Result<(), StorageError> {
        if values.is_empty() {
            return Ok(());
        }

        // Prepare column-wise vectors
        let mut ids = Vec::with_capacity(values.len());
        let mut entity_ids = Vec::with_capacity(values.len());
        let mut property_ids = Vec::with_capacity(values.len());
        let mut space_ids = Vec::with_capacity(values.len());
        let mut value_values = Vec::with_capacity(values.len());
        let mut languages = Vec::with_capacity(values.len());
        let mut units = Vec::with_capacity(values.len());

        for prop in values {
            ids.push(&prop.id);
            entity_ids.push(&prop.entity_id);
            property_ids.push(&prop.property_id);
            space_ids.push(&prop.space_id);
            value_values.push(&prop.value);
            languages.push(&prop.language);
            units.push(&prop.unit);
        }

        let query = r#"
                INSERT INTO values (
                    id, entity_id, property_id, space_id, value, language, unit
                )
                SELECT * FROM UNNEST(
                    $1::text[],
                    $2::text[],
                    $3::text[],
                    $4::text[],
                    $5::text[],
                    $6::text[],
                    $7::text[]
                )
                ON CONFLICT (id) DO UPDATE SET
                    value = EXCLUDED.value,
                    language = EXCLUDED.language,
                    unit = EXCLUDED.unit
            "#;

        sqlx::query(query)
            .bind(&ids)
            .bind(&entity_ids)
            .bind(&property_ids)
            .bind(&space_ids)
            .bind(&value_values)
            .bind(&languages)
            .bind(&units)
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    async fn delete_values(
        &self,
        property_ids: &Vec<String>,
        space_id: &String,
    ) -> Result<(), StorageError> {
        if property_ids.is_empty() {
            return Ok(());
        }

        let ids: Vec<&str> = property_ids.iter().map(|id| id.as_str()).collect();

        sqlx::query(
            "DELETE FROM values
                     WHERE space_id = $1 AND id IN
                     (SELECT * FROM UNNEST($2::text[]))",
        )
        .bind(space_id)
        .bind(&ids)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn insert_relations(&self, relations: &Vec<SetRelationItem>) -> Result<(), StorageError> {
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
                    $1::text[], $2::text[], $3::text[], $4::text[], $5::text[],
                    $6::text[], $7::text[], $8::text[], $9::text[], $10::boolean[]
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
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    async fn update_relations(
        &self,
        relations: &Vec<UpdateRelationItem>,
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
        let result = query_builder.build().execute(&self.pool).await;

        if let Err(error) = result {
            println!("Error writing relations {}", error);
        }

        Ok(())
    }

    async fn unset_relation_fields(
        &self,
        relations: &Vec<UnsetRelationItem>,
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

        query_builder.build().execute(&self.pool).await?;

        Ok(())
    }

    async fn delete_relations(
        &self,
        relation_ids: &Vec<String>,
        space_id: &String,
    ) -> Result<(), StorageError> {
        if relation_ids.is_empty() {
            return Ok(());
        }

        let ids: Vec<&str> = relation_ids.iter().map(|id| id.as_str()).collect();

        sqlx::query(
            "DELETE FROM relations
                     WHERE space_id = $1 AND id IN
                     (SELECT * FROM UNNEST($2::text[]))",
        )
        .bind(space_id)
        .bind(&ids)
        .execute(&self.pool)
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
    async fn insert_properties(&self, properties: &Vec<PropertyItem>) -> Result<(), StorageError> {
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
                FROM UNNEST($1::text[], $2::text[]) AS t(id, type)
                ON CONFLICT (id) DO NOTHING
            "#;

        sqlx::query(query)
            .bind(&ids)
            .bind(&types)
            .execute(&self.pool)
            .await?;

        Ok(())
    }
}

fn string_to_data_type(s: &str) -> Option<DataType> {
    match s {
        DATA_TYPE_TEXT => Some(DataType::Text),
        DATA_TYPE_NUMBER => Some(DataType::Number),
        DATA_TYPE_CHECKBOX => Some(DataType::Checkbox),
        DATA_TYPE_TIME => Some(DataType::Time),
        DATA_TYPE_POINT => Some(DataType::Point),
        DATA_TYPE_RELATION => Some(DataType::Relation),
        _ => None,
    }
}
