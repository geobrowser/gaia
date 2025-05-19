use async_trait::async_trait;

use sqlx::{postgres::PgPoolOptions, Postgres, QueryBuilder};

use crate::models::{
    entities::EntityItem,
    properties::{ValueChangeType, ValueOp},
    relations::RelationItem,
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
            language_option: query.language_option,
            change_type: ValueChangeType::SET,
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

    async fn insert_values(&self, properties: &Vec<ValueOp>) -> Result<(), StorageError> {
        if properties.is_empty() {
            return Ok(());
        }

        // Create a query builder for PostgreSQL
        let mut query_builder: QueryBuilder<Postgres> = QueryBuilder::new(
            "INSERT INTO values (id, entity_id, property_id, space_id, value, language_option) ",
        );

        // Start the VALUES section
        query_builder.push_values(properties, |mut b, property| {
            b.push_bind(format!(
                "{}:{}:{}",
                property.entity_id, property.property_id, property.space_id
            ));
            b.push_bind(&property.entity_id);
            b.push_bind(&property.property_id);
            b.push_bind(&property.space_id);
            b.push_bind(&property.value);
            b.push_bind(&property.language_option);
        });

        query_builder.push(
            " ON CONFLICT (id) DO UPDATE SET
                        value = EXCLUDED.value,
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

    async fn delete_values(&self, property_ids: &Vec<String>) -> Result<(), StorageError> {
        if property_ids.is_empty() {
            return Ok(());
        }

        let ids: Vec<&str> = property_ids.iter().map(|id| id.as_str()).collect();

        let result = sqlx::query(
            "DELETE FROM values
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

    async fn insert_relations(&self, relations: &Vec<RelationItem>) -> Result<(), StorageError> {
        if relations.is_empty() {
            return Ok(());
        }

        // Create a query builder for PostgreSQL
        let mut query_builder: QueryBuilder<Postgres> = QueryBuilder::new(
            "INSERT INTO relations (id, space_id, from_entity_id, to_entity_id, to_space_id, type_id, index) ",
        );

        // Start the VALUES section
        query_builder.push_values(relations, |mut b, relation| {
            b.push_bind(&relation.id);
            b.push_bind(&relation.space_id);
            b.push_bind(&relation.from_id);
            b.push_bind(&relation.to_id);
            b.push_bind(&relation.to_space_id);
            b.push_bind(&relation.type_id);
            b.push_bind(&relation.position);
        });

        query_builder.push(
            " ON CONFLICT (id) DO UPDATE SET
                        to_space_id = EXCLUDED.to_space_id,
                        index = EXCLUDED.index",
        );

        // Execute the query
        let result = query_builder.build().execute(&self.pool).await;

        if let Err(error) = result {
            println!("Error writing relations {}", error);
        }

        Ok(())
    }

    async fn delete_relations(&self, relation_ids: &Vec<String>) -> Result<(), StorageError> {
        if relation_ids.is_empty() {
            return Ok(());
        }

        let ids: Vec<&str> = relation_ids.iter().map(|id| id.as_str()).collect();

        let result = sqlx::query(
            "DELETE FROM relations
                     WHERE id IN
                     (SELECT * FROM UNNEST($1::text[]))",
        )
        .bind(&ids)
        .execute(&self.pool)
        .await;

        if let Err(error) = result {
            println!("Error deleting relations {}", error);
        }

        Ok(())
    }
}
