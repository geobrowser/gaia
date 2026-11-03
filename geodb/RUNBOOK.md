# geo-db entity cards, stage 1: runbook (testnet)

What this stands up: a geo-db replica (`geodb-0`: `serve` + `feeder` containers) fed from
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
  docker run --rm -v "$PWD:/k" -w /k registry.digitalocean.com/geo/geodb:47b7666dd1 \
    geodb-client --key $who.key keygen > $who.keyid
  $K create secret generic geodb-$who-key --from-file=key=./$who.key --from-file=keyid=./$who.keyid
done
shred -u secret.hex writer.key reader.key 2>/dev/null || rm -P secret.hex writer.key reader.key
```

   `keyid` must be the bare 64-hex id with no trailing text. Check it with `cat writer.keyid` before the
   `create secret` lines, and re-run `keygen` if it prints anything else.
4. Nothing to create for `geodb-space-map`: the feeder makes that ConfigMap itself.

## 1. Deploy order

Each step is a manual dispatch from `main` after the PR is merged. Wait for green before the next.

```bash
# a. geodb image (serve + client), pinned at geo-db final 47b7666dd1. ~1 h of compile.
gh workflow run build-geodb-image.yml --ref main -f geodb_ref=47b7666dd1
# b. feeder image
gh workflow run build-v2-images.yml --ref main -f service=geodb-feeder
# c. the replica: StatefulSet, Service, RBAC, serve config. TAG = the 8-char sha of the run in (b).
gh workflow run deploy-v2.yml --ref main -f service=geodb-feeder -f tag=<TAG> -f cluster=geo-testnet-k8s
$K rollout status statefulset/geodb
# d. alerts
kubectl --context do-nyc2-geo-testnet-k8s apply -f monitoring/k8s/v2/geodb-feeder-alerts.yaml \
  -f monitoring/k8s/v2/hermes-metrics-servicemonitor.yaml
# e. api, last: it carries the init container that reads the node id, and it needs geodb:47b7666dd1 to
#    exist in the registry (step a) or its pods stay in ImagePullBackOff and the rollout stalls.
gh workflow run build-v2-images.yml --ref main -f service=api
gh workflow run deploy-v2.yml --ref main -f service=api -f tag=<API TAG> -f cluster=geo-testnet-k8s
```

Why (e) comes after (c): the api's init container writes its binding from `node-id` in ConfigMap
`geodb-space-map`, which the feeder publishes on its first start. If the api rolled out first it
would start without a binding and answer every card from Postgres. Harmless, but fix it by
`$K rollout restart deployment/api` once the ConfigMap has `node-id`.

Check the replica is up and filling:

```bash
$K get pod geodb-0                        # 2/2 Running
$K get cm geodb-space-map -o jsonpath='{.data.node-id}{"\n"}'   # 64 hex
$K logs geodb-0 -c feeder --tail=20       # JSON lines: "founded space", "applied"
$K exec geodb-0 -c feeder -- wget -qO- localhost:9090/health/readiness
$K port-forward pod/geodb-0 9090 &  curl -s localhost:9090/metrics | grep -E 'consumer_lag|spaces|dead_letters'
```

The first start founds one geo-db Space per gaia Space and each founding restarts serve (about 1 s on
the dedicated volume), so a long topic with many Spaces takes a while before the lag falls. That is
normal; `geodb_feeder_spaces` rises, then `geodb_feeder_consumer_lag` falls.

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

## 3. The exit check: a week of traffic

Do not turn serve mode on until all of these hold over 7 days of shadow:

- `failed` is about 0: `sum(increase(gaia_api_geodb_card_shadow_total{result="failed"}[7d])) / sum(increase(gaia_api_geodb_card_shadow_total[7d])) < 0.001`.
- Differences are low and steady: `(only_postgres + only_geodb + both) / total` stays under 1%, and any bump
  matches a known cause. Known steady causes: edits older than the topic's retention window
  (`geodb_feeder_translate_total{counter="entity-predates-window"}`), and the translator's documented
  mapping gaps (PR #1054). Find specific cards from the api's shadow log lines.
- The replica keeps up: `geodb_feeder_consumer_lag` returns to near 0 after bursts, and
  `geodb_feeder_dead_letters_total` is flat or every dead letter is explained
  (`$K exec geodb-0 -c feeder -- cat /data/feeder/errors.jsonl`).
- p99 `gaia_api_geodb_read_seconds` is well under the 500 ms budget (`GEODB_TIMEOUT_MS`).
- No `GeoDbFeederDown`, `GeoDbFeederStalled`, `ApiGeoDbCardShadowFailing` alert in the window.

The topic may not hold full history. If retention is shorter than the age of the data, the replica only
knows entities touched within it, and cards for older entities answer `only_postgres`. That is a coverage
limit, not a bug, and it is why `only_postgres` has a floor above zero. Decide whether that floor is
acceptable before serve mode; backfilling older state is outside stage 1.

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
StatefulSet, the PVC `data-geodb-0`, and the four Secrets and the `geodb-space-map` ConfigMap.

## 6. Alerts (monitoring/k8s/v2/geodb-feeder-alerts.yaml)

| alert | means | first look |
| --- | --- | --- |
| `GeoDbFeederDown` | no feeder series, serve down, or consumer down for 10m | `$K get pod geodb-0`; `logs -c feeder`, `logs -c serve`; Kafka credentials and ACL |
| `GeoDbFeederStalled` | lag above 0 and nothing applied for 15m | the repeating error in the feeder log (`transient` = serve unreachable, `exhausted` = serve restarting) |
| `GeoDbFeederLagHigh` | more than 5000 behind for 30m | expected once on first catch-up; otherwise throughput or a stall |
| `GeoDbFeederDeadLetters` | an edit or intent was skipped | `errors.jsonl` on the volume (see above) |

## 7. Operating notes

- Crash recovery: the feeder resumes from its stored offset (`/data/feeder/offsets.json`), and an
  interrupted message is replayed from the intent it stopped at. Applies are idempotent, so a replay is
  safe. Kafka's own group offset is only used to report lag.
- Replaying from scratch: scale to 0, delete `data-geodb-0`, scale to 1. Space ids come out the same
  because the store secret and the founding nonces are deterministic.
- New Spaces become readable to the api within the grant lifetime (`GEODB_GRANT_TTL_S`, 300 s) and the
  space-map refresh (10 s).
- A serve restart for founding drops the feeder's connection for about a second; the feeder retries.
- Capacity: `spaces = 256` in `geodb-serve-config` is geo-db's Space capacity. The shadow droplet holds 79
  gaia Spaces on testnet (2026-11-03), so there is headroom. If the feeder log shows founding refused
  for capacity, `GeoDbFeederStalled` fires; raise `spaces`, redeploy, and the feeder resumes.
  The space-map ConfigMap (1 MiB limit) holds roughly 15,000 Spaces, so it is not the constraint.
- Upgrading geo-db: build a new image tag, change the pinned tag in `geodb/k8s/v2/geodb.yaml` and
  `api/k8s/v2/api.yaml` together, and redeploy. Read the geo-db release notes for store-format changes first.
