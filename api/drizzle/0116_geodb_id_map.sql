-- The reverse id map from geo-db ids to gaia ids, kept by Postgres itself.
--
-- WHY. geo-db accepts only UUIDv4 ids, so every other gaia id (v1/v5/v7 and random hash ids; ~4% of
-- entities and ~12% of relations, and clients still mint them) is stored there under a one-way hash:
-- sha256("geo:legacy-<kind>:v1\0<32 hex>"), first 16 bytes, version and variant bits forced to v4. That is
-- conformingId in api/src/geodb/card.ts and geodb-feeder/src/ids.ts. Anything served from geo-db has to turn
-- those ids back into the gaia ids PostGraphile returns, and a hash cannot be reversed.
--
-- HOW. geodb.conform is the same hash in SQL, and an expression index over the legacy rows of entities makes
-- "which gaia id conforms to this geo-db id" one index probe. Postgres maintains it on every insert, so it
-- covers ids minted after this migration with nothing to backfill and no second writer to keep in step. The
-- index is partial (non-v4 ids only), so v4 rows, the other ~96%, pay one cheap predicate per write.
-- The api reads it in api/src/geodb/idMap.ts. The schema is not `public`, so PostGraphile does not expose it.
--
-- Large envs: build the index CONCURRENTLY first, so it does not block writes to entities while it builds
-- (then the IF NOT EXISTS below no-ops). The functions have to exist for it, so run this whole block:
--   CREATE SCHEMA IF NOT EXISTS geodb;
--   <the three CREATE OR REPLACE FUNCTION statements below>
--   CREATE INDEX CONCURRENTLY IF NOT EXISTS "entities_geodb_conform_idx" ON "entities" (geodb.conform('entity', "id")) WHERE NOT geodb.is_v4("id");
-- On testnet (48.9M entities, ~2M of them legacy) a 10% sample hashed in ~3 s, so expect the build in minutes.

CREATE SCHEMA IF NOT EXISTS geodb;
--> statement-breakpoint

-- RFC 4122 version 4: version nibble 4, variant 10xx. Exactly isUuidV4 in TypeScript.
CREATE OR REPLACE FUNCTION geodb.is_v4(id uuid) RETURNS boolean
LANGUAGE sql IMMUTABLE STRICT PARALLEL SAFE AS $$
  SELECT get_byte(uuid_send(id), 6) >> 4 = 4 AND get_byte(uuid_send(id), 8) >> 6 = 2
$$;
--> statement-breakpoint

-- Forces the version (4) and variant (10xx) bits of a 16-byte hash and reads it as a uuid.
CREATE OR REPLACE FUNCTION geodb.v4_bits(h bytea) RETURNS uuid
LANGUAGE sql IMMUTABLE STRICT PARALLEL SAFE AS $$
  SELECT encode(set_byte(set_byte(h, 6, (get_byte(h, 6) & 15) | 64), 8, (get_byte(h, 8) & 63) | 128), 'hex')::uuid
$$;
--> statement-breakpoint

-- The id geo-db holds for a gaia id: a v4 id is kept, any other is hashed under its kind ('entity' for
-- entities, properties and relation types and targets; 'statement' for relation ids).
CREATE OR REPLACE FUNCTION geodb.conform(kind text, id uuid) RETURNS uuid
LANGUAGE sql IMMUTABLE STRICT PARALLEL SAFE AS $$
  SELECT CASE WHEN geodb.is_v4(id) THEN id
    ELSE geodb.v4_bits(substring(sha256(
      ('geo:legacy-' || kind || ':v1')::bytea || '\x00'::bytea || replace(id::text, '-', '')::bytea
    ) FROM 1 FOR 16)) END
$$;
--> statement-breakpoint

CREATE INDEX IF NOT EXISTS "entities_geodb_conform_idx" ON "entities" (geodb.conform('entity', "id")) WHERE NOT geodb.is_v4("id");
--> statement-breakpoint

DO $$
BEGIN
  IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'gaia_app') AND current_user <> 'gaia_app' THEN
    EXECUTE 'GRANT USAGE ON SCHEMA geodb TO gaia_app';
    EXECUTE 'GRANT EXECUTE ON ALL FUNCTIONS IN SCHEMA geodb TO gaia_app';
  END IF;
END
$$;
