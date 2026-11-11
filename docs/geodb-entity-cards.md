# geo-db entity cards (opt-in)

`GET /geodb/entities/:entityId/card?spaceId=<space>` reads one entity's statements in one Space from
geo-db, behind `GEODB_ENTITY_CARDS` (`off` default, `on`, `shadow`). Code: `api/src/geodb/`.

## Configuration

| variable | meaning |
| --- | --- |
| `GEODB_ENTITY_CARDS` | `off` (route not mounted), `on` (geo-db answers, Postgres per-request fallback), `shadow` (Postgres answers, geo-db compared in the background) |
| `GEODB_CLIENT_BIN` | the `geodb-client` binary (needs the `pipe` verb, geo-db `final` from f380cc031b) |
| `GEODB_KEY_PATH`, `GEODB_BINDING_PATH` | the principal's key file and the server's binding file |
| `GEODB_SPACE_MAP_PATH` | the space map, below |
| `GEODB_TIMEOUT_MS` | per-request budget, default 500 |
| `GEODB_POOL_SIZE` | long-lived `geodb-client pipe` workers, default 4 |
| `GEODB_MULTI_SPACE_READS` | `on`: the gateway shadow reads an entity's card across several Spaces in one `pipe` line (`<space>,<space>\t<geoql>`) instead of one read per Space. Needs a `geodb-client` from geo-db 353ab26313 or later; an older one refuses the line and loses its worker, so leave it off until the image is bumped. Default off |
| `GEODB_EVERY_SPACE_READS` | `on` (needs `GEODB_MULTI_SPACE_READS`): an `entity` answer that does not say which Spaces the entity lives in (`EntityTypes`, `EntityExists`, `ProfileSpaceIdForEntity`: `entity(id) { spaceIds }` alone, where every Space geo-db holds statements in must be one the answer names) is compared against a read of every Space in the space map, in one `pipe` line, under its own budget (0.5 reads/s). Default off |
| `GEODB_EVERY_SPACE_ENTITIES` | `on` (needs `GEODB_EVERY_SPACE_READS`): the same read for an `entities` answer with no lists, no `spaceIds` and no Space argument (`EntityNames`, `{ id }` under a Type or relation filter), on one sampled entity per operation. Default off |
| `GEODB_BACKLINK_READS` | `on`: the gateway's entity shadow also compares `EntityBacklinksPage` (`entity { backlinksList { spaceId fromEntity { id } } }`) with geo-db's card backlinks (`.args.backlinks`): one read of the filter's Space, or, with `GEODB_EVERY_SPACE_READS` and `GEODB_MULTI_SPACE_READS` on, one line over every Space. Results count under `field="entity"`; the geo-db read times under `shape="backlinks"`. Default off |
| `GEODB_ENTITIES_BATCH_SHADOW` | `on`: the light `entities` shadow accepts geogenesis's `EntitiesBatch` selection, whose unfiltered `allRelationsList { spaceId }` projection selects no edge. That list is counted `membership_only` and never compared; the rest compares as any light `entities` answer does (a sample of three entities, each read in its own `spaceIds`). Duplicate relations are collapsed and Postgres-derived rank relations are skipped, as in the other shadows. Default off |
| `GEODB_DERIVED_FIELDS_SHADOW` | `on`: the light `entities` shadow accepts the vote scores PostGraphile computes from Postgres (`upvotes`, `downvotes`, `agrees`, `disagrees`, `commenters`, `commentScore`). They are counted `derived_fields` and never compared; the rest of the selection (name, `spaceIds`, ...) compares as any light `entities` answer does. Default off |
| `GEODB_EVERY_SPACE_CHUNKS` | `on`: an every-Space read (an `entity`/`entities` answer that names no Space) over more loaded Spaces than one line can name (geo-db `MAX_SOURCES`, 1024) is split into lines of at most 1024 and the rows concatenated, instead of being refused `many_spaces`. Costs one extra read per 1024 Spaces against `everySpaceBudget`. The ordered page read (`GEODB_ORDER_PICK_SHADOW`) merges the lines' pages by key only for asc + least (`createdAt ASC`) and desc + greatest (`updatedAt DESC`); `createdAt DESC` and `updatedAt ASC` stay uncompared past 1024 Spaces. Backlinks read line by line and union the rows. Default off |
| `GEODB_EVERY_SPACE_TIMEOUT_MS` | Timeout in ms of an every-Space shadow read, in place of `GEODB_TIMEOUT_MS`. A read naming all 1579 loaded Spaces can pass 500 ms on a large entity, and counted as `failed` in `gaia_api_gateway_shadow_total` (about 20 an hour on 2026-11-06). Unset: the pool's timeout. Shadow reads only. |
| `GEODB_HOLDERS_LOOKUP` | `on` (needs `GEODB_EVERY_SPACE_READS`): an every-Space card read first asks geo-db which Spaces hold the entity (`.args.holders = true`, one read a line of at most 1024 Spaces) and reads the card in those Spaces alone, batched in lines; no holder is an empty answer with no card read. Needs a serve whose store has geo-db 48739dfef5; a store that refuses the argument is read the old way (the card across every Space) and not asked again for five minutes. Multi-entity card reads already batch through `.args.ids`, 32 to a read (`readCards`). Default off |
| `GEODB_ORDER_PICK_SHADOW` | `on` (needs `GEODB_MULTI_SPACE_READS`, `GEODB_EVERY_SPACE_READS` and `GEODB_EVERY_SPACE_ENTITIES`, and a serve whose store reads the order's `.pick` and `.ties`, geo-db d3967dffbf or later: an older store refuses the document): a light `entities` page with one type, no Space argument, a `first` and a single `orderBy: CREATED_AT_* \| UPDATED_AT_*` is also read from geo-db, ordered with `.pick` (least createdAt, greatest updatedAt, across every Space) and `.ties = .entity`, and compared with PostGraphile's page. No ties: the sequence is compared. Ties inside the page: its members are compared as a set (`order_ties`). A tie at the cut: nothing of the page is compared (`order_ties`, `order_completeness` stays). Default off |
| `GEODB_TYPE_SCAN_SHADOW` | `on` (needs a serve whose store reads `entities` `.args = .{ .types, .first }`, geo-db c2f9d09cfa or later: an older store refuses the document, counted `unsupported_selection` / `type_scan_refused`, and costs one worker restart per refused read): a light `entities` page with one Space argument, one type, no `orderBy`, a `first` and an optional `offset` is also read from geo-db as the type scan with `first = offset + first`, the first `offset` ids dropped, and the ids compared as a sequence with PostGraphile's primary-key-order page. A window past 4096 is not read (`type_scan_window`). Order is exact only for conforming ids. Default off |
| `GEODB_COUNT_ONLY_SHADOW` | `on` (needs a serve whose store has the entities count form with `.where`, geo-db DR-154 item 8: an older store refuses the document, counted `unsupported_selection` / `count_refused`, and costs one worker restart per refused read): an `entitiesConnection` that asks only `totalCount` (alone or several aliased roots, at most 8) with `filter: {and: [{typeIds: {overlaps: [T]}}, {relations: {some: {typeId: {is: R}, toEntityId: {in: [..]}}}}]}` (one type, at most one relation type, at most 4 targets) is read from geo-db as `.args = .{ .type = T, .count = true, .where = .{ R = .{ .in = targets } } }`, the pages summed until `complete`, and `members` is compared with `totalCount`. A root with no Space argument is counted across every Space (a `*` line; needs `GEODB_EVERY_SPACE_READS`). A shape the form cannot state stays `unsupported_selection` / `count_only`; a walk that did not finish is `count_incomplete`. Default off |
| `GEODB_RELATIONS_TYPE_FILTER_SHADOW` | `on`: the light `entities` shadow accepts a relations list filtered by `typeId` (`is` or `in`, at most 20 Types) that selects no type (`relations(first, filter: {typeId: {in}}) { nodes { toEntity { id name } } }`). Each row is judged as held when geo-db has a relation from the entity to that target under one of the filter's Types (counted `filter_completeness`; a filter of derived Types only is not judged). Default off |
| `GEODB_PROPERTY_INFO_SHADOW` | `on` (needs `GEODB_EVERY_SPACE_READS`): a values row's `property { dataTypeId dataTypeName renderableTypeId renderableTypeName format isType }` is compared with the Property's own card (one batched `.args.ids` read across every Space, at most 64 Properties in two reads, under the every-Space budget). A data or renderable type must be among geo-db's relation targets (null: geo-db holds none), a name among their names, `format` and `isType` as the Property's statements. A Property geo-db holds nothing of is counted `no_statements`, a name geo-db prints none for `cross_space`; neither is a mismatch. Settles `property_info`. Default off |
| `GEODB_RELATIONS_MEMBERSHIP_SHADOW` | `on`: an unfiltered `allRelationsList { spaceId }` (`Entity` card query; `EntitiesBatch` projection with `GEODB_ENTITIES_BATCH_SHADOW`) is compared with geo-db's reference statements of the entity (`.args.relations = true`, geo-db ISS-590-263; an older serve refuses it, counted `failed`) read in the Spaces PostgreSQL says hold it: a Space where geo-db holds more relations than PostgreSQL lists is a mismatch. A cut list is not judged. One more read an entity (every-Space budget on `Entity`). Settles `membership_only`. Default off |
| `GEODB_CROSS_SPACE_HOLDERS_SHADOW` | `on` (needs `GEODB_MULTI_SPACE_READS`; the every-Space fallback needs `GEODB_EVERY_SPACE_READS`, narrowed by `GEODB_HOLDERS_LOOKUP`): an `entity` card comparison, or one from the light `entities` shadow's named-Space reads, that would be counted `cross_space` (the name, description, Types, `spaceIds` or an unfiltered list came from Spaces the first read did not cover) is judged again against the card read across every Space PostgreSQL says hold the entity, as one multi-Space line, or across every loaded Space narrowed by `.args.holders` when the entity is in more Spaces than a line takes. The statement diff stays on the Space asked for. A value no Space holds is now a mismatch. A Space geo-db has not loaded, a refused budget or a failed read keeps `cross_space`. One extra read per such comparison. Default off |

## The space map

geo-db Space ids are not gaia's. A gaia Space id maps to the id geo-db founded it under, and the route
reads that mapping from a file:

```json
{"a19c345ab9866679b001d7d2138d88a1": {"geo": "daf0bc83ca744e1a9ee0377967846846"}}
```

Keys and `geo` are 32 hex digits (dashes are accepted). `"loaded": false` marks a founded Space whose
history has not been backfilled: the api treats it as not loaded (`unmapped_space`, and the shadows count
it unsupported rather than as a gap), and the feeder writes it for a Space outside the backfill's finished
set. Other extra fields are ignored. A file with any
malformed entry is refused as a whole, so every request falls back to Postgres rather than reading
the wrong Space. A Space absent from the file falls back with reason `unmapped_space`.

The file is checked for a change (mtime and size) at most every 10 seconds and reloaded when it
changed, so a Space the feeder founds becomes readable without restarting the api. If a reload fails the
previous map stays in use; only a map that has never loaded makes requests fall back.

In the cluster the file is produced by the feeder (`geodb-feeder/`): it founds each gaia Space in geo-db
online over the wire (`geodb-client found`) on first sight under a deterministic nonce, records the pair, and publishes the whole map to the
ConfigMap `geodb-space-map` (key `space-map.json`), which the api pods mount. The ids are stable for a
fixed store secret, so a different store secret founds different ids and needs its own map. The same
ConfigMap carries `node-id` (serve's node key id, which the api's binding pins) and `sealer-id` (the
key id Spaces are founded under), both published by serve's supervisor (see `geodb/RUNBOOK.md`).
Outside the cluster, with the geo-shadow harness: `remote/ingest_loop.py` loads the corpus and writes
`corpus/spaces-loaded.json` (slug to geo-db id), then `node emit/space-map.mjs` joins that with the
manifest's `source.legacySpaceId` and writes `corpus/space-map.json`.

## Metrics and alerts

`/health/metrics` serves, only while the route is mounted:

- `gaia_api_geodb_card_served_total{source}` — responses by producing store.
- `gaia_api_geodb_card_fallback_total{reason}` — mode-on reads answered from Postgres, by `timeout`,
  `unmapped_space`, `client_failed`, `bad_request`, `overloaded`, `error`.
- `gaia_api_geodb_card_shadow_total{result}` — `match`, `only_postgres`, `only_geodb`, `both`, `failed`.
- `gaia_api_geodb_read_seconds{outcome}` — histogram of one geo-db read, queue wait included.

Draft alerts on these: `monitoring/k8s/v2/geodb-card-alerts.yaml` (not applied). The feeder's own alerts are in `monitoring/k8s/v2/geodb-feeder-alerts.yaml`; deploy, rollout and rollback are in `geodb/RUNBOOK.md`.
