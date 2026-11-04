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
  docker run --rm -v "$PWD:/k" -w /k registry.digitalocean.com/geo/geodb:289618ffbf \
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
# a. geodb image (serve + client), pinned at geo-db final 289618ffbf. ~1 h of compile.
gh workflow run build-geodb-image.yml --ref main -f geodb_ref=289618ffbf
# b. feeder image
gh workflow run build-v2-images.yml --ref main -f service=geodb-feeder
# c. the replica: StatefulSet geodb, Deployment geodb-feeder, Services, RBAC, serve config. TAG = the 8-char sha of the run in (b).
gh workflow run deploy-v2.yml --ref main -f service=geodb-feeder -f tag=<TAG> -f cluster=geo-testnet-k8s
$K rollout status statefulset/geodb && $K rollout status deployment/geodb-feeder
# d. alerts
kubectl --context do-nyc2-geo-testnet-k8s apply -f monitoring/k8s/v2/geodb-feeder-alerts.yaml \
  -f monitoring/k8s/v2/hermes-metrics-servicemonitor.yaml
# e. api, last: it carries the init container that reads the node id, and it needs geodb:289618ffbf to
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
  `api/k8s/v2/api.yaml` together, and redeploy. Read the geo-db release notes for store-format changes first. A change of the store's persistent revision needs §8 first.

## 8. Migrating the store across a persistent cut

geo-db's store carries a persistent revision (`/data/store/FORMAT`, line `persistent <n> <fingerprint>`). A serve built at
a different revision refuses to open the store; it does not touch it. The store is moved to the new revision by
`geodb-migrate`, which replays each Space's op log into a fresh root and checks every version's seal against the
recorded one. This applies to any geodb image at or past geo-db `b8211a3fe5` (the apostrophe fold, DR-167-318, revision
3 to 4), so also to gaia#1063, which needs an image at or past `7d32584825`. Images `289618ffbf` to `4d3b31dff4` are
revision 3 and do not need it.

The image carries the migrator since `build-geodb-image.yml` installs it as `/usr/local/bin/geodb-migrate`. Older
tags (`289618ffbf`, `4d3b31dff4`) have only serve and client. Rehearsed 2026-11-04 on a copy of the live store with the
image `geodb:7d32584825` (built from the branch that added the migrator).

Measured (2026-11-04, testnet store of 4.7 GiB: 100 Spaces, 57,857 versions, after the full backfill):

| step | result |
| --- | --- |
| volume snapshot of `data-geodb-0` (50 Gi, live) to ready | under 20 s |
| restore into a new PVC and a pod running | 14 s |
| old serve opens the crash-consistent copy, then SIGTERM | opens in 5 s, `closed: the final checkpoint written` |
| `geodb-migrate`, revision 3 to 4 (single thread, no CPU limit) | **1094 s (18 min 14 s)**, exit 0, `DONE migrate spaces=100 versions=57857`, every seal equal |
| migrator peak memory | 606 MiB (`VmHWM` 620,860 kB) |
| new store size | 4.7 GiB (same as the old) |
| new serve opens the migrated store | 2 s, 551 MiB resident at start; same `node` and `sealer` ids as the live serve, so the api's binding stays valid |
| new serve on an unmigrated store | `DefiniteRefusal: the store is at persistent revision 3 ... migrate it`, exit 1, store unchanged |
| 20 `entity` reads (`geodb-client pipe`, the api's reader key and card query) | the migrated copy answered byte-for-byte what live geodb-0 answered (20 of 20 frames, 13,731 bytes) |

Time scales with versions, about 53 a second here. Expect downtime of the migration plus about 5 minutes (scale-down,
Job start, image pull, serve start): roughly 25 minutes on this store, with the feeder paused the whole time (Kafka keeps
the edits; the feeder replays from its stored offset). The api sees geo-db down: in `shadow` that is `failed` results
only, in `on` every card falls back to Postgres.

A snapshot of a running serve is crash-consistent, and `geodb-migrate` refuses a store whose segment "ends in an
unsealed frame". To migrate (or rehearse on) a copy taken while serve was running, open the copy once with the old
serve under `geodb-supervisor` and stop it with SIGTERM (it writes its final checkpoint), then migrate. The procedure
below quiesces serve first, so it does not need that.

### 8a. Procedure

Order against deploy-v2: the pin PR can merge any time (merging deploys nothing). Do steps 1 to 6 before the
`deploy-v2` dispatch in step 7. Deploying the new tag first is not harmful but is an outage: geodb-0 crash-loops on
the refusal above, readiness stays false, and the store is unchanged; scale it to 0 and carry on from step 3.

```bash
K="kubectl --context do-nyc2-geo-testnet-k8s -n gaia"
NEW=<10-char geodb tag, at or past 7d32584825>
# 0. image with serve, client and migrate; the tag must exist and carry the migrator.
gh workflow run build-geodb-image.yml --ref main -f geodb_ref=$NEW        # ~8 min
doctl registry repository list-tags geodb --registry geo | grep $NEW
# 1. PR: change the pin to $NEW in geodb/k8s/v2/geodb.yaml (both places: serve and the feeder's geodb-setup init
#    container) and api/k8s/v2/api.yaml. Merge it; nothing deploys.
# 2. before-picture: 20 entity reads from live (a throwaway pod, §8c), saved as before.out. Silence GeoDbFeederDown,
#    GeoDbFeederStalled, GeoDbFeederLagHigh and ApiGeoDbCardShadowFailing for 1 h.
# 3. quiesce: feeder first, then serve (SIGTERM, the supervisor forwards SIGINT, serve writes its final checkpoint).
$K scale deployment/geodb-feeder --replicas=0 && $K wait --for=delete pod -l app=geodb-feeder --timeout=120s
$K logs -f geodb-0 > /tmp/geodb-0-shutdown.log 2>&1 &
$K scale statefulset/geodb --replicas=0 && $K wait --for=delete pod/geodb-0 --timeout=330s
grep 'closed: the final checkpoint written' /tmp/geodb-0-shutdown.log      # must print; if not, stop and look
# 4. rollback point: snapshots of both volumes while nothing writes. Keep them until step 9.
for pvc in data-geodb-0 geodb-feeder-state; do
  cat <<EOF | $K apply -f -
apiVersion: snapshot.storage.k8s.io/v1
kind: VolumeSnapshot
metadata: {name: premigrate-rev3-$pvc, namespace: gaia}
spec: {volumeSnapshotClassName: do-block-storage, source: {persistentVolumeClaimName: $pvc}}
EOF
done
$K get volumesnapshot            # both READYTOUSE true before going on
```

5. Migrate in a Job on `data-geodb-0`. It checks the store is at revision 3, writes `store-rev4` next to `store`, checks
   the migrator's `DONE` line and that the new FORMAT says revision 4, and only then swaps the directories. The old
   store stays on the volume as `store-rev3` (4.7 GiB of the 50 Gi). Nothing is deleted. A failed run leaves `store`
   untouched; delete the Job and apply it again (it clears `store-rev4` first).

```bash
cat <<EOF | $K apply -f -
apiVersion: batch/v1
kind: Job
metadata: {name: geodb-migrate, namespace: gaia}
spec:
  backoffLimit: 0
  ttlSecondsAfterFinished: 86400
  template:
    spec:
      restartPolicy: Never
      enableServiceLinks: false
      automountServiceAccountToken: false
      securityContext: {runAsNonRoot: true, runAsUser: 1001, runAsGroup: 1001, fsGroup: 1001}
      imagePullSecrets: [{name: geo}]
      volumes:
        - {name: data, persistentVolumeClaim: {claimName: data-geodb-0}}
        - {name: serve-config, configMap: {name: geodb-serve-config}}
        - {name: store-secret, secret: {secretName: geodb-store-secret, defaultMode: 0440}}
      containers:
        - name: migrate
          image: registry.digitalocean.com/geo/geodb:$NEW
          command:
            - sh
            - -c
            - |
              set -eu
              cd /data
              [ ! -e store-rev3 ] || { echo "store-rev3 exists: an earlier migration was not cleaned up"; exit 1; }
              grep -q '^persistent 3 ' store/FORMAT
              rm -rf store-rev4
              geodb-migrate --from /data/store --to /data/store-rev4 \
                --secret-file /etc/geodb/store-secret/secret.hex --config /etc/geodb/serve.cfg > migrate.log 2>&1 \
                || { tail -n 5 migrate.log; exit 1; }
              tail -n 2 migrate.log
              n=\$(ls store/ops | wc -l)
              grep -q "^DONE migrate spaces=\$((n)) " migrate.log
              grep -q '^persistent 4 ' store-rev4/FORMAT
              mv store store-rev3 && mv store-rev4 store
              echo swapped
          volumeMounts:
            - {name: data, mountPath: /data}
            - {name: serve-config, mountPath: /etc/geodb/serve.cfg, subPath: serve.cfg, readOnly: true}
            - {name: store-secret, mountPath: /etc/geodb/store-secret, readOnly: true}
          resources:
            requests: {memory: 1Gi, cpu: "1"}
            limits: {memory: 2Gi}
          securityContext: {allowPrivilegeEscalation: false, capabilities: {drop: [ALL]}}
EOF
$K wait --for=condition=complete job/geodb-migrate --timeout=45m     # ~18 min here; progress: $K exec job/geodb-migrate -- wc -l /data/migrate.log
$K logs job/geodb-migrate                                              # DONE migrate spaces=<n> versions=<m>, then "swapped"
```

6. Check the Job log: `spaces` equals the number of Spaces before (100 on 2026-11-04) and no line says a seal differs.
7. Bring both back on the new image. `deploy-v2` applies `geodb.yaml`, whose StatefulSet and feeder
   Deployment are `replicas: 1`, so this also scales them back up; geodb-0 opens the migrated store.

```bash
gh workflow run deploy-v2.yml --ref main -f service=geodb-feeder -f tag=<FEEDER TAG> -f cluster=geo-testnet-k8s
$K rollout status statefulset/geodb && $K rollout status deployment/geodb-feeder
$K get cm geodb-space-map -o jsonpath='{.data.node-id}{"\n"}{.data.sealer-id}{"\n"}'   # unchanged from before step 3
$K logs geodb-0 | head -5                                                              # listen / node / sealer / serving
# api last, as in §1 (e), so its init container pulls the same client: gaia api deploy.
```

8. Verify: the same 20 reads as step 2 (§8c) give `after.out`; `cmp before.out after.out` (an entity the feeder
   edited since differs legitimately, so use entities that are quiet). Then watch `geodb_feeder_consumer_lag` fall
   and the shadow `match` share return to its earlier level (§2).
9. After a day on the new revision, delete the rollback points: `$K delete volumesnapshot premigrate-rev3-data-geodb-0
   premigrate-rev3-geodb-feeder-state` and remove `store-rev3` from the volume with a Job like step 5 whose script is
   `rm -rf /data/store-rev3 /data/migrate.log`, with serve scaled to 0 for it (the volume is ReadWriteOnce). Resume the alerts.

### 8b. Rollback

Keep the snapshots from step 4 and the `store-rev3` directory until step 9.

- Before the feeder has run on the migrated store (steps 5 to 7 done, nothing applied since): scale both to 0 as in
  step 3, swap back in a one-shot Job or pod on `data-geodb-0` (`mv store store-rev4-discarded && mv store-rev3 store`),
  revert the pin PR (or `deploy-v2` from the previous commit), and bring the feeder up. The old serve opens the
  untouched revision 3 store.
- After the feeder has applied edits on the migrated store: `store-rev3` is behind the feeder's stored offset, so restore
  both volumes together. Scale both to 0; delete PVCs `data-geodb-0` and `geodb-feeder-state`; create each again with the
  same name, size and class and `dataSource: {apiGroup: snapshot.storage.k8s.io, kind: VolumeSnapshot, name:
  premigrate-rev3-<pvc>}`; revert the pin; `deploy-v2`. The feeder resumes at its snapshotted offset, so the topic's
  retention must still cover it (check the offset in `/data/feeder/offsets.json` against the topic before deleting
  anything). The api binding needs no change: node and sealer ids do not move.

### 8c. The read check

A throwaway pod that reads with the api's own key and query, so nothing writes. Use the old image for live geodb-0
(revision 3) and `$NEW` against the migrated one; the wire grammars are the same (`geodb-client check` prints `same`).

```bash
# 20 (gaia space, entity) pairs spread over the feeder's journal, mapped to geo-db Spaces by the space map
$K exec deploy/geodb-feeder -c feeder -- sh -c 'n=$(wc -l < /data/feeder/known-entities.log); \
  awk -v n=$n "BEGIN{s=int(n/20)} NR%s==1 && c<20 {print; c++}" /data/feeder/known-entities.log' > sample.txt
$K get cm geodb-space-map -o jsonpath='{.data.space-map\.json}' > spacemap.json
python3 - <<'EOF' > reads.tsv
import json
m = json.load(open("spacemap.json"))
for line in open("sample.txt"):
    g, e = line.strip().split("|")
    print(f'{m[g]["geo"]}\t.{{ .version = 1, .select = .{{ .c = .{{ .field = "entity", .args = .{{ .id = "{e}" }} }} }} }}')
EOF
# a pod with the reader key (fsGroup so the 0440 key file is readable) and the image under test; then
#   printf 'version = 1\nlisten = tcp:<geodb Service ClusterIP>:7447\nnode = <node-id from the ConfigMap>\n' > /tmp/binding.txt
#   geodb-client --key /etc/geodb/reader-key/key --binding /tmp/binding.txt pipe < reads.tsv > out
# every request answers `+<n>` and its rows; `-<n>` is a refusal.
```
