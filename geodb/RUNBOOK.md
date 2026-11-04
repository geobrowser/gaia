# geo-db entity cards, stage 1: runbook (testnet)

What this stands up: a geo-db replica (StatefulSet `geodb-0` running `serve`, plus a separate `geodb-feeder` Deployment) fed from
`testnet.knowledge.edits`, and the api reading entity cards from it behind `GEODB_ENTITY_CARDS`.
Postgres stays the system of record and the fallback. The flag ships `off`; nothing changes for
users until you flip it. Context is `geo-testnet-k8s`, namespace `gaia`.

```bash
K="kubectl --context do-nyc2-geo-testnet-k8s -n gaia"   # adjust the context name to yours
```

## 0. Before you start (once)

Four things only a human with the right access can make. None is in git.

1. A GitHub token for the image build (geo-db and its siblings are private): a fine-grained PAT with
   read-only Contents on `geobrowser/geo-db`, `atom-swap`, `zblake3`, `envelope`, `auth`, `tidy`,
   `zrapidhash`, `grc-20-new`. Add it as the gaia Actions secret `GEO_DB_READ_TOKEN`.
2. A dedicated Kafka user for the feeder (not `kafka-credentials`, not the kg-indexer's): in the DO
   Kafka cluster, create user `geodb-feeder` and give it consume (read) on `testnet.knowledge.edits`
   and nothing else. Its consumer group is `geodb-feeder-v1-testnet`; if the cluster lists group ACLs
   separately, grant read on that group too.
3. The four Secrets, created from files so nothing lands in shell history. `geodb-client keygen`
   comes from the image (step 1) or any build of geo-db `final`:

```bash
# feeder's Kafka login; broker and CA are public values, copied from the kg-indexer's secret
BROKER=$($K get secret kg-indexer-secrets -o jsonpath='{.data.KAFKA_BROKER}' | base64 -d)
CA=$($K get secret kg-indexer-secrets -o jsonpath='{.data.KAFKA_SSL_CA_PEM}' | base64 -d)
$K create secret generic geodb-feeder-kafka \
  --from-literal=KAFKA_BROKER="$BROKER" --from-literal=KAFKA_SSL_CA_PEM="$CA" \
  --from-literal=KAFKA_USERNAME=geodb-feeder --from-file=KAFKA_PASSWORD=./geodb-feeder-kafka-password.txt

# store secret: fixes every Space id. Create once, never rotate (a new one founds different ids).
od -An -N32 -tx1 /dev/urandom | tr -d ' \n' > secret.hex
$K create secret generic geodb-store-secret --from-file=secret.hex=./secret.hex

# the feeder's write key and the api's read-only key
for who in writer reader; do
  docker run --rm -v "$PWD:/k" -w /k registry.digitalocean.com/geo/geodb:beedb70fd4 \
    geodb-client --key $who.key keygen > $who.keyid
  $K create secret generic geodb-$who-key --from-file=key=./$who.key --from-file=keyid=./$who.keyid
done
shred -u secret.hex writer.key reader.key 2>/dev/null || rm -P secret.hex writer.key reader.key
```

   `keyid` must be the bare 64-hex id with no trailing text. Check it with `cat writer.keyid` before the
   `create secret` lines, and re-run `keygen` if it prints anything else.
4. Nothing to create for `geodb-space-map`: serve's supervisor creates it and publishes `node-id` and
   `sealer-id`; the feeder adds `space-map.json`.

## 1. Deploy order

Each step is a manual dispatch from `main` after the PR is merged. Wait for green before the next.

```bash
# a. geodb image (serve + client), pinned at geo-db final beedb70fd4. ~1 h of compile.
gh workflow run build-geodb-image.yml --ref main -f geodb_ref=beedb70fd4
# b. feeder image
gh workflow run build-v2-images.yml --ref main -f service=geodb-feeder
# c. the replica: StatefulSet geodb, Deployment geodb-feeder, Services, RBAC, serve config. TAG = the 8-char sha of the run in (b).
gh workflow run deploy-v2.yml --ref main -f service=geodb-feeder -f tag=<TAG> -f cluster=geo-testnet-k8s
$K rollout status statefulset/geodb && $K rollout status deployment/geodb-feeder
# d. alerts
kubectl --context do-nyc2-geo-testnet-k8s apply -f monitoring/k8s/v2/geodb-feeder-alerts.yaml \
  -f monitoring/k8s/v2/hermes-metrics-servicemonitor.yaml
# e. api, last: it carries the init container that reads the node id, and it needs geodb:beedb70fd4 to
#    exist in the registry (step a) or its pods stay in ImagePullBackOff and the rollout stalls.
gh workflow run build-v2-images.yml --ref main -f service=api
gh workflow run deploy-v2.yml --ref main -f service=api -f tag=<API TAG> -f cluster=geo-testnet-k8s
```

Why (e) comes after (c): the api's init container writes its binding from `node-id` in ConfigMap
`geodb-space-map`, which serve's supervisor publishes on its first start. If the api rolled out first it
would start without a binding and answer every card from Postgres. Harmless, but fix it by
`$K rollout restart deployment/api` once the ConfigMap has `node-id`.

Check the replica is up and filling:

```bash
$K get pod geodb-0                        # 1/1 Running (serve)
$K get pod -l app=geodb-feeder            # 1/1 Running (feeder)
$K get cm geodb-space-map -o jsonpath='{.data.node-id}{"\n"}{.data.sealer-id}{"\n"}'   # 64 hex each
$K logs deploy/geodb-feeder --tail=20     # JSON lines: "founded space", "applied"
$K logs geodb-0 --tail=20                 # serve and supervisor
$K port-forward svc/geodb-feeder 9090 &  curl -s localhost:9090/metrics | grep -E 'consumer_lag|spaces|dead_letters'
```

The first start founds one geo-db Space per gaia Space, online over the wire (`geodb-client found`);
serve is never restarted for it, but a long topic with many Spaces still takes a while before the lag
falls. That is normal; `geodb_feeder_spaces` rises, then `geodb_feeder_consumer_lag` falls.

## 2. Turn on shadow mode

Shadow answers every card from Postgres and compares with geo-db in the background; users see no change.

```bash
$K set env deployment/api GEODB_ENTITY_CARDS=shadow
$K rollout status deployment/api
```

`kubectl set env` is undone by the next `deploy-v2` of api (the manifest says `off`). To make it stick,
change `GEODB_ENTITY_CARDS` in `api/k8s/v2/api.yaml` in a follow-up PR once you are happy.

Watch (in Grafana or `/health/metrics` on an api pod):

```
sum by (result) (rate(gaia_api_geodb_card_shadow_total[10m]))
histogram_quantile(0.99, sum by (le) (rate(gaia_api_geodb_read_seconds_bucket[10m])))
```

Results: `match`, `only_postgres`, `only_geodb`, `both`, `failed`. `failed` means geo-db could not be asked
(binding, key, space map, or serve down).

### 2a. Replay traffic

Testnet has too few people opening cards for a week of shadow to mean much. Deployment `geodb-card-replay`
(`geodb/k8s/v2/geodb-card-replay.yaml`, same image as the feeder) calls the card route in cluster, through
Service `api` (`http://api:3000/geodb/entities/<id>/card?spaceId=<space>`), at `REPLAY_RATE` requests per
second: 0.5 by default, and the driver refuses more than 1 or any host other than Service `api`. It never
uses the public endpoint.

Pairs are drawn from gaia Postgres (read only) and refreshed every 15 minutes, only for Spaces in the
published space map: 50% from each Space's top `local_scores` entities (Zipf by rank, the proxy for what
people open), 30% recently edited entities, 20% a uniform sample. A Space mapped out since the last refresh
is skipped and counted (`result="skipped_unmapped"`), never requested.

It ships with the feeder (`deploy-v2 -f service=geodb-feeder` applies both files). If the feeder was deployed
before this file existed, deploy it again with the same tag, or apply only the replay:

```bash
sed "s|geodb-feeder:latest|geodb-feeder:<TAG>|" geodb/k8s/v2/geodb-card-replay.yaml | $K apply -f -
$K rollout status deployment/geodb-card-replay
$K logs deploy/geodb-card-replay --tail=5     # "replay pools refreshed" with top/recent/random counts
```

Its own series (scraped through the shared ServiceMonitor, app `geodb-card-replay`):

```
sum by (result) (rate(geodb_card_replay_requests_total[10m]))   # ok, http_error, error, skipped_unmapped
geodb_card_replay_pairs                                           # pool sizes, by pool
time() - geodb_card_replay_last_refresh_seconds                   # pool age; > 1800 means refreshes are failing
```

Stop it with `$K scale deployment/geodb-card-replay --replicas=0`; change the rate with
`$K set env deployment/geodb-card-replay REPLAY_RATE=0.2`.

### 2b. Backfill (once, before the week starts)

The feeder only knows what passed through the topic since its group started, so most cards begin
`only_postgres` (2026-11-03: 55 of 61 sampled pairs, 90%). Job `geodb-backfill`
(`geodb/k8s/v2/geodb-backfill-job.yaml`, same image, `node dist/backfill-main.js`) loads the current state of
every entity geo-db does not know yet, from gaia Postgres (read only), through the feeder's own translator
and apply path, onto the feeder's own volume and journals.

Why it is safe to run against a live replica (full argument in `geodb-feeder/src/backfill.ts`):

- **Ordering.** It runs only while the feeder is scaled to 0, and checks before it starts that the
  feeder's consumer group has no members, that no feeder message is half applied, and that the kg-indexer
  (which writes Postgres) has committed past every offset the feeder has applied, on every partition. So
  Postgres already holds everything geo-db does, and nothing newer arrives in geo-db while it runs. It
  never overwrites a cell geo-db holds: an entity the journal does not know gets all its values and
  relations; one it knows is read back from geo-db first, and only properties with no row and relations
  whose statement id is absent are written. Everything written is recorded in the same journal. When the
  feeder resumes, it replays every edit since its stored offset in topic order on top, so the newest value
  wins (a `set` replaces the backfilled cell; an `unset` or delete applies, because the journal now knows the
  entity or statement). It re-checks the group every minute and stops if the feeder comes back.
- **Idempotence.** A cursor on the volume (`/data/feeder/backfill/`) records the finished batches. A rerun
  after a crash resumes from the last finished batch; the batch that was in flight is marked, and its
  entities are all read back from geo-db first, so nothing they already got is written twice.
- **Spaces.** A gaia Space with no geo-db Space is founded exactly as the feeder founds it
  (`geodb-client found`, same sealer), up to `BACKFILL_SPACE_CAP` (192, under serve's `spaces = 256`), and
  the space map (`geodb-space-map`) is republished before the Job exits.
- **Load.** 100 entities per batch, chunks of 200 intents, 100 ms pause after each apply; one connection
  to Postgres, one batch query at a time, each in a `READ ONLY` transaction with a statement timeout.

Size (testnet, 2026-11-03): mapped Spaces hold ~1.18M entities, ~2.9M values and ~4.8M relations, about 7.6M
intents; at the paced rate that is roughly 1.5 to 2 hours. `all` (1,574 Spaces, ~11.5M rows) is larger and
founds Spaces up to the cap.

**Serve memory is the risk.** geodb-0 already sits at its 4Gi limit (`kubectl top`, 2026-11-03) with a
fraction of this data. Stage the run and watch it; if serve logs `exhausted` or restarts, stop and raise
serve's memory or profile before going on.

```bash
TAG=<the feeder tag deployed now>
# 0. silence GeoDbFeederDown and GeoDbFeederLagHigh for ~3h: the feeder is down on purpose, and catches up after.
# 1. stop the feeder and wait for its group to be empty (it leaves on SIGTERM).
$K scale deployment/geodb-feeder --replicas=0
$K wait --for=delete pod -l app=geodb-feeder --timeout=120s
# 2. stage 1: just the Space the exit check samples, to see it work end to end.
sed -e "s|geodb-feeder:latest|geodb-feeder:$TAG|" -e "s|__SPACES__|41e851610e13a19441c4d980f2f2ce6b|" \
  geodb/k8s/v2/geodb-backfill-job.yaml | $K apply -f -
$K logs -f job/geodb-backfill          # "ordering precondition" ok:true, then "backfill batch", "backfill done"
$K top pod geodb-0 --containers        # serve memory, while it runs
$K delete job geodb-backfill
# 3. stage 2: the mapped Spaces, capped first (BACKFILL_MAX_ENTITIES), then without the cap. Same command with
#    __SPACES__ -> mapped, and (first) -e 's|value: "0"|value: "50000"|' for the cap. Delete the Job between runs.
# 4. optional: __SPACES__ -> all, which also founds Spaces geo-db does not have yet (largest first).
# 5. start the feeder again; it replays every edit since its stored offset, then the lag falls to ~0.
$K scale deployment/geodb-feeder --replicas=1
$K rollout status deployment/geodb-feeder
```

If the Job fails, read why (`$K logs job/geodb-backfill`), delete it, and apply it again: it resumes. If you
must give up, just scale the feeder back to 1; whatever was written is in the journal and correct.

Check it worked:

```bash
# the exit check's sample card: shadow result for it should be match (no "geodb card diff" line for it)
$K get --raw "/api/v1/namespaces/gaia/services/api:3000/proxy/geodb/entities/073ff16be7fe4591a984c325d629d92d/card?spaceId=41e851610e13a19441c4d980f2f2ce6b"
$K logs deploy/api --since=2m | grep 073ff16be7fe4591a984c325d629d92d
# the only_postgres share over the replay's sample, before vs after (compare a window before step 0 with one after step 5)
sum(increase(gaia_api_geodb_card_shadow_total{result="only_postgres"}[1h])) / sum(increase(gaia_api_geodb_card_shadow_total[1h]))
```

`only_postgres` does not go to 0: properties gaia derives itself and no edit carries
(`BACKFILL_EXCLUDE_PROPERTIES`, default the list in `geodb-feeder/src/pgAdapter.ts`) are left out on
purpose, so the replica holds only what the stream would give it.

## 3. The exit check: a week of traffic

Do not turn serve mode on until all of these hold over 7 days of shadow:

- `failed` is about 0: `sum(increase(gaia_api_geodb_card_shadow_total{result="failed"}[7d])) / sum(increase(gaia_api_geodb_card_shadow_total[7d])) < 0.001`.
- Differences are low and steady: `(only_postgres + only_geodb + both) / total` stays under 1%, and any bump
  matches a known cause. Known steady causes: edits older than the topic's retention window
  (`geodb_feeder_translate_total{counter="entity-predates-window"}`), and the translator's documented
  mapping gaps (PR #1054). Find specific cards from the api's shadow log lines.
- The replica keeps up: `geodb_feeder_consumer_lag` returns to near 0 after bursts, and
  `geodb_feeder_dead_letters_total` is flat or every dead letter is explained
  (`$K exec deploy/geodb-feeder -- cat /data/feeder/errors.jsonl`).
- p99 `gaia_api_geodb_read_seconds` is well under the 500 ms budget (`GEODB_TIMEOUT_MS`).
- No `GeoDbFeederDown`, `GeoDbFeederStalled`, `ApiGeoDbCardShadowFailing` alert in the window.

The topic does not hold full history; §2b's backfill fills in the older state once. Run it before the week
starts, so the week measures the replica as it would serve, not its coverage gap.

Reading the week. With the replay (§2a) running, most samples are its own, so the totals are steady and
comparable day to day:

```
# share of each result over the week (the replay's ~300k requests plus real users)
sum by (result) (increase(gaia_api_geodb_card_shadow_total[7d])) / ignoring(result) group_left sum(increase(gaia_api_geodb_card_shadow_total[7d]))
# day by day, to see a trend or a bump
sum by (result) (increase(gaia_api_geodb_card_shadow_total[1d]))
# the replay itself was healthy all week: ok dominates, error/http_error near 0
sum by (result) (increase(geodb_card_replay_requests_total[7d]))
```

A gap in `geodb_card_replay_requests_total{result="ok"}` is a gap in the sample, not a replica problem;
`http_error` or `error` from the replay means the api route itself failed. `skipped_unmapped` is a Space
that left the map. The diffs worth reading are in the api's `geodb card diff` log lines (entity, space, and
a sample of the differing statements).

## 4. Turn on serve mode

```bash
$K set env deployment/api GEODB_ENTITY_CARDS=on
$K rollout status deployment/api
```

geo-db answers; each request that times out or errors falls back to Postgres on its own, so users still get
a card. Watch `ApiGeoDbCardFallbackRateHigh` (more than 5% falling back for 10m). Break it down with
`sum by (reason) (rate(gaia_api_geodb_card_fallback_total[10m]))`. Apply the draft rules first:
`kubectl apply -f monitoring/k8s/v2/geodb-card-alerts.yaml`.

## 5. Rollback

Flag off. Nothing else is needed, and no data is lost:

```bash
$K set env deployment/api GEODB_ENTITY_CARDS=off      # or: rollout undo deployment/api
$K rollout status deployment/api
```

With the flag off the route is not mounted and the api never spawns `geodb-client`. The replica can keep
running (it only costs one pod and a volume), or be stopped with `$K scale statefulset/geodb --replicas=0`.
The PVC is kept; scaling back up resumes from the stored offset. To remove it entirely, delete the
StatefulSet, the Deployment `geodb-feeder`, the PVCs `data-geodb-0` and `geodb-feeder-state`, and the four Secrets and the `geodb-space-map` ConfigMap.

## 6. Alerts (monitoring/k8s/v2/geodb-feeder-alerts.yaml)

| alert | means | first look |
| --- | --- | --- |
| `GeoDbFeederDown` | no feeder series, serve down, or consumer down for 10m | `$K get pod geodb-0` and `-l app=geodb-feeder`; `logs deploy/geodb-feeder`, `logs geodb-0`; Kafka credentials and ACL |
| `GeoDbFeederStalled` | lag above 0 and nothing applied for 15m | the repeating error in the feeder log (`transient` = serve unreachable, `exhausted` = serve is out of capacity: `$K rollout restart statefulset/geodb`) |
| `GeoDbFeederLagHigh` | more than 5000 behind for 30m | expected once on first catch-up; otherwise throughput or a stall |
| `GeoDbFeederDeadLetters` | an edit or intent was skipped | `errors.jsonl` on the volume (see above) |

## 7. Operating notes

- Crash recovery: the feeder resumes from its stored offset (`/data/feeder/offsets.json`), and an
  interrupted message is replayed from the intent it stopped at. Applies are idempotent, so a replay is
  safe. Kafka's own group offset is only used to report lag.
- Replaying from scratch: scale both workloads to 0, delete PVCs `data-geodb-0` and `geodb-feeder-state`, scale both to 1. Space ids come out the same
  because the store secret and the founding nonces are deterministic.
- New Spaces become readable to the api within the grant lifetime (`GEODB_GRANT_TTL_S`, 300 s) and the
  space-map refresh (10 s).
- Founding is online: no serve restart. If serve is restarted by hand, the feeder retries its connection.
- Capacity: `spaces = 256` in `geodb-serve-config` is geo-db's Space capacity. Testnet maps 99 gaia Spaces
  (2026-11-03); a backfill with `all` founds more, up to `BACKFILL_SPACE_CAP` (192).
  Serve's memory is at its 4Gi limit at the medium profile before any backfill: watch it through §2b. If the feeder log shows founding refused
  for capacity, `GeoDbFeederStalled` fires; raise `spaces`, redeploy, and the feeder resumes.
  The space-map ConfigMap (1 MiB limit) holds roughly 15,000 Spaces, so it is not the constraint.
- Upgrading geo-db: build a new image tag, change the pinned tag in `geodb/k8s/v2/geodb.yaml` and
  `api/k8s/v2/api.yaml` together, and redeploy. Read the geo-db release notes for store-format changes first.
