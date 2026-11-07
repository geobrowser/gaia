# geodb-feeder: the legacy id map, and its backfill (plan only)

Status: the live-path writer is in this PR behind `GEODB_LEGACY_ID_MAP` (default off). The backfill below is a plan;
nothing here has been run, and the sizes come from code, migrations and docs, not from a live count.

## What is written

geo-db's GAP-10 (ISS-629-174, `final` bac1c75004) answers "which gaia id is this geo-db id" from one statement per
non-v4 entity:

    (keyed, sys.legacy.Id, Bytes(the original id's 16 raw bytes))

- `keyed` is the id the feeder already stores the entity under: `conformingId("entity", original)`, gaia's sha256
  remap. geo-db's reads take the map from the statement and never recompute the keyed id, so the feeder does not need
  to move to `sys_ids.legacyEntityId`.
- The statement id is `sys_ids.legacyStatementId(keyed, original)`: the record key of kind `sys.legacy.Entity` over
  `[id keyed, id sys.legacy.Id, bytes original16]`. `legacyMap.ts` computes it with the record-key port the feeder
  already has; `__tests__/vectors/legacy-map.json` pins it to geo-db's own derivation.
- The value is 16 raw bytes. A 32-character hex text would be refused by a value-range read (only 19 payload bytes
  have an exact key).
- It is written wherever the feeder creates an entity from a non-v4 id (a `createEntity`, an `updateEntity` or a
  relation `from` for an entity new to the Space), in the same Space and the same apply as the `create_entity`.
  Relation ids are statement ids and are not covered by GAP-10.

### Blocker before the flag goes on: the client has no Bytes edit form

`geodb-client`'s edit grammar knows text, number, boolean, point, reference, instant,
date and decimal. It has no Bytes form, so an `add` carrying `.{ .bytes = "<32 hex>" }` is refused, and a refused
intent fails the whole edit. This PR emits that form because it is the obvious one (hex text for 16 bytes, as the
`reference` form carries ids), but the engine owner has to land and name it before `GEODB_LEGACY_ID_MAP` is set
anywhere. 
## Size

From code and docs only:

- gaia's migration `0116_geodb_id_map.sql` puts non-v4 ids at about 4% of entities and about 12% of relations, and
  testnet at 48.9M entities with ~2M legacy ones.
- `geodb/RUNBOOK.md` 2b sizes the mapped Spaces at ~1.18M entities. At 4% that is roughly 47,000 non-v4 entities in the
  mapped Spaces.
- So the backfill is on the order of 50,000 statements for the mapped Spaces and about 2,000,000 for the whole corpus
  (`BACKFILL_SPACES=all`). One statement per (entity, Space) pair, so an entity present in several Spaces adds more;
  the 4% is global rather than per Space, so treat both figures as estimates to be replaced by one read-only count
  before sizing a run (see the last section).
- Cost per statement is one `add` intent of about 150 bytes, against 7.6M intents for the full mapped backfill: the
  mapped part is under 1% of that run.

## Which ids get statements, and how

1. **New entities** (any entity the feeder creates once the flag is on) get the statement inline. Nothing to do.
2. **Entities loaded by the Space backfill** (`backfill.ts`) after the flag is on also get it inline: the backfill job
   passes `config.legacyIdMap` to the same `translateOps`. So set `GEODB_LEGACY_ID_MAP=1` in the backfill Job's env
   before the not-yet-run stages (the `mapped` and `all` stages of RUNBOOK 2b), and those entities need no second pass.
3. **Entities geo-db already holds** (the journal knows them, so `ensure` never re-creates them) get nothing from 1 or
   2. They need a dedicated pass, a map backfill modelled on the times backfill (`timesBackfill.ts`):
   - Source: gaia Postgres, read-only, `SELECT id FROM entities WHERE NOT geodb.is_v4(id)`, keyset-paged by id (the
     partial conform index from migration 0116 serves the predicate), joined to the Spaces where the entity has
     statements, since the statement lives in every Space where the entity does.
   - For each (entity, Space): skip it when geo-db holds no row for the keyed entity in that Space (never create a
     phantom entity, as the times backfill does), else send one `add` with the map statement id. The id is derived,
     and a replay of the same id and body is an audited no-op, so no read-back of the statement is needed and a rerun
     is free.
   - Cursor: per-Space index on the feeder's volume, apart from the other backfills' cursors, as for the times backfill.
   - Pacing: the same `BACKFILL_BATCH`, `BACKFILL_PAUSE_MS` and `BACKFILL_MAX_ENTITIES` knobs, a staged first run on one
     Space, then the mapped Spaces capped, then uncapped.
   - Entities deleted and re-created: `delete_entity` removes the map statement with the entity's other cells, and the
     journal keeps the entity known, so a re-create by replay does not write it again. The map backfill repairs this too
     because it keys on what geo-db holds now.

## Ordering constraints with the feeder's consumer groups

- **The main edit consumer** (`config.kafka.groupId`) is the one that creates entities. Turn the flag on in the
  deployed feeder first, so every entity created after the cutover is already mapped and the backfill chases a fixed set.
  Entities created between "flag on" and the end of the backfill are written by both, which is harmless (same id, same
  body).
- **The backfill must run with that group empty**, exactly as the Space backfill and the times backfill do
  (`ordering.ts`): feeder scaled to 0, no message in flight, the kg-indexer committed past the feeder's last applied
  offset. The map backfill reads a set of ids, not values, so the Postgres-versus-geo-db staleness argument is
  weaker, but it still writes through the feeder's apply path and journals, which a running consumer must not share.
  Use the same guard and the same re-check each minute.
- **The keyed consumers** (membership, votes, proposal votes, proposal actions, subspaces, voting settings, proposal
  versions and status, ranks, space topics, space addresses, proposals, space entities) have their own groups and
  offsets. They write natural-key records into the feeder's system Spaces under record-key ids, not entities keyed by
  `conformingId` of an entity id, so they are neither blocked by nor part of this work. They do not need to be stopped
  for the map backfill, but they share the serve and the volume, so keep the run staged. The space-entities
  consumer does create entities, from `space.creations`; it uses `cid("entity", ...)` on Space
  ids, which are v4 or are mapped like any other entity if not, and it does not call `translateOps`, so it writes no map
  statement. If any Space id is non-v4, the map backfill covers its system entity (a row of `entities`).
- **Do not run it before serve is on bac1c75004 or later and the client has the Bytes edit form.** An older serve does
  not know the atoms' semantics (the statement is an ordinary statement there, harmless but unread); an older client
  refuses the form and, with the flag on, the whole edit.
- **gaia#1331's idMap twin** (`GEODB_ID_MAP_TWIN`) counts a non-v4 id `unmapped_in_geodb` until its statement exists.
  Read its mismatch metric after the live flag is on and again after the backfill; the backfill is done when
  `unmapped_in_geodb` is 0 for ids the api has asked about.

## The one count to take before running (read-only, held for Patrick)

Before the run, count what it will write, per mapped Space, with one read-only query against gaia Postgres, for
example the number of distinct (entity, space) pairs among `values` and `relations` rows whose entity id fails
`geodb.is_v4`. That replaces the estimates above. This PR does not run it.
