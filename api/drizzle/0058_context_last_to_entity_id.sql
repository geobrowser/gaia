-- RFC 0006: persist the GRC-20 context's leaf entity
-- (`edges.last().to_entity_id`) alongside the existing context_root_id +
-- context_edge_type_id columns. Without this, context-aware diff discovery
-- infers the "changed child" from value_versions.entity_id or
-- relation_versions.from_entity_id — correct by structural coincidence in
-- well-formed cases, wrong when an edit authored under a context creates a
-- reified relation between foreign entities (the canonical example in the
-- RFC: a LINK between Source_A and Source_B authored inside a TextBlock).
--
-- Nullable: forward-only. Old rows keep NULL; the API query falls back to
-- the old inference path when the column is NULL. New rows written by the
-- indexer carry the column from this point forward.
ALTER TABLE "value_versions" ADD COLUMN "context_last_to_entity_id" uuid;--> statement-breakpoint
ALTER TABLE "relation_versions" ADD COLUMN "context_last_to_entity_id" uuid;
