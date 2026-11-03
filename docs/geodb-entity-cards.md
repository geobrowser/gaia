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

## The space map

geo-db Space ids are not gaia's. A gaia Space id maps to the id geo-db founded it under, and the route
reads that mapping from a file, once, at the first request (a restart re-reads it):

```json
{"a19c345ab9866679b001d7d2138d88a1": {"geo": "daf0bc83ca744e1a9ee0377967846846"}}
```

Keys and `geo` are 32 hex digits (dashes are accepted). Extra fields are ignored. A file with any
malformed entry is refused as a whole, so every request falls back to Postgres rather than reading
the wrong Space. A Space absent from the file falls back with reason `unmapped_space`.

The map is not recomputed at load time and does not change between loads. geo-db derives a Space id
from its founding nonce under the store secret, so a fixed secret and nonce scheme give the same ids on
every load. Produce the file from the loader's output after a load, and keep it with the geo-db
store it describes (a different store secret founds different ids and needs its own map). With the
geo-shadow harness: `remote/ingest_loop.py` loads the corpus and writes `corpus/spaces-loaded.json`
(slug to geo-db id), then `node emit/space-map.mjs` joins that with the manifest's
`source.legacySpaceId` and writes `corpus/space-map.json`.

## Metrics and alerts

`/health/metrics` serves, only while the route is mounted:

- `gaia_api_geodb_card_served_total{source}` — responses by producing store.
- `gaia_api_geodb_card_fallback_total{reason}` — mode-on reads answered from Postgres, by `timeout`,
  `unmapped_space`, `client_failed`, `bad_request`, `overloaded`, `error`.
- `gaia_api_geodb_card_shadow_total{result}` — `match`, `only_postgres`, `only_geodb`, `both`, `failed`.
- `gaia_api_geodb_read_seconds{outcome}` — histogram of one geo-db read, queue wait included.

Draft alerts on these: `monitoring/k8s/v2/geodb-card-alerts.yaml` (not applied).
