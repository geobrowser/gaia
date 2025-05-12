use async_trait::async_trait;

use grc20::pb::ipfs::ValueType;
use sqlx::{postgres::PgPoolOptions, Postgres, QueryBuilder};

use crate::models::{
    entities::EntityItem,
    properties::{PropertyChangeType, PropertyOp},
};

use super::{StorageBackend, StorageError};

pub struct PostgresStorage {
    pool: sqlx::Pool<Postgres>,
}

impl PostgresStorage {
    pub async fn new(database_url: &String) -> Result<Self, StorageError> {
        let database_url_static = database_url.as_str();

        let pool = PgPoolOptions::new()
            .max_connections(20)
            .connect(database_url_static)
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

    pub async fn get_property(&self, triple_id: &String) -> Result<PropertyOp, StorageError> {
        let query = sqlx::query!("SELECT * FROM properties WHERE id = $1", triple_id)
            .fetch_one(&self.pool)
            .await?;

        Ok(PropertyOp {
            id: query.id,
            attribute_id: query.attribute_id,
            entity_id: query.entity_id,
            value_type: ValueType::Text, // @TODO real value type
            space_id: query.space_id,
            text_value: query.text_value,
            number_value: query.number_value,
            boolean_value: query.boolean_value,
            format_option: query.format_option,
            language_option: query.language_option,
            unit_option: query.unit_option,
            change_type: PropertyChangeType::SET,
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

    async fn insert_properties(&self, properties: &Vec<PropertyOp>) -> Result<(), StorageError> {
        if properties.is_empty() {
            return Ok(());
        }

        // Create a query builder for PostgreSQL
        let mut query_builder: QueryBuilder<Postgres> = QueryBuilder::new(
                    "INSERT INTO properties (id, entity_id, attribute_id, space_id, value_type, text_value, boolean_value, number_value, language_option, format_option, unit_option) "
                );

        // Start the VALUES section
        query_builder.push_values(properties, |mut b, property| {
            b.push_bind(format!(
                "{}:{}:{}",
                property.entity_id, property.attribute_id, property.space_id
            ));
            b.push_bind(&property.entity_id);
            b.push_bind(&property.attribute_id);
            b.push_bind(&property.space_id);
            b.push_bind(property.value_type as i32); // Assuming PbValueType can be cast to i32
            b.push_bind(&property.text_value);
            b.push_bind(property.boolean_value);
            b.push_bind(&property.number_value);
            b.push_bind(&property.language_option);
            b.push_bind(&property.format_option);
            b.push_bind(&property.unit_option);
        });

        query_builder.push(
            " ON CONFLICT (id) DO UPDATE SET
                        text_value = EXCLUDED.text_value,
                        boolean_value = EXCLUDED.boolean_value,
                        number_value = EXCLUDED.number_value,
                        language_option = EXCLUDED.language_option,
                        format_option = EXCLUDED.format_option,
                        unit_option = EXCLUDED.unit_option",
        );

        // Execute the query
        let result = query_builder.build().execute(&self.pool).await;

        if let Err(error) = result {
            println!("Error writing properties {}", error);
        }

        Ok(())
    }

    async fn delete_properties(&self, property_ids: &Vec<String>) -> Result<(), StorageError> {
        if property_ids.is_empty() {
            return Ok(());
        }

        let ids: Vec<&str> = property_ids.iter().map(|id| id.as_str()).collect();

        let result = sqlx::query(
            "DELETE FROM properties
                     WHERE id IN
                     (SELECT * FROM UNNEST($1::text[]))",
        )
        .bind(&ids)
        .execute(&self.pool)
        .await;

        if let Err(error) = result {
            println!("Error deleting properties {}", error);
        }

        Ok(())
    }
}
