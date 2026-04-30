-- Partial composite indexes for context-aware diff discovery (RFC 0003).
-- queryContextEntities does:
--   WHERE context_root_id = $1
--     AND context_edge_type_id IS NOT NULL
--     AND valid_from_key <= $key
--     AND (valid_to_key IS NULL OR valid_to_key > $key)
--     AND space_id = $space_id
-- on both value_versions and relation_versions. Without these indexes, every
-- grouped-diff request seq-scans the version tables. Including space_id in
-- the composite key lets PG seek directly to (root, space) rather than
-- post-filter; the partial filter keeps index size bounded since most rows
-- pre-RFC carry NULL context_root_id.
CREATE INDEX IF NOT EXISTS "value_versions_context_root_idx"
    ON "value_versions" ("context_root_id", "space_id", "valid_from_key")
    WHERE "context_root_id" IS NOT NULL;
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS "relation_versions_context_root_idx"
    ON "relation_versions" ("context_root_id", "space_id", "valid_from_key")
    WHERE "context_root_id" IS NOT NULL;
