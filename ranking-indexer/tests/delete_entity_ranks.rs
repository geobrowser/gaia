//! The ranks.deleted_rankings flow (migration 0125) next to kg-indexer's `DeleteEntity` handling.
//!
//! kg-indexer now removes a deleted Rank's outgoing `TYPES -> Rank` relation from `relations`,
//! which is what `find_unregistered_ranks` reads. ranking-indexer's `delete_rankings` tombstones
//! the Rank. Whichever of the two runs first, the deleted Rank must not be found as unregistered
//! (which would make the recovery sweep register it again), and with the kill switch off the
//! tombstone must still be the guard.
//!
//! Needs Postgres with every api migration applied and DATABASE_URL set.
//! Run with: cargo test -p ranking-indexer --test delete_entity_ranks -- --ignored

use kg_indexer::models::entities::EntityItem;
use kg_indexer::models::relations::SetRelationItem;
use kg_indexer::storage::{DeletingEdit, Storage as KgStorage};
use ranking_indexer::storage::Storage as RankStorage;
use sdk::core::ids;
use uuid::Uuid;

struct Rig {
    kg: KgStorage,
    ranks: RankStorage,
    pool: sqlx::PgPool,
    space: Uuid,
    rank: Uuid,
}

async fn rig(apply: bool) -> Rig {
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
    let kg = KgStorage::new(&url)
        .await
        .expect("kg storage")
        .with_apply_delete_entity(apply);
    let ranks = RankStorage::new(&url).await.expect("rank storage");
    let pool = sqlx::PgPool::connect(&url).await.unwrap();
    let rig = Rig {
        kg,
        ranks,
        pool,
        space: Uuid::new_v4(),
        rank: Uuid::new_v4(),
    };
    // The Rank is typed as a Rank in the space: the relation the recovery sweep looks for.
    let rank_type = Uuid::parse_str(ids::RANK_TYPE_ID).unwrap();
    let types = Uuid::parse_str(ids::TYPE_RELATION_TYPE_ID).unwrap();
    let mut tx = rig.kg.begin_with_timeout().await.unwrap();
    sqlx::query("SET CONSTRAINTS ALL DEFERRED")
        .execute(&mut *tx)
        .await
        .unwrap();
    let ents: Vec<EntityItem> = [rig.rank, rank_type]
        .into_iter()
        .map(|id| EntityItem {
            id,
            created_at: "1".into(),
            created_at_block: "1".into(),
            updated_at: "1".into(),
            updated_at_block: "1".into(),
        })
        .collect();
    rig.kg.insert_entities(&ents, &mut tx).await.unwrap();
    rig.kg
        .insert_relations(
            &[SetRelationItem {
                id: Uuid::new_v4(),
                entity_id: Uuid::new_v4(),
                type_id: types,
                from_id: rig.rank,
                from_space_id: None,
                from_version_id: None,
                to_id: rank_type,
                to_space_id: None,
                to_version_id: None,
                position: None,
                space_id: rig.space,
                verified: None,
                is_system: false,
                context_root_id: None,
                context_edge_type_id: None,
                context_last_to_entity_id: None,
            }],
            &mut tx,
        )
        .await
        .unwrap();
    tx.commit().await.unwrap();
    rig
}

impl Rig {
    async fn kg_delete(&self) {
        let mut tx = self.kg.begin_with_timeout().await.unwrap();
        let edit = DeletingEdit {
            edit_id: Uuid::new_v4(),
            block_number: 1,
            created_at: 1,
        };
        self.kg
            .delete_entities(&[self.rank], self.space, edit, &mut tx)
            .await
            .unwrap();
        tx.commit().await.unwrap();
    }

    async fn unregistered(&self) -> bool {
        self.ranks
            .find_unregistered_ranks()
            .await
            .unwrap()
            .contains(&(self.rank, self.space))
    }

    async fn tombstoned(&self) -> bool {
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM ranks.deleted_rankings WHERE id = $1 AND space_id = $2",
        )
        .bind(self.rank)
        .bind(self.space)
        .fetch_one(&self.pool)
        .await
        .unwrap()
            == 1
    }

    async fn rank_deleted_by_ranking_indexer(&self) {
        self.ranks
            .delete_rankings(&[self.rank], &[], self.space)
            .await
            .unwrap();
    }
}

#[tokio::test]
#[ignore]
async fn unregistered_rank_is_found_until_its_delete() {
    let r = rig(true).await;
    assert!(r.unregistered().await, "typed as a Rank, not registered");
}

#[tokio::test]
#[ignore]
async fn kg_deletes_first_then_ranking_indexer_tombstones_and_nothing_resurrects() {
    let r = rig(true).await;
    r.kg_delete().await;
    assert!(!r.unregistered().await, "the TYPES relation is gone");
    r.rank_deleted_by_ranking_indexer().await;
    // The relation was already gone, so no tombstone is needed and none is written.
    assert!(!r.tombstoned().await);
    assert!(!r.unregistered().await);
}

#[tokio::test]
#[ignore]
async fn ranking_indexer_tombstones_first_then_kg_delete_keeps_it_deleted() {
    let r = rig(true).await;
    r.rank_deleted_by_ranking_indexer().await;
    assert!(r.tombstoned().await, "the TYPES relation was still there");
    assert!(!r.unregistered().await);
    r.kg_delete().await;
    assert!(r.tombstoned().await);
    assert!(!r.unregistered().await);
}

#[tokio::test]
#[ignore]
async fn with_the_kill_switch_off_the_tombstone_is_still_the_guard() {
    let r = rig(false).await;
    r.kg_delete().await;
    assert!(
        r.unregistered().await,
        "flag off: nothing deleted, relation stays"
    );
    r.rank_deleted_by_ranking_indexer().await;
    assert!(r.tombstoned().await);
    assert!(
        !r.unregistered().await,
        "the tombstone, not the delete, protects it"
    );
}
