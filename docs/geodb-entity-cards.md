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
| `GEODB_EVERY_SPACE_READS` | `on` (needs `GEODB_MULTI_SPACE_READS`): an `entity` answer that does not say which Spaces the entity lives in (`EntityTypes`, `EntityExists`) is compared against a read of every Space in the space map, in one `pipe` line, under its own budget (0.5 reads/s). Default off |
| `GEODB_EVERY_SPACE_ENTITIES` | `on` (needs `GEODB_EVERY_SPACE_READS`): the same read for an `entities` answer with no lists, no `spaceIds` and no Space argument (`EntityNames`, `{ id }` under a Type or relation filter), on one sampled entity per operation. Default off |
| `GEODB_BACKLINK_READS` | `on`: the gateway's entity shadow also compares `EntityBacklinksPage` (`entity { backlinksList { spaceId fromEntity { id } } }`) with geo-db's card backlinks (`.args.backlinks`): one read of the filter's Space, or, with `GEODB_EVERY_SPACE_READS` and `GEODB_MULTI_SPACE_READS` on, one line over every Space. Results count under `field="entity"`; the geo-db read times under `shape="backlinks"`. Default off |

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
