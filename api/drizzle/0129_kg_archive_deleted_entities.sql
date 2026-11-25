-- Archive for rows kg-indexer removes when it applies a DeleteEntity op (KG_INDEXER_APPLY_DELETE_ENTITY).
--
-- kg-indexer copies every `values` / `relations` row it deletes, and every open `value_versions` /
-- `relation_versions` row it closes, into kg_archive.deleted_entity_rows in the same transaction as the
-- delete, so a delete is reversible: kg_archive.restore_deleted_entities(<edit id>) puts them back.
--
-- It lives in its own schema, not `public`, so PostGraphile does not publish it as GraphQL fields (the
-- same reason `ranks` is a schema of its own). Each row is kept as `row_data jsonb` (to_jsonb of the
-- source row), so a column added to `values` or `relations` later cannot make the archiving INSERT fail
-- and halt the indexer; restore reads it back through jsonb_populate_record, where a column missing
-- from the archive is NULL.
--
-- NO FILL, NO PRUNE: this migration creates empty objects. Nothing deletes archive rows; prune by hand
-- once a delete is known to be right, e.g. DELETE FROM kg_archive.deleted_entity_rows WHERE deleted_at < now() - interval '90 days'.
CREATE SCHEMA IF NOT EXISTS "kg_archive";
--> statement-breakpoint
CREATE TABLE IF NOT EXISTS "kg_archive"."deleted_entity_rows" (
	"archive_id" bigserial PRIMARY KEY,
	-- which table the row came from: 'values', 'relations', 'value_versions' or 'relation_versions'
	"source" text NOT NULL,
	-- the deleting edit: its id, block, and created_at (as carried on the edit's metadata)
	"edit_id" uuid NOT NULL,
	"block_number" bigint NOT NULL,
	"edit_created_at" bigint NOT NULL,
	"space_id" uuid NOT NULL,
	-- the deleted entity: values.entity_id, relations.from_entity_id, *_versions.entity_id / from_entity_id
	"entity_id" uuid NOT NULL,
	-- version rows only: the version_key they were closed at (valid_to_key was NULL before)
	"closed_to_key" bigint,
	"deleted_at" timestamptz NOT NULL DEFAULT now(),
	"restored_at" timestamptz,
	"row_data" jsonb NOT NULL,
	CONSTRAINT "deleted_entity_rows_source_check"
		CHECK ("source" IN ('values', 'relations', 'value_versions', 'relation_versions'))
);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS "deleted_entity_rows_edit_idx" ON "kg_archive"."deleted_entity_rows" ("edit_id");
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS "deleted_entity_rows_entity_idx" ON "kg_archive"."deleted_entity_rows" ("entity_id", "space_id");
--> statement-breakpoint
-- Put back what the edit's DeleteEntity ops removed. One row per source: how many archived rows were
-- restored and how many were skipped. A row is skipped, and stays unrestored, when something newer has
-- taken its place (a live value/relation with the same id, or a version closed again since); restore
-- never overwrites newer data. Re-running is safe: restored rows are marked and not applied twice.
-- Does not touch the `entities` row (kg-indexer never deletes it).
CREATE OR REPLACE FUNCTION "kg_archive"."restore_deleted_entities"(p_edit_id uuid)
RETURNS TABLE ("source" text, "restored" bigint, "skipped" bigint)
LANGUAGE plpgsql
AS $fn$
DECLARE
	total bigint;
	done bigint;
BEGIN
	-- values
	SELECT count(*) INTO total FROM kg_archive.deleted_entity_rows a
		WHERE a.edit_id = p_edit_id AND a.source = 'values' AND a.restored_at IS NULL;
	WITH ins AS (
		INSERT INTO public."values"
		SELECT r.* FROM kg_archive.deleted_entity_rows a,
			LATERAL jsonb_populate_record(NULL::public."values", a.row_data) r
		WHERE a.edit_id = p_edit_id AND a.source = 'values' AND a.restored_at IS NULL
		ON CONFLICT (id) DO NOTHING
		RETURNING id
	), marked AS (
		UPDATE kg_archive.deleted_entity_rows a SET restored_at = now()
		WHERE a.edit_id = p_edit_id AND a.source = 'values' AND a.restored_at IS NULL
			AND a.row_data->>'id' IN (SELECT id FROM ins)
		RETURNING 1
	) SELECT count(*) INTO done FROM marked;
	source := 'values'; restored := done; skipped := total - done; RETURN NEXT;

	-- relations
	SELECT count(*) INTO total FROM kg_archive.deleted_entity_rows a
		WHERE a.edit_id = p_edit_id AND a.source = 'relations' AND a.restored_at IS NULL;
	WITH ins AS (
		INSERT INTO public.relations
		SELECT r.* FROM kg_archive.deleted_entity_rows a,
			LATERAL jsonb_populate_record(NULL::public.relations, a.row_data) r
		WHERE a.edit_id = p_edit_id AND a.source = 'relations' AND a.restored_at IS NULL
		ON CONFLICT (id) DO NOTHING
		RETURNING id
	), marked AS (
		UPDATE kg_archive.deleted_entity_rows a SET restored_at = now()
		WHERE a.edit_id = p_edit_id AND a.source = 'relations' AND a.restored_at IS NULL
			AND a.row_data->>'id' IN (SELECT id::text FROM ins)
		RETURNING 1
	) SELECT count(*) INTO done FROM marked;
	source := 'relations'; restored := done; skipped := total - done; RETURN NEXT;

	-- value_versions: reopen the version this edit closed
	SELECT count(*) INTO total FROM kg_archive.deleted_entity_rows a
		WHERE a.edit_id = p_edit_id AND a.source = 'value_versions' AND a.restored_at IS NULL;
	WITH reopened AS (
		UPDATE public.value_versions v SET valid_to_key = NULL
		FROM kg_archive.deleted_entity_rows a
		WHERE a.edit_id = p_edit_id AND a.source = 'value_versions' AND a.restored_at IS NULL
			AND v.id = (a.row_data->>'id')::uuid AND v.valid_to_key = a.closed_to_key
		RETURNING a.archive_id
	), marked AS (
		UPDATE kg_archive.deleted_entity_rows a SET restored_at = now()
		WHERE a.archive_id IN (SELECT archive_id FROM reopened)
		RETURNING 1
	) SELECT count(*) INTO done FROM marked;
	source := 'value_versions'; restored := done; skipped := total - done; RETURN NEXT;

	-- relation_versions
	SELECT count(*) INTO total FROM kg_archive.deleted_entity_rows a
		WHERE a.edit_id = p_edit_id AND a.source = 'relation_versions' AND a.restored_at IS NULL;
	WITH reopened AS (
		UPDATE public.relation_versions v SET valid_to_key = NULL
		FROM kg_archive.deleted_entity_rows a
		WHERE a.edit_id = p_edit_id AND a.source = 'relation_versions' AND a.restored_at IS NULL
			AND v.id = (a.row_data->>'id')::uuid AND v.valid_to_key = a.closed_to_key
		RETURNING a.archive_id
	), marked AS (
		UPDATE kg_archive.deleted_entity_rows a SET restored_at = now()
		WHERE a.archive_id IN (SELECT archive_id FROM reopened)
		RETURNING 1
	) SELECT count(*) INTO done FROM marked;
	source := 'relation_versions'; restored := done; skipped := total - done; RETURN NEXT;
END
$fn$;
