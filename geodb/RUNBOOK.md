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
  docker run --rm -v "$PWD:/k" -w /k registry.digitalocean.com/geo/geodb:7d32584825 \
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
# a. geodb image (serve + client), pinned at geo-db final 4d3b31dff4. ~1 h of compile.
gh workflow run build-geodb-image.yml --ref main -f geodb_ref=4d3b31dff4
# b. feeder image
gh workflow run build-v2-images.yml --ref main -f service=geodb-feeder
# c. the replica: StatefulSet geodb, Deployment geodb-feeder, Services, RBAC, serve config. TAG = the 8-char sha of the run in (b).
gh workflow run deploy-v2.yml --ref main -f service=geodb-feeder -f tag=<TAG> -f cluster=geo-testnet-k8s
$K rollout status statefulset/geodb && $K rollout status deployment/geodb-feeder
# d. alerts
kubectl --context do-nyc2-geo-testnet-k8s apply -f monitoring/k8s/v2/geodb-feeder-alerts.yaml \
  -f monitoring/k8s/v2/hermes-metrics-servicemonitor.yaml
# e. api, last: it carries the init container that reads the node id, and it needs the pinned geodb image to
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

### 2b-ter. Held-repair (values the old backfill skipped)

Before gaia#1244, the backfill's `held.props` marked a property held when geo-db had any card row on it, and a
relation's type is a card row's property too. A value sharing its property with a relation type of the same entity in
the same Space (a text `""` plus relations typed by that property) was skipped for good: about 1,562 Postgres value
rows (257 empty texts, 96 properties) never reached geo-db. A Space backfill rerun is the wrong repair (its cursor skips
finished batches, and it needs the feeder at 0 for hours). Job `geodb-held-repair`
(`geodb/k8s/v2/geodb-held-repair-job.yaml`, same image, `node dist/backfill-main.js` with `BACKFILL_MODE=held-repair`)
visits only the (Space, entity) pairs with that shape, reads geo-db for each, and writes only the values geo-db lacks,
through the feeder's translate + apply path and journals (`geodb-feeder/src/heldRepair.ts`). It never overwrites a row,
writes no relation, creates no entity (one geo-db lacks is left to the Space backfill) and founds no Space. Its cursor is
`/data/feeder/held-repair/`, apart from the Space backfill's. Logs carry counts only, never ids or text.

- `DRY_RUN=1`: reads Postgres and geo-db, logs counts, writes nothing (no geo-db, journal, cursor or space-map write) and
  skips the Kafka check, so it needs no stopped feeder. Final line `held-repair done`: `candidatePairs`, `entitiesToRepair`,
  `valuesToWrite` (the number a real run writes), `valuesAlreadyHeld`, `entitiesAbsent`, `spacesUnmapped`.
- `DRY_RUN=0`: the same, and writes. Needs the feeder at 0 and the ordering precondition (as §2b). Knobs:
  `BACKFILL_BATCH` (100), `BACKFILL_PAUSE_MS` (100), `BACKFILL_MAX_ENTITIES` (0: all; the batch cap, for a staged run).
- Idempotent: a value written is a geo-db row, so a rerun (or one with the cursor deleted) finds the property held and
  writes nothing: `valuesToWrite` 0.

```bash
TAG=<the feeder tag deployed now, built from a main that contains this mode>
# dry run: the feeder may keep running. Expect valuesToWrite close to 1,562 less what geo-db already got.
sed -e "s|geodb-feeder:latest|geodb-feeder:$TAG|" -e "s|__DRY_RUN__|1|" \
  geodb/k8s/v2/geodb-held-repair-job.yaml | $K apply -f -
$K logs -f job/geodb-held-repair        # "held-repair plan", then "held-repair done"
$K delete job geodb-held-repair
# real run: silence GeoDbFeederDown/LagHigh, stop the feeder, wait for its group to empty (as §2b steps 0-1).
$K scale deployment/geodb-feeder --replicas=0
$K wait --for=delete pod -l app=geodb-feeder --timeout=120s
# optional staged first run: add -e '/BACKFILL_MAX_ENTITIES/{n;s|"0"|"200"|;}' to the sed.
sed -e "s|geodb-feeder:latest|geodb-feeder:$TAG|" -e "s|__DRY_RUN__|0|" \
  geodb/k8s/v2/geodb-held-repair-job.yaml | $K apply -f -
$K logs -f job/geodb-held-repair        # "ordering precondition" ok:true ... "held-repair done" valuesWritten, deadLettered 0
$K delete job geodb-held-repair
$K scale deployment/geodb-feeder --replicas=1
$K rollout status deployment/geodb-feeder
# verify: rerun with __DRY_RUN__ -> 1 (feeder may run): valuesToWrite 0.
```

If the Job fails, read why, delete it and apply it again: it resumes. Giving up is just scaling the feeder back to 1.

### 2b-bis. Proposal system entities

The kg-indexer writes a system entity per proposal (type Proposal, five values, two System/Proposal type
relations) straight to Postgres on PROPOSAL_CREATED; no edit carries it. With `GEODB_PROPOSAL_ENTITIES=1` the
feeder consumes `<env>.space.governance` in its own group (`geodb-feeder-proposals-v1-<env>`, own offsets under
`feeder/proposals`) and writes the same rows, only into Spaces geo-db already holds (it never founds one).
Metrics: `geodb_feeder_proposal_events_total{outcome}` (`unmapped` = Space not founded), `geodb_feeder_proposals_consumer_lag`,
`geodb_feeder_proposals_consumer_running`. The Kafka user needs consume on `space.governance`. Proposals created
before the consumer first ran, or in a Space founded later, are loaded by the backfill (`BACKFILL_SPACES=<ids>`).

### 2b-ter. Space system entities

The kg-indexer writes a system entity per registered Space (id = the Space id, four values, System/Space/EoaSpace-or-DaoSpace
relations) straight to Postgres on `space.creations`; no edit carries it. With `GEODB_SPACE_ENTITIES=1` the feeder
consumes `<env>.space.creations` in its own group (`geodb-feeder-space-creations-v1-<env>`, own offsets under
`feeder/space-entities`, from the earliest offset when it has none) and writes the same rows into the Space they describe.
It never founds a Space: an entity for a Space geo-db does not hold yet is parked in `feeder/space-entities/pending.json`
and applied by a sweep (every lag poll) once an edit has founded the Space. Metrics:
`geodb_feeder_space_entity_events_total{outcome}`, `geodb_feeder_space_entities_pending`,
`geodb_feeder_space_entities_consumer_lag`, `geodb_feeder_space_entities_consumer_running`. The Kafka user needs consume on
`space.creations`. The topic keeps only a short window (testnet 2026-11-04: about two dozen messages), so the consumer
replays only that; it is not a backfill. Spaces the backfill loaded already hold the entity (its rows carry the Space's own
`space_id`), so those are no-ops. A Space founded later by an edit whose creation message has expired gets its entity from a
backfill of that Space (`BACKFILL_SPACES=<id>`, §2b), not from this consumer.

### 2c. Membership backfill (before GEODB_MEMBERSHIP_DUAL_WRITE)

Space members and editors reach geo-db only through the `space.membership` consumer, which is off
(`GEODB_MEMBERSHIP_DUAL_WRITE`, `GEODB_MEMBERSHIP_SHADOW_COMPARE`; the feeder reads `1` or `true` as on, anything else as off).
The topic does not hold full history, so load the current state first. Job `geodb-membership-backfill`
(`geodb/k8s/v2/geodb-membership-backfill-job.yaml`, `node dist/membership-backfill-main.js`) reads every row of
`members` and `editors` (read only) and applies each as a grant through the dual-write path, recording it in the
membership journal. Rows the journal holds are no-ops, so a rerun resumes. It never retracts. Like §2b it runs
with the feeder scaled to 0 and refuses to start while any feeder consumer group has members. Its init container
takes the geodb image tag from `geodb.yaml` at apply time (the `GEODB_TAG` placeholder), so its geodb-client matches
the running serve; with `GEODB_RECORD_KEYS` that serve must be 9d97069a78 or later (§2c-bis).

```bash
$K scale deployment/geodb-feeder --replicas=0
$K wait --for=delete pod -l app=geodb-feeder --timeout=120s
GEODB_TAG=$(awk '/^ *- name: serve$/{s=1} s && /image: registry.digitalocean.com\/geo\/geodb:/{sub(/.*geodb:/,""); print; exit}' geodb/k8s/v2/geodb.yaml)
sed -e "s|geodb-feeder:latest|geodb-feeder:$TAG|" -e "s|geodb:GEODB_TAG|geodb:$GEODB_TAG|" \
  geodb/k8s/v2/geodb-membership-backfill-job.yaml | $K apply -f -
$K logs -f job/geodb-membership-backfill   # "membership backfill done", deadLetter 0
$K delete job geodb-membership-backfill
# then GEODB_MEMBERSHIP_DUAL_WRITE "1" in the feeder manifest, scale to 1; once lag is ~0, SHADOW_COMPARE "1".
```

The comparison checks the full id sets per Space and role; a mismatch logs the differing ids (5 per side, 10
Spaces per run) and sets `geodb_feeder_membership_shadow_diff_ids{role,side}`.

### 2c-bis. Keyed membership records (GEODB_RECORD_KEYS)

`GEODB_RECORD_KEYS` (default off) moves membership from the journal-planned dual write to natural-key records
(geo-db plan 10.12; `geodb-feeder/src/membershipKeyed.ts`). Every (Space, role, member) row becomes one edge
statement under `recordStatementId(membership kind, [space entity, role property, member entity])` in the
`governance` system Space; a grant is `create_entity` + `add` + `set_active(true)`, a revoke `set_active(false)`.
The intents depend on the event alone, so there is no journal and a replay from offset zero converges. It also
allows the `put` intent. Activation order, each step only after the one before it is live:

1. geo-db serve and the geodb-client init container on 9d97069a78 or later (`put`, and a retract of an unseen id;
   §1). A serve before that refuses `put` as an invalid wire payload.
2. A feeder image that carries this path (it is inert while the flag is off).
3. Required: the backfill Job of §2c with `GEODB_RECORD_KEYS: "1"` added to its env, which writes the Postgres
   snapshot as keyed grants (rerunnable; it never retracts). `testnet.space.membership` keeps 7 days
   (`retention_ms` 604800000, cleanup `delete`; `doctl databases topics get <cluster id> testnet.space.membership`),
   so the consumer's replay from the start only covers that window, and every older grant comes from this snapshot.
   Run it before step 4, while the keyed consumer has never run: the Job refuses once
   `/data/feeder/membership-keyed/offsets.json` exists, because a grant-only snapshot taken after the consumer has
   applied a revoke would grant that role again, and nothing would replay the revoke. To run it later anyway, add
   `BACKFILL_KEYED_RESET_CONSUMER: "1"`: the Job deletes those offsets, and the keyed consumer resets its group to
   earliest on its next start and replays the retained topic over the snapshot, so every key the window touched ends
   at its last event. A ResourceExhausted while founding the governance Space fails the Job; nothing is skipped.
4. `GEODB_RECORD_KEYS: "1"` beside `GEODB_MEMBERSHIP_DUAL_WRITE: "1"` in the feeder manifest. The keyed consumer
   runs instead of the legacy one, on its own group (`geodb-feeder-membership-keyed-v1-<env>`, offsets in
   `/data/feeder/membership-keyed`), and the shadow compare reads the keyed rows. The legacy rows stay in each
   Space's own geo-db Space, untouched and no longer written.

The first keyed write founds the `governance` Space (one Space against the cap of §2d; three at most, one per
family: governance, votes, config). Its nonce is fixed by the family name
(`recordEntityId(SYSTEM_SPACE_NONCE_KIND, ["gaia.system-space.governance"])`), its id is serve's
`foundingSpaceId(sealer, nonce)`, and it is recorded in `/data/feeder/system-spaces.json` and published to the
`geodb-space-map` ConfigMap under `system-spaces.json`, beside `space-map.json` (the publisher keeps its own copy of
what it last sent in `/data/feeder/system-spaces.published.json`; the record is `system-spaces.json`):

```bash
$K get configmap geodb-space-map -o jsonpath='{.data.system-spaces\.json}'
```

Unlike a gaia Space (§7, `space_cap`), a system Space is never skipped: every row of its family lives there, so
dead-lettering would drop the whole family. If founding answers ResourceExhausted (the cap of §2d, or a serve
limit; the two look alike on the wire), the keyed consumer retries the message every 5 s without committing it,
the partition stalls and `GeoDbFeederMembershipStalled` fires after 30 min (`GeoDbFeederStalled` watches the
edits consumer only); it resumes by itself once founding succeeds. Raise the cap
(§2d) if `founded` is at `spaces`. `geodb_feeder_space_map_publish_total{key="system-spaces.json"}` counts the
system map's publishes apart from the gaia map's.

Rollback: unset `GEODB_RECORD_KEYS`. The legacy consumer resumes from its own offsets; the keyed rows stay in
the governance Space, unread.

### 2c-ter. Keyed user votes (GEODB_VOTES)

`GEODB_VOTES` (default off; it takes effect only with `GEODB_RECORD_KEYS`) writes gaia's `user_votes` as natural-key
records in the `votes` system Space (`geodb-feeder/src/votesKeyed.ts`). The feeder consumes `<env>.curation.votes`
(HermesVoteCast) on its own group, `geodb-feeder-votes-keyed-v1-<env>`; vote-indexer's group is untouched, and it
still writes Postgres, which the api still reads. One record per `user_votes` primary key:

- record entity `recordEntityId(userVote kind, [user, object, object_type, space, vote_kind])`, over the raw gaia ids;
  `vote_kind` is an i16 and is keyed as its u16 bit pattern, so a negative kind (vote-indexer stores `kind as i16`)
  gets its own record like any other;
- key columns (voter, object, Space as references; object type and kind as numbers), each `add` +
  `set_active(true)` under `recordStatementId(userVote kind, [record, column, target])`. The object references
  `cid("entity", id)` for an entity and `cid("statement", id)` for a relation (geo-db holds a relation as a statement).
  UNCONFIRMED: neither hermes-pipeline nor vote-indexer says whether a relation vote's `object_id` is the relation's
  own id (assumed) or its reified entity id, and no client sends relation votes yet; settle it before anything reads
  that column (the record key uses the raw id either way);
- mutable columns `direction` (0 up, 1 down, 2 removed) and `voted_at` (instant, whole seconds) as `put`, under
  `recordStatementId(userVote kind, [record, column, value])`: a put closes the cell's other values, so a changed
  vote replaces the old one, and a replay of the current value is a no-op. A removal keeps the record with
  direction 2, as vote-indexer keeps the row. Column property ids are pinned in `recordKinds.ts` (`RECORD_COLUMNS`).

The feeder rejects (dead-letters, never writes) exactly what vote-indexer rejects: no metadata, an id that is not
16 bytes, an object type other than entity/relation, an unknown direction, Interested + Down. An unknown kind keeps
its raw value (narrowed to i16) as its own key. Like vote-indexer it decodes every message on the topic, whatever its
`event-type` header.

A permanent refusal of a single intent inside a vote (geo-db refusing one statement for good) is dead-lettered on its
own (`GeoDbFeederDeadLetters`) and the vote's other intents are written, so that record is left partial, for
example missing a key column. Replaying the same event is refused again, so it does not repair itself: fix the
cause, then rerun the votes backfill (step 4, with `BACKFILL_KEYED_RESET_CONSUMER: "1"` once the consumer has run).
The same holds for keyed membership (§2c-bis).

Activation order, each step only after the one before it is live:

1. geo-db serve and the init container on 9d97069a78 or later (`put`; §1), as for §2c-bis. The backfill Job's init
   container takes `geodb.yaml`'s tag at apply time, so it is only as new as that pin: `db33c7e6e1` (the pin now) carries
   `put`; a pin older than `9d97069a78` would dead-letter every row.
2. A feeder image that carries this path (inert while either flag is off).
3. Kafka ACL (not yet checked; reading the user's ACLs means reading its credentials, so whoever holds them does it):
   the feeder's Kafka user needs consume on `<env>.curation.votes` and on the group
   `geodb-feeder-votes-keyed-v1-<env>`, and the backfill Job needs describe on every feeder group for its
   empty-group check. Without them the votes consumer fails to start: the edits consumer is unaffected, the error is
   logged, and `geodb_feeder_consumer_start_failed{consumer="votes"}` goes to 1 (`GeoDbFeederConsumerStartFailed`).
4. Required: the votes backfill Job (`geodb/k8s/v2/geodb-votes-backfill-job.yaml`), with the feeder scaled to 0. It
   reads `user_votes` READ ONLY, a page of 5000 at a time in primary-key order, and writes every row as its record
   (rerunnable: the same ids again). `testnet.curation.votes` keeps 7 days (`retention_ms` 604800000, cleanup
   `delete`), so every older vote comes from this snapshot. Run it while the votes consumer has never run: the Job
   refuses once `/data/feeder/votes-keyed/offsets.json` exists, because a snapshot row read after the consumer has
   applied a later vote would put the older value back with nothing to replay the newer one. To run it later anyway,
   add `BACKFILL_KEYED_RESET_CONSUMER: "1"`: the Job deletes those offsets and the consumer replays the retained topic
   from earliest over the snapshot, in order (the topic is keyed by object id, and every key includes it), so every
   key the window touched ends at its last event. A row Postgres holds that no event could have written is
   dead-lettered and fails the Job at the end; a ResourceExhausted founding the votes Space fails it at once.

   ```bash
   $K scale deploy/geodb-feeder --replicas=0
   GEODB_TAG=$(awk '/^ *- name: serve$/{s=1} s && /image: registry.digitalocean.com\/geo\/geodb:/{sub(/.*geodb:/,""); print; exit}' geodb/k8s/v2/geodb.yaml)
   sed -e "s|geodb-feeder:latest|geodb-feeder:$TAG|" -e "s|geodb:GEODB_TAG|geodb:$GEODB_TAG|" \
     geodb/k8s/v2/geodb-votes-backfill-job.yaml | $K apply -f -
   $K logs -f job/geodb-votes-backfill
   ```

5. `GEODB_RECORD_KEYS: "1"` and `GEODB_VOTES: "1"` in the feeder manifest, then scale it back to 1. The consumer
   resets its new group to earliest and replays the retained week over the snapshot. Turning on `GEODB_RECORD_KEYS`
   for membership alone does not start it.

The first vote founds the `votes` Space (nonce `744a1a2a52d0432199372b395a4057b7`, published under
`system-spaces.json` as for governance in §2c-bis). A ResourceExhausted founding it, or a failing message of any
kind, is retried every 5 s without committing: the partition stalls and `GeoDbFeederVotesStalled` fires after 30
min. Metrics: `geodb_feeder_vote_events_total{outcome}`, `geodb_feeder_votes_consumer_lag`,
`geodb_feeder_votes_consumer_running`, `geodb_feeder_vote_freshness_seconds`.

Not migrated by this path (gaps): the `votes` log (append-only history; geo-db's statement history of each record
carries the changes from the switch on), `votes_count`, the curation score values and ranking scores (all derived
from `user_votes`). `proposal_votes` is §2c-quater.

Rollback: unset `GEODB_VOTES`. The records stay in the votes Space, unread; nothing reads them yet.

### 2c-quater. Keyed proposal votes (GEODB_PROPOSAL_VOTES)

`GEODB_PROPOSAL_VOTES` (default off; it takes effect only with `GEODB_RECORD_KEYS`) writes gaia's `proposal_votes` as
natural-key records in the same `votes` system Space as §2c-ter (`geodb-feeder/src/proposalVotesKeyed.ts`; the
pinned `proposalVote` kind belongs to the votes family). It is its own flag, not `GEODB_VOTES`, because its backfill
must run before its consumer: turning on `GEODB_VOTES` after the user-votes backfill must not start a proposal-votes
consumer whose snapshot was never taken. The feeder consumes `<env>.space.governance` on its own group,
`geodb-feeder-proposal-votes-keyed-v1-<env>`, and handles only `event-type: PROPOSAL_VOTED` (as kg-indexer routes);
every other event on the topic counts as `ignored`. kg-indexer's group is untouched, it still writes Postgres, and the
api still reads Postgres. One record per `proposal_votes` primary key:

- record entity `recordEntityId(proposalVote kind, [proposal, proposal_version, voter])`, over the raw 16-byte ids.
  The version is kg-indexer's: the event's u32, 0 read as 1, stored as i32 and keyed as its u32 bit pattern;
- key columns (proposal, voter, Space as references to `cid("entity", id)`; version as a number), each `add` +
  `set_active(true)`. The Space is written once, as Postgres writes it once (its upsert does not update `space_id`);
- mutable columns `vote` (1 Yes, 2 No, 3 Abstain; a missing or unknown vote is Abstain, as kg-indexer maps it),
  `created_at` (whole seconds, as an instant) and `created_at_block`, as `put`: a changed vote replaces the old one, a
  replay of the current value is a no-op. Column property ids are pinned in `recordKinds.ts`.

The feeder dead-letters (never writes) what kg-indexer rejects: an id that is not 16 bytes, or a message that does not
decode. Missing metadata gives created_at 0 and block 0, as kg-indexer does. The backfill dead-letters a row no event
could have produced (an unknown vote, version 0, a value outside i64).

Known difference, in either direction: kg-indexer drops the WHOLE block from Postgres when any write in it fails
permanently, on any topic (`proposal_votes_version_fk`, checked at commit, is one cause; any other row in the block
is another). The feeder writes each vote on its own, so geo-db can hold a vote Postgres lacks, or a newer value for a
key than Postgres holds. Drift between `proposal_votes` and the records is expected, to be investigated, never read
as a bug in either side. The backfill cannot see it; the stream replays it only while retained. Tallies
(`proposal_versions` yes/no/abstain counts) are derived by the tally worker and are not written. Assumption: the
event's `space_id` is the proposal's Space; a record is written with that one Space reference.

Tally gate (shadow only, nothing reads it): a geo-db serve that carries the view sink (geo-db c8c04842af or later) and
runs with `derived_rows_max` above 0 keeps one derived row per (proposal, version) in collection 9
(`proposal_vote_counts`: key proposal id then big-endian version, value yes/no/abstain as three u64, group the owning
Space), updated in the same frame as each vote the write path carries. The feeder writes the same records and needs no
change. It counts only votes written after the sink is live, so run it after the §2c-quater backfill is rewritten or
re-run on such a serve. Diff it against `proposal_versions` with `geodb-feeder/src/proposalTallyGate.ts <postgres.tsv>
<scan.tsv>`; the inputs and the findings it classifies are in that file's header. It exits non-zero on a count
difference or a version geo-db lacks; geo-db-only versions are reported, as for the votes.

Gate before anything reads these records (a shadow read, a diff job, a cutover): diff `proposal_votes` against the
keyed records, by row count and by a checksum over (proposal, version, voter, vote), and assert that every record has
exactly one active space reference. Report geo-db-only keys and geo-db-newer values as findings, and make the shadow
target tolerate them rather than fail on them. Do not proceed on an unexplained difference.

Activation order, each step only after the one before it is live (steps 1 and 3 as §2c-ter):

1. geo-db serve and the init container on 9d97069a78 or later (`put`).
2. A feeder image that carries this path (inert while either flag is off).
3. Kafka ACL (not yet checked): consume on `<env>.space.governance` (the proposals consumer already has it, §2b-bis)
   and on the group `geodb-feeder-proposal-votes-keyed-v1-<env>`, and describe on every feeder group for the
   backfill Job's empty-group check. Without them the consumer fails to start, the edits consumer is unaffected, and
   `geodb_feeder_consumer_start_failed{consumer="proposal_votes"}` goes to 1 (`GeoDbFeederConsumerStartFailed`).
4. Required: the proposal-votes backfill Job (`geodb/k8s/v2/geodb-proposal-votes-backfill-job.yaml`), with the
   feeder scaled to 0. It reads `proposal_votes` READ ONLY in primary-key pages and writes every row as its record
   (rerunnable). `testnet.space.governance` keeps 7 days (`retention_ms` 604800000, cleanup `delete`, 2 partitions;
   `doctl databases topics get <cluster id> testnet.space.governance`), so every older vote comes from this
   snapshot. The Job refuses once `/data/feeder/proposal-votes-keyed/offsets.json` exists, for the reason in
   §2c-ter step 4; `BACKFILL_KEYED_RESET_CONSUMER: "1"` deletes those offsets and the consumer replays the retained
   topic over the snapshot. Ordering holds: hermes keys PROPOSAL_VOTED by Space, and every record key includes the
   proposal, which belongs to one Space, so one key's events are on one partition in order.

   ```bash
   $K scale deploy/geodb-feeder --replicas=0
   GEODB_TAG=$(awk '/^ *- name: serve$/{s=1} s && /image: registry.digitalocean.com\/geo\/geodb:/{sub(/.*geodb:/,""); print; exit}' geodb/k8s/v2/geodb.yaml)
   sed -e "s|geodb-feeder:latest|geodb-feeder:$TAG|" -e "s|geodb:GEODB_TAG|geodb:$GEODB_TAG|" \
     geodb/k8s/v2/geodb-proposal-votes-backfill-job.yaml | $K apply -f -
   $K logs -f job/geodb-proposal-votes-backfill
   ```

5. `GEODB_RECORD_KEYS: "1"` and `GEODB_PROPOSAL_VOTES: "1"` in the feeder manifest, then scale it back to 1.
6. The gate above, before any reader depends on the records.

   The gate is `geodb-feeder/src/votesGate.ts` (`bun src/votesGate.ts proposal|user <postgres.tsv> <dump dir>`;
   `user` is the §2c-ter gate). Read only on both sides: `\copy` the Postgres rows from a read-only session, then dump
   each record column from the votes Space with one GQL query per column (`MATCH (r)-[:`<kind's first key column>`]->(k)
   RETURN r, r.`<column>``; ids in backticks; one column per query, a two-column dump of every record exceeds the
   gRPC message limit). Take the Postgres read first and the geo-db dumps after it: both sides keep moving, and a vote
   that lands between the two reads shows up as a geo-db-only record or a one-column difference that the next read
   closes. Exit status is 1 for a Postgres-only key, a value difference or a record without exactly one reference.

A failing message of any kind, a ResourceExhausted founding the votes Space included, is retried every 5 s without
committing: the partition stalls and `GeoDbFeederProposalVotesStalled` fires after 30 min. Metrics:
`geodb_feeder_proposal_vote_events_total{outcome}`, `geodb_feeder_proposal_votes_consumer_lag`,
`geodb_feeder_proposal_votes_consumer_running`, `geodb_feeder_proposal_vote_freshness_seconds`.

Not migrated, and why (the rest of the governance family is not clean as keyed records): `proposals` and
`proposal_versions` (the version number is assigned by Postgres as `current_version + 1`, settings escalation updates
a version row in place (§2c-octies keeps it as a record), tallies are derived, `unexecutable_at` is set only by an operator backfill, `executed_at`
comes from PROPOSAL_EXECUTED), and `proposal_actions` (keyed on the Postgres-assigned version). Proposal entities are
already written by §2b-bis.

Rollback: unset `GEODB_PROPOSAL_VOTES`. The records stay in the votes Space, unread.

### 2c-quinquies. Keyed proposal actions (GEODB_PROPOSAL_ACTIONS)

`GEODB_PROPOSAL_ACTIONS` (default off; takes effect only with `GEODB_RECORD_KEYS`) writes gaia's `proposal_actions`
as natural-key records in the `governance` system Space (`geodb-feeder/src/proposalActionsKeyed.ts`; the pinned
`proposalAction` kind). Its own flag, because its backfill must run before its consumer. The feeder consumes
`<env>.space.governance` on its own group, `geodb-feeder-proposal-actions-keyed-v1-<env>`, and handles
`PROPOSAL_CREATED` and `PROPOSAL_UPDATED`; every other event counts as `ignored`. kg-indexer's group is untouched and
the api still reads Postgres.

Key, and the one deliberate difference from Postgres: Postgres keys a row by (proposal, version ordinal, index). An
UPDATE event does not carry its ordinal (storage assigns `current_version + 1`), so the record is keyed by
(proposal, version-stamp block, index), where the block is the event's `meta.block_number`, equal to
`proposal_versions.version_created_at_block` and unique per proposal. The ordinal is not stored; a later
`proposal_versions` consumer, or ranking the blocks per proposal, recovers it. The backfill and the gate map a row's
version to its block with a join on `proposal_versions(proposal_id, proposal_version)`. The legacy `fast_threshold`
(= flat) and `slow_threshold` (= partial) columns are not stored; readers derive them.

The feeder dead-letters a bad 16-byte id, missing settings, a wrong wire type or a non-UTF-8 `content_uri`; an action
with a bad target length or no oneof member is written as `Unknown`. As for §2c-quater, drift in either direction
(kg-indexer drops a whole block on a permanent failure) is to be investigated, never read as a bug in either side.

Activation order, each step only after the one before it is live:

1. geo-db serve and init container on 9d97069a78 or later (`put`); a feeder image that carries this path.
2. Kafka ACL: consume on the group `geodb-feeder-proposal-actions-keyed-v1-<env>` (the topic is already granted), and
   describe on every feeder group for the backfill Job's empty-group check. Without them the consumer fails to start
   and `geodb_feeder_consumer_start_failed{consumer="proposal_actions"}` goes to 1.
3. Required: the backfill Job (`geodb/k8s/v2/geodb-proposal-actions-backfill-job.yaml`), feeder scaled to 0, applied
   as in §2c-quater step 4. It reads `proposal_actions` JOIN `proposal_versions` READ ONLY in keyset pages and is
   rerunnable. `space.governance` keeps 7 days, so every older action comes from this snapshot.
4. `GEODB_RECORD_KEYS: "1"` and `GEODB_PROPOSAL_ACTIONS: "1"` in the feeder manifest, then scale it back to 1.
5. The gate, before any reader depends on the records: `bun src/proposalActionsGate.ts <postgres.tsv> <dump dir>`
   (`geodb-feeder/src/proposalActionsGate.ts`; its header lists the `\copy` columns and the `pa-<name>.tsv` dumps).
   It exits non-zero on a Postgres-only key or a value difference; geo-db-only keys are reported, not failed.

Rollback: unset `GEODB_PROPOSAL_ACTIONS`. The records stay in the Space, unread; nothing reads them yet.

### 2c-sexies. Keyed subspaces (GEODB_SUBSPACES)

`GEODB_SUBSPACES` (default off; takes effect only with `GEODB_RECORD_KEYS`) writes gaia's `subspaces` and
`subspace_topics` as edges in the `governance` system Space (`geodb-feeder/src/subspacesKeyed.ts`; kinds `subspace`
and `subspaceTopic`). Its own flag, because its backfill must run before its consumer. The feeder consumes
`<env>.space.trust.extensions` on its own group, `geodb-feeder-subspaces-keyed-v1-<env>`, and handles
`event-type: TRUST_EXTENSION`; everything else counts as `ignored`. kg-indexer's group is untouched and the api and
PostGraphile still read Postgres. One edge per Postgres primary key, as §2c-bis does for membership:

- subject `cid("entity", parent Space)`, value a reference to `cid("entity", child Space)` (or the topic entity);
- property by type: `verified` and `related` (the `type` is part of `subspaces`' primary key) and `topic` for
  `subspace_topics`, pinned in `recordKinds.ts`;
- an insert variant is `create_entity` + `add` + `set_active(true)`; a removal variant (kg-indexer deletes the row) is
  `set_active(false)`. A reader takes the ACTIVE edges as the table's rows.

The feeder dead-letters a source or target that is not 16 bytes and ignores a message with no recognised variant, as
`handle_trust_extension` does. Drift in either direction (kg-indexer drops a whole block on a permanent failure, an
FK violation among them) is to be investigated, never read as a bug in either side.

Activation order, each step only after the one before it is live:

1. geo-db serve and init container on 9d97069a78 or later; a feeder image that carries this path.
2. Kafka ACL: consume on `<env>.space.trust.extensions` (not yet granted to the feeder) and on the group
   `geodb-feeder-subspaces-keyed-v1-<env>`, and describe on every feeder group for the backfill Job's empty-group
   check. Without them the consumer fails to start and `geodb_feeder_consumer_start_failed{consumer="subspaces"}`
   goes to 1.
3. Required: the backfill Job (`geodb/k8s/v2/geodb-subspaces-backfill-job.yaml`), feeder scaled to 0, applied as in
   §2c-quater step 4. It reads both tables READ ONLY (small) and writes every row as an active edge (rerunnable). It
   never retracts, so it refuses once the consumer has offsets, as §2c-ter step 4 explains
   (`BACKFILL_KEYED_RESET_CONSUMER: "1"` to replay the retained topic over the snapshot).
4. `GEODB_RECORD_KEYS: "1"` and `GEODB_SUBSPACES: "1"` in the feeder manifest, then scale it back to 1.
5. The gate, before any reader depends on the edges: `bun src/subspacesGate.ts <postgres.tsv> <dump dir>`
   (`geodb-feeder/src/subspacesGate.ts`; its header gives the `\copy` query and the `subspaces-<type>.tsv` dumps of
   active edges). It exits non-zero on a Postgres-only row; geo-db-only edges are reported, not failed.

Rollback: unset `GEODB_SUBSPACES`. The edges stay in the Space, unread; nothing reads them yet.

### 2c-septies. Keyed space voting settings (GEODB_SPACE_VOTING_SETTINGS)

`GEODB_SPACE_VOTING_SETTINGS` (default off; takes effect only with `GEODB_RECORD_KEYS`) writes gaia's
`space_voting_settings` as natural-key records in the `governance` system Space
(`geodb-feeder/src/spaceVotingSettingsKeyed.ts`; the pinned `spaceVotingSettings` kind). Its own flag, because its
backfill must run before its consumer. The feeder consumes `<env>.space.governance` on its own group,
`geodb-feeder-space-voting-settings-keyed-v1-<env>`, and handles `VOTING_SETTINGS_UPDATED`; every other event counts as
`ignored`. kg-indexer's group is untouched and the api still reads Postgres.

One record per Space, keyed by the Space (the table's primary key). The key column is a reference to the Space's entity;
the eight settings columns, `updated_at` and `updated_at_block` are mutable and written by `put`, so a later event
replaces the earlier one as `ON CONFLICT (space_id) DO UPDATE` does (last event per Space on its partition wins; a
replay converges, and A, B, A ends on A). Values are the `as i64` casts kg-indexer makes.

The feeder dead-letters a `space_id` that is not 16 bytes or a wrong wire type; missing metadata is written as time 0,
block 0. As for §2c-quater, drift in either direction (kg-indexer drops a whole block on a permanent failure, and the
table has an FK to `spaces`) is to be investigated, never read as a bug in either side.

Activation order, each step only after the one before it is live:

1. geo-db serve and init container on 9d97069a78 or later (`put`); a feeder image that carries this path.
2. Kafka ACL: consume on the group `geodb-feeder-space-voting-settings-keyed-v1-<env>` (the topic is already granted),
   and describe on every feeder group for the backfill Job's empty-group check. Without them the consumer fails to
   start and `geodb_feeder_consumer_start_failed{consumer="space_voting_settings"}` goes to 1.
3. Required: the backfill Job (`geodb/k8s/v2/geodb-space-voting-settings-backfill-job.yaml`), feeder scaled to 0,
   applied as in §2c-quater step 4. It reads `space_voting_settings` READ ONLY (one row per Space) and is rerunnable.
   `space.governance` keeps 7 days, and settings change rarely, so almost every row comes from this snapshot.
4. `GEODB_RECORD_KEYS: "1"` and `GEODB_SPACE_VOTING_SETTINGS: "1"` in the feeder manifest, then scale it back to 1.
5. The gate, before any reader depends on the records: `bun src/spaceVotingSettingsGate.ts <postgres.tsv> <dump dir>`
   (`geodb-feeder/src/spaceVotingSettingsGate.ts`; its header lists the `\copy` columns and the `svs-<name>.tsv` dumps).
   It exits non-zero on a Postgres-only key or a value difference; geo-db-only keys are reported, not failed.

Rollback: unset `GEODB_SPACE_VOTING_SETTINGS`. The records stay in the Space, unread; nothing reads them yet.

### 2c-config. Keyed for_you_config mirror (GEODB_FOR_YOU_CONFIG)

`GEODB_FOR_YOU_CONFIG` (default off; takes effect only with `GEODB_RECORD_KEYS`) lets the
`geodb-for-you-config-backfill` Job write the one `personalization.for_you_config` row as a natural-key record in the
`config` system Space (`geodb-feeder/src/forYouConfigKeyed.ts`; the pinned `forYouConfig` kind). First slice of the
config tables (geo-db `docs/plans/10-wire-and-federation/11-zero-postgres-table-inventory.md` §3.4 and §6). A mirror,
not a move: Postgres stays the source (`readSettings` and `user_topic_weights` read it). There is no consumer, because
nothing writes the table but migrations and operator SQL; the Job is the only writer, and a rerun after an operator edit
writes the new values (a changed value closes the old statement; the record keeps one active value a column).

The api compares it: `GEODB_FOR_YOU_CONFIG_SHADOW` (api, default `0`) reads the record after a For you request and counts
`gaia_api_for_you_config_shadow_total{result=match|mismatch|unsupported|failed}`. It never changes a response.

Enable, each step only after the one before it is live:

1. A feeder image that carries this path (the Job's image) and a geo-db serve at 9d97069a78 or later (`put`).
2. Kafka: describe on every feeder group (the Job's empty-group check). No consume ACL is new.
3. The backfill Job (`geodb/k8s/v2/geodb-for-you-config-backfill-job.yaml`), feeder scaled to 0, applied as in §2c-quater
   step 4. It reads one row READ ONLY and is rerunnable. It founds the `config` system Space on first use, the third of
   the three families against the serve's Space cap.
4. The gate: `bun src/forYouConfigGate.ts <postgres.tsv> <dump dir>` (its header lists the `\copy` and the `fyc-<name>.tsv`
   dumps). Exits non-zero on a difference.
5. `GEODB_FOR_YOU_CONFIG_SHADOW: "1"` in `api/k8s/v2/api.yaml`; watch `mismatch`, `unsupported{system_space_missing}`.

Rollback: set both flags back to 0. The record stays in the Space, unread.

### 2c-config-ranking. Keyed entity_ranking_config mirror (GEODB_ENTITY_RANKING_CONFIG)

`GEODB_ENTITY_RANKING_CONFIG` (default off; takes effect only with `GEODB_RECORD_KEYS`) lets the
`geodb-entity-ranking-config-backfill` Job write the one `entity_ranking_config` row (31 tunables: tau, Wilson, caps,
participation, comment, topic, watch, trending) as a natural-key record in the `config` system Space
(`geodb-feeder/src/entityRankingConfigKeyed.ts`; the pinned `entityRankingConfig` kind). Second slice of the config tables
(after §2c-config). A mirror, not a move: Postgres stays the source. Every reader but one is a plpgsql function
(`SELECT * INTO cfg FROM entity_ranking_config WHERE id`, migrations 0073 to 0118, plus the feed-composition samplers and
`trending`/`watch` scorers), which cannot read geo-db, so those keep reading the table. The one TypeScript reader is
`readSettings` (`api/src/forYou/queries.ts`), which reads `tau_seconds`. Nothing writes the table but migrations and
operator SQL, so there is no consumer: the Job is the only writer, and a rerun after an operator edit writes the new
values (a changed value closes the old statement). `updated_at` is not mirrored.

The api compares it: `GEODB_ENTITY_RANKING_CONFIG_SHADOW` (api, default `0`) reads the record after a For you request and
counts `gaia_api_entity_ranking_config_shadow_total{result=match|mismatch|unsupported|failed}` for `tau_seconds`, the only
value the api sees. It never changes a response. The other 30 columns are checked by the gate, not live.

Enable, each step only after the one before it is live:

1. A feeder image that carries this path (the Job's image) and a geo-db serve at 9d97069a78 or later (`put`).
2. Kafka: describe on every feeder group (the Job's empty-group check). No consume ACL is new.
3. The backfill Job (`geodb/k8s/v2/geodb-entity-ranking-config-backfill-job.yaml`), feeder scaled to 0, applied as in
   §2c-quater step 4. It reads one row READ ONLY and is rerunnable. The `config` system Space already exists once
   §2c-config has run (no new Space against the serve's cap).
4. The gate: `bun src/entityRankingConfigGate.ts <postgres.tsv> <dump dir>` (its header lists the `\copy` and the
   `erc-<column>.tsv` dumps). Exits non-zero on a difference.
5. `GEODB_ENTITY_RANKING_CONFIG_SHADOW: "1"` in `api/k8s/v2/api.yaml`; watch `mismatch`, `unsupported{system_space_missing}`.

Rollback: set both flags back to 0. The record stays in the Space, unread.

### 2c-config-interest. Keyed interest_config and interest_signal_weights mirror (GEODB_INTEREST_CONFIG)

`GEODB_INTEREST_CONFIG` (default off; takes effect only with `GEODB_RECORD_KEYS`) lets the `geodb-interest-config-backfill`
Job write `personalization.interest_config` (the one tunables row: half-life, spread, co-occurrence limits, sweep lookback)
and every `personalization.interest_signal_weights` row (one per signal kind: weight, decays, note) as natural-key records
in the `config` system Space (`geodb-feeder/src/interestConfigKeyed.ts`; the pinned `interestConfig` and
`interestSignalWeight` kinds). Third slice of the config tables (after §2c-config and §2c-config-ranking). A mirror, not a
move: Postgres stays the source. Every reader is a plpgsql function (api/drizzle 0100, 0105, 0117) and no application code
reads either table, so there is no api shadow; the gate is the check. Nothing writes the tables but migrations
(0100 seeds; 0105, 0106, 0107 retune weights) and operator SQL, so there is no consumer: the Job is the only writer, and a
rerun after an operator edit writes the new values (a changed value closes the old statement). `sweep_lookback` is an
interval, mirrored as seconds. A note set to null, or a kind deleted from Postgres, is not cleared in geo-db; the gate
reports it.

Enable, each step only after the one before it is live:

1. A feeder image that carries this path (the Job's image) and a geo-db serve at 9d97069a78 or later (`put`).
2. Kafka: describe on every feeder group (the Job's empty-group check). No consume ACL is new.
3. The backfill Job (`geodb/k8s/v2/geodb-interest-config-backfill-job.yaml`), feeder scaled to 0, applied as in
   §2c-quater step 4. It reads two small tables READ ONLY and is rerunnable. The `config` system Space already exists once
   §2c-config has run.
4. The gate, once per table: `bun src/interestConfigGate.ts config <postgres.tsv> <dump dir>` and
   `bun src/interestConfigGate.ts weights <postgres.tsv> <dump dir>` (its header lists the `\copy` and the `ic-<column>.tsv`
   / `isw-<column>.tsv` dumps). Exits non-zero on a Postgres-only key or a value difference.

Rollback: set the flag back to 0. The records stay in the Space, unread.

### 2c-config-experiments. Keyed feed_experiments mirror (GEODB_FEED_EXPERIMENTS)

`GEODB_FEED_EXPERIMENTS` (default off; takes effect only with `GEODB_RECORD_KEYS`) lets the `geodb-feed-experiments-backfill`
Job write every `personalization.feed_experiments` row (id, active, arm_a, arm_b, share, salt, note) as a natural-key record
in the `config` system Space (`geodb-feeder/src/feedExperimentsKeyed.ts`; the pinned `feedExperiment` kind, keyed by the
experiment id). Fourth slice of the config tables (after §2c-config-interest). A mirror, not a move: Postgres stays the
source. The reader is the plpgsql `personalization.feed_experiment_for_user` (api/drizzle 0102:184), called from
`readExperiment` (api/src/forYou/queries.ts); the api has a flag-off shadow of it, `GEODB_FEED_EXPERIMENT_SHADOW`
(`gaia_api_feed_experiment_shadow_total`), that checks the served experiment is active in geo-db with the same arms and, for a
hash-assigned user, recomputes `interleaved` from the record's salt and share. `created_at` is not mirrored; membership is
§2c-config-experiment-members. Nothing writes the table but migrations and operator SQL, so
there is no consumer: the Job is the only writer, and a rerun after an operator edit writes the new values. A note set to
null, or an experiment deleted from Postgres, is not cleared in geo-db; the gate reports it.

Enable, each step only after the one before it is live:

1. A feeder image that carries this path (the Job's image) and a geo-db serve at 9d97069a78 or later (`put`).
2. Kafka: describe on every feeder group (the Job's empty-group check). No consume ACL is new.
3. The backfill Job (`geodb/k8s/v2/geodb-feed-experiments-backfill-job.yaml`), feeder scaled to 0, applied as in
   §2c-quater step 4. It reads one small table READ ONLY and is rerunnable.
4. The gate: `bun src/feedExperimentsGate.ts <postgres.tsv> <dump dir>` (its header lists the `\copy` and the
   `fe-<column>.tsv` dumps). Exits non-zero on a Postgres-only key or a value difference.
5. Only then set `GEODB_FEED_EXPERIMENT_SHADOW=1` on the api and watch the mismatch counter.

Rollback: set the flags back to 0. The records stay in the Space, unread.

### 2c-config-experiment-members. Keyed feed_experiment_members mirror (GEODB_FEED_EXPERIMENT_MEMBERS)

`GEODB_FEED_EXPERIMENT_MEMBERS` (default off; takes effect only with `GEODB_RECORD_KEYS`) lets the
`geodb-feed-experiment-members-backfill` Job write every `personalization.feed_experiment_members` row (explicit
membership, which wins over the hash) as a natural-key record in the `config` system Space
(`geodb-feeder/src/feedExperimentMembersKeyed.ts`; the pinned `feedExperimentMember` kind, keyed by experiment id and
user). Fifth slice of the config tables. A mirror, not a move: Postgres stays the source. The one reader is the plpgsql
`personalization.feed_experiment_for_user` (api/drizzle/0102:184-193), called from `readExperiment`
(api/src/forYou/queries.ts:236); the api's existing shadow, `GEODB_FEED_EXPERIMENT_SHADOW`, now also compares a member's
served `interleaved` with the member record (a missing record counts as unsupported, `member_record_missing`). `added_at`
is not mirrored. Nothing writes the table but migrations and operator SQL, so the Job is the only writer; rerun it after an
edit. A membership deleted from Postgres is not cleared in geo-db; the gate reports it.

Enable, each step only after the one before it is live (run §2c-config-experiments first, the shadow needs both):

1. A feeder image that carries this path and a geo-db serve at 9d97069a78 or later (`put`).
2. Kafka: describe on every feeder group (the Job's empty-group check).
3. The backfill Job (`geodb/k8s/v2/geodb-feed-experiment-members-backfill-job.yaml`), feeder scaled to 0, applied as in
   §2c-quater step 4. It reads one small table READ ONLY and is rerunnable.
4. The gate: `bun src/feedExperimentMembersGate.ts <postgres.tsv> <dump dir>` (its header lists the `\copy` and the
   `fem-<column>.tsv` dumps). Exits non-zero on a Postgres-only key or a value difference.

Rollback: set the flags back to 0. The records stay in the Space, unread.

### 2c-octies. Keyed proposal versions (GEODB_PROPOSAL_VERSIONS)

`GEODB_PROPOSAL_VERSIONS` (default off; takes effect only with `GEODB_RECORD_KEYS`) writes gaia's `proposal_versions`
as natural-key records in the `governance` system Space (`geodb-feeder/src/proposalVersionsKeyed.ts`; the pinned
`proposalVersion` kind). It reads the same `PROPOSAL_CREATED` and `PROPOSAL_UPDATED` events as §2c-quinquies, on its
own group, `geodb-feeder-proposal-versions-keyed-v1-<env>` (kg-indexer's group is untouched; the api still reads
Postgres). It closes the gap §2c-quinquies leaves: the version each action belongs to.

Key: (proposal, version-stamp block), the block being `meta.block_number` of the event, equal to
`proposal_versions.version_created_at_block` (unique per proposal). The ordinal `proposal_version` is not stored
(storage assigns `current_version + 1`, which an event does not carry); a reader ranks the blocks of a proposal. The gate
checks that rank against Postgres.

Columns: voting_mode (`Fast`/`Slow`), start_time, end_time, quorum, partial, universal, flat, created_at, and only when
present execute_by (the event's 0 is "no deadline") and name (kg-indexer's `derive_proposal_name`; none for an event
without actions). Not stored: the ordinal; `threshold` (legacy copy of flat for Fast, partial for Slow); `yes_count`,
`no_count`, `abstain_count` (kg-indexer's tally worker writes them; geo-db derives them from the proposal-vote records).

Escalation: a fast-to-slow escalation (`PROPOSAL_SETTINGS_UPDATED`) rewrites the current version's settings in place in
Postgres, and the event does not say which version. The same consumer writes it as its own immutable record keyed
(proposal, block of the event), holding the eight settings the event set (execute_by absent when the event's is 0). The
version it hit is resolved at READ time: the proposal's version with the greatest block at or before the escalation's block;
the latest escalation of a version (greatest block, then record id) supplies all eight settings. An escalation with no
version at or before its block is an orphan (kg-indexer did nothing). Because the record depends on the event alone, a
replay is a no-op and converges, including after the backfill, which carries the escalated values in the version records.
There is no escalation backfill (Postgres keeps no history of the event), so an escalation older than the topic's 7 days
is represented only by the escalated values in the backfilled version. Two events in one block cannot be ordered: an
escalation in the block that creates a version is taken to follow it, and two in one block resolve by record id.

Activation order, each step only after the one before it is live:

1. geo-db serve and init container on 9d97069a78 or later (`put`); a feeder image that carries this path.
2. Kafka ACL: consume on the group `geodb-feeder-proposal-versions-keyed-v1-<env>` (the topic is already granted), and
   describe on every feeder group for the backfill Job's empty-group check. Without them the consumer fails to start
   and `geodb_feeder_consumer_start_failed{consumer="proposal_versions"}` goes to 1.
3. Required: the backfill Job (`geodb/k8s/v2/geodb-proposal-versions-backfill-job.yaml`), feeder scaled to 0, applied
   as in §2c-quater step 4. It reads `proposal_versions` READ ONLY in keyset pages and is rerunnable.
   `space.governance` keeps 7 days, so every older version comes from this snapshot.
4. `GEODB_RECORD_KEYS: "1"` and `GEODB_PROPOSAL_VERSIONS: "1"` in the feeder manifest, then scale it back to 1.
5. The gate, before any reader depends on the records: `bun src/proposalVersionsGate.ts <postgres.tsv> <dump dir>`
   (`geodb-feeder/src/proposalVersionsGate.ts`; its header lists the `\copy` columns and the `pv-<name>.tsv` and `pe-<name>.tsv`
   dumps; the escalation dumps overlay the versions before the compare).
   It exits non-zero on a Postgres-only key, a value difference or an ordinal that is not the block's rank; geo-db-only
   keys and orphan escalations are reported, not failed.

Rollback: unset `GEODB_PROPOSAL_VERSIONS`. The records stay in the Space, unread; nothing reads them yet.

### 2c-novies. Keyed proposal status facts (GEODB_PROPOSAL_STATUS)

Read this first: **gaia stores no proposal status, so this does not mirror one.** `proposals` has no status column. The
api computes "proposed", "executable", "rejected" and the rest on every read (`api/src/proposals/status.ts`
`computeProposalStatus`, and the SQL twins `sqlIsProposed`, `sqlIsExecutable`, `sqlIsRejected` in
`api/src/proposals/queries.ts`) from the votes, the settings snapshot, `executed_at`, `unexecutable_at` and the current
time. Most of those transitions have no event: a voting window closes, a grace period lapses. A status column in geo-db
would be stale the moment it was written, so the transitions do not belong in the feeder.

What the feeder can mirror are the two stored inputs, and `GEODB_PROPOSAL_STATUS` (default off; takes effect only with
`GEODB_RECORD_KEYS`) does exactly that, as natural-key records in the `governance` system Space
(`geodb-feeder/src/proposalStatusKeyed.ts`; the pinned `proposalStatus` kind), one per proposal, keyed by the proposal:

- `executed_at`: event-written. `PROPOSAL_EXECUTED` on `<env>.space.governance` becomes
  `UPDATE proposals SET executed_at = <block time>` in kg-indexer; the feeder consumes it on its own group,
  `geodb-feeder-proposal-status-keyed-v1-<env>`, and `put`s it (a second event replaces the first, as the UPDATE does).
  Every other event counts as `ignored`.
- `unexecutable_at`: **no event.** It is set by an operator-run script,
  `proposal-executor/src/backfill-unexecutable.ts` (dry run by default, an on-chain `latestProposalVersion == 0`
  check). The consumer cannot carry it; only the backfill does, reading Postgres. A value set after the backfill reaches
  geo-db only on a rerun (below), until that script publishes an event, a change in `proposal-executor`.

Where the computed status belongs: a function over these records, the votes (`proposal_votes`, §2c-quater), the
settings (`space_voting_settings`, §2c-septies) and `now`, evaluated at read time, or a derived store class
refreshed on the clock (the GAP-9 materialized-view question). It is not a stored column, and it needs a reader,
which does not exist yet. The gate below checks only the stored facts.

Activation order, each step only after the one before it is live:

1. geo-db serve and init container on 9d97069a78 or later (`put`); a feeder image that carries this path.
2. Kafka ACL: consume on the group `geodb-feeder-proposal-status-keyed-v1-<env>` (the topic is already granted), and
   describe on every feeder group for the backfill Job's empty-group check. Without them the consumer fails to start and
   `geodb_feeder_consumer_start_failed{consumer="proposal_status"}` goes to 1.
3. Required: the backfill Job (`geodb/k8s/v2/geodb-proposal-status-backfill-job.yaml`), feeder scaled to 0, applied as in
   §2c-quater step 4. It reads `proposals` READ ONLY, only rows with `executed_at` or `unexecutable_at` set, and is
   rerunnable. `space.governance` keeps 7 days, so almost every row comes from this snapshot.
4. `GEODB_RECORD_KEYS: "1"` and `GEODB_PROPOSAL_STATUS: "1"` in the feeder manifest, then scale it back to 1.
5. The gate, before any reader depends on the records: `bun src/proposalStatusGate.ts <postgres.tsv> <dump dir>`
   (`geodb-feeder/src/proposalStatusGate.ts`; its header lists the `\copy` columns and the `ps-<name>.tsv` dumps). It
   exits non-zero on a Postgres-only key or a value difference; geo-db-only keys are reported, not failed.

Carrying a later `unexecutable_at`: after an operator runs the unexecutable script, rerun the Job. Once the consumer has
offsets the Job refuses unless `BACKFILL_KEYED_RESET_CONSUMER` is `"1"` (which deletes the consumer's offsets, so it replays
the retained 7 days from earliest on its next start: safe here, because a replayed PROPOSAL_EXECUTED carries the
same value Postgres holds and writes the same ids).

Drift in either direction is to be investigated, never read as a bug in either side: the UPDATE matches no row for a
proposal Postgres does not hold yet, and kg-indexer drops a whole block on a permanent failure.

Rollback: unset `GEODB_PROPOSAL_STATUS`. The records stay in the Space, unread; nothing reads them yet.

### 2c-decies. Keyed ranks (GEODB_RANKS)

`GEODB_RANKS` (default off; takes effect only with `GEODB_RECORD_KEYS`) mirrors the three tables ranking-indexer folds
out of `knowledge.edits` into the `ranks` schema (`ranking_blocks`, `rankings`, `ranking_items`; api migrations 0061 and
0068) as natural-key records, one per primary key (`geodb-feeder/src/ranksKeyed.ts`; the pinned `rankingBlock`,
`ranking` and `rankingItem` kinds). They live in the `votes` system Space, so no fourth system Space is founded. The
consumer is its own group on `<env>.knowledge.edits`, `geodb-feeder-ranks-keyed-v1-<env>`; the edits consumer's group and
ranking-indexer's are untouched, and the feeder's Kafka user already consumes the topic.

How the fold maps (ranking-indexer's, replayed through `detectRanks`):

- Blocks and rankings: each column is a mutable `put`, the value in the statement id, so a replay writes the same ids
  and is a no-op, and A then B then A ends on A. A value an edit does not carry is never written and never cleared.
- Items: a ranking edit replaces the ranking's items as a whole. geo-db has no read-before-write, so every item carries
  a `generation` (the edit's block number; 0 for the backfill) and a reader takes, per ranking, only the items at the
  ranking's maximum generation. Replaying the stream from the start converges on the same state; the stream supersedes
  the backfill.
- Not mirrored: `ranking_scores`, and the `RANK_POSITION` projection. `author_address` is carried by the backfill only.

Differences from Postgres, to be known before reading the records:

- Two edits in the same block for one ranking have the same generation, so their items are unioned (Postgres keeps the
  later). A missing block number in the edit metadata gives generation 0.
- A `RANK_BLOCK` link writes the ranking's `block` column even when the ranking is created in another edit.
- A value absent from an edit is not cleared; Postgres overwrites it with NULL.
- An item without `to_space` is skipped, as ranking-indexer skips it.

Activation order, each step only after the one before it is live:

1. geo-db serve and init container on 9d97069a78 or later (`put`); a feeder image that carries this path.
2. Kafka ACL: consume on the group `geodb-feeder-ranks-keyed-v1-<env>` (the topic is already granted), and describe on
   every feeder group for the backfill Job's empty-group check. Without them the consumer fails to start and
   `geodb_feeder_consumer_start_failed{consumer="ranks"}` goes to 1.
3. Required: the backfill Job (`geodb/k8s/v2/geodb-ranks-backfill-job.yaml`), feeder scaled to 0, applied as in
   §2c-quater step 4. It reads the three tables READ ONLY and is rerunnable. `knowledge.edits` retains a bounded window,
   so older rankings come only from this snapshot.
4. `GEODB_RECORD_KEYS: "1"` and `GEODB_RANKS: "1"` in the feeder manifest, then scale it back to 1.
5. The gate, once per table, before any reader depends on the records:
   `bun src/ranksGate.ts <blocks|rankings|items> <postgres.tsv> <dump dir>` (`geodb-feeder/src/ranksGate.ts`; its header
   lists the `\copy` columns and the `rb-`, `rk-` and `ri-` dumps). It exits non-zero on a Postgres-only key or a value
   difference; geo-db-only keys (items of a superseded generation) are reported, not failed.

A rerun of the Job once the consumer has offsets refuses unless `BACKFILL_KEYED_RESET_CONSUMER` is `"1"` (which deletes
the consumer's offsets, so it replays the retained window from earliest on its next start; safe, the replay writes the
same ids).

Rollback: unset `GEODB_RANKS`. The records stay in the Space, unread; nothing reads them yet.

### 2c-undecies. Keyed space topics (GEODB_SPACE_TOPICS)

`GEODB_SPACE_TOPICS` (default off; takes effect only with `GEODB_RECORD_KEYS`) mirrors `spaces.topic_id`, the column
kg-indexer sets from `<env>.space.topics`, as one natural-key record per Space in the `governance` system Space
(`geodb-feeder/src/spaceTopicsKeyed.ts`; the pinned `spaceTopic` kind). The consumer is its own group on
`<env>.space.topics`, `geodb-feeder-space-topics-keyed-v1-<env>`; kg-indexer's group is untouched.

How the fold maps: `TOPIC_DECLARED` (or no `event-type` header, as kg-indexer reads it) is a `put` of the topic
reference, the topic in the statement id, so a new topic replaces the old one, a replay is a no-op and A then B then A
ends on A. `TOPIC_REMOVED` closes that topic's statement, so it clears the column only when the removed topic is the
current one, as `clear_space_topic` does. An id that is not 16 bytes is dead-lettered.

`spaces.address` is its own consumer (§2c-duodecies). A topic event for a Space Postgres lacks is written anyway
(kg-indexer's UPDATE would match no row).

Activation order, each step only after the one before it is live:

1. geo-db serve and init container on 9d97069a78 or later (`put`); a feeder image that carries this path.
2. Kafka ACL: consume on the group `geodb-feeder-space-topics-keyed-v1-<env>` and read on `<env>.space.topics`, and
   describe on every feeder group for the backfill Job's empty-group check. Without them the consumer fails to start and
   `geodb_feeder_consumer_start_failed{consumer="space_topics"}` goes to 1.
3. Required: the backfill Job (`geodb/k8s/v2/geodb-space-topics-backfill-job.yaml`), feeder scaled to 0, applied as in
   §2c-quater step 4. It reads `spaces WHERE topic_id IS NOT NULL` READ ONLY and is rerunnable.
4. `GEODB_RECORD_KEYS: "1"` and `GEODB_SPACE_TOPICS: "1"` in the feeder manifest, then scale it back to 1.
5. The gate, before any reader depends on the records: `bun src/spaceTopicsGate.ts <postgres.tsv> <dump dir>`
   (`geodb-feeder/src/spaceTopicsGate.ts`; its header lists the `\copy` columns and the `st-space.tsv` and
   `st-topic.tsv` dumps). It exits non-zero on a Postgres-only key or a value difference; geo-db-only keys (a removed
   topic's record, or a Space Postgres lacks) are reported, not failed.

A rerun of the Job once the consumer has offsets refuses unless `BACKFILL_KEYED_RESET_CONSUMER` is `"1"`.

Rollback: unset `GEODB_SPACE_TOPICS`. The records stay in the Space, unread; nothing reads them yet.

### 2c-duodecies. Keyed space addresses (GEODB_SPACE_ADDRESSES)

`GEODB_SPACE_ADDRESSES` (default off; takes effect only with `GEODB_RECORD_KEYS`) mirrors `spaces.address` as one
natural-key record per Space in the `governance` system Space (`geodb-feeder/src/spaceAddressesKeyed.ts`; the pinned
`spaceAddress` kind). The consumer is its own group on `<env>.space.creations`,
`geodb-feeder-space-addresses-keyed-v1-<env>`, a NEW group (the Kafka ACL must allow it); kg-indexer's and the Space
entities consumer's groups are untouched.

How the fold maps: `SPACE_ADDRESS_UPDATED` is a `put` of `0x` plus the lowercase hex of the 20-byte address, as
`update_space_address` writes it. A `HermesCreateSpace` (no `event-type` header) is a `put` of the EIP-55 checksummed
address of its `eoa_space.owner` or `default_dao_space.address`, as `handle_create_space` inserts it. The address is in
the statement id, so a new address replaces the old one, a replay is a no-op and A then B then A ends on A. A message
that is malformed, has no payload or an address that is not 20 bytes is dead-lettered, as kg-indexer errors on it.

Differences from Postgres, none of which the gate fails on except the first:
- A create is `ON CONFLICT DO NOTHING` in Postgres; a keyed write cannot read, so a second create for the same Space
  would overwrite an address an update set. kg-indexer emits one create per Space; the gate would show a mismatch.
- An update for a Space Postgres never saw created is written anyway (Postgres updates no row). Reported by the gate as
  geo-db-only.
- Casing follows the path, as in Postgres: checksummed from a create, lowercase from an update.

Activation order, each step only after the one before it is live:

1. geo-db serve and init container on 9d97069a78 or later (`put`); a feeder image that carries this path.
2. Kafka ACL: consume on the group `geodb-feeder-space-addresses-keyed-v1-<env>` and read on `<env>.space.creations`.
   Without them the consumer fails to start and `geodb_feeder_consumer_start_failed{consumer="space_addresses"}` goes to 1.
3. Required: the backfill Job (`geodb/k8s/v2/geodb-space-addresses-backfill-job.yaml`), feeder scaled to 0, applied as in
   §2c-quater step 4. It reads `spaces WHERE address IS NOT NULL` READ ONLY and is rerunnable.
4. `GEODB_RECORD_KEYS: "1"` and `GEODB_SPACE_ADDRESSES: "1"` in the feeder manifest, then scale it back to 1.
5. The gate: `bun src/spaceAddressesGate.ts <postgres.tsv> <dump dir>` (`geodb-feeder/src/spaceAddressesGate.ts`; its header
   lists the `\copy` columns and the `sa-space.tsv` and `sa-address.tsv` dumps).

A rerun of the Job once the consumer has offsets refuses unless `BACKFILL_KEYED_RESET_CONSUMER` is `"1"`.

Rollback: unset `GEODB_SPACE_ADDRESSES`. The records stay in the Space, unread; nothing reads them yet.

### 2f. Derived score tables (GEODB_DERIVED_MIRROR)

Shadow only, nothing reads these rows. With `GEODB_DERIVED_MIRROR=1` (default off) the score writers copy the rows they
write into geo-db's derived class (last-write-wins, no history) after their Postgres write commits. Postgres stays the
source of truth: a geo-db error is counted (`geodb_derived_mirror_errors_total{table}`) and logged, never returned, a
call is bounded by `GEODB_DERIVED_BUDGET_MS` (default 5000; rows past it are counted in
`geodb_derived_mirror_dropped_total` and left for the next write of the key), and after a failure the pipe is not
respawned for 10 s.

| writer | tables mirrored (collection) |
| -- | -- |
| `vote-indexer` (vote batches, `comment_sweep`, `new_entity_sweep`) | `entity_type_ranking` (0), `entity_topic_ranking` (1) |
| `ranking-indexer` (`replace_ranking_scores`) | `ranks.ranking_scores` (11) |

Both need `geodb-client` in the image (the init-container pattern of hermes, `hermes/k8s/v2/hermes-pipeline.yaml`),
`GEODB_KEY_PATH`, `GEODB_BINDING_PATH` and `GEODB_CLIENT_BIN`, and a serve opened with `derived_rows_max` above 0
(144 B a row resident). The key's binding line must grant `derived=0,1,2,3,4,5,11,12,13`; the hermes key grants only collection 10, so
this is a new key and binding (not minted). Not mirrored: `entity_ranking_scores` (48.9M rows, over the resident class),
trending and agenda listings.

`global_scores` (2), `local_scores` (3) and `space_scores` (4), written by the Python scoring cronjob
(`scoring-service/cronjob`), are mirrored behind `GEODB_DERIVED_MIRROR` in that cronjob (#1169, default `"0"`). It
rewrites every row each run (upserts, no deletes), after its Postgres commit. It uses the shared `geodb-scores-key`
(binding `derived=0,1,2,3,4,5,11,12,13`, see §2f-scores). **Sizing warning:** count `local_scores` (one row per
entity perspective) and add it to the other mirrored tables against `derived_rows_max` (4194304 since Stage A (#1182), was 2097152; 144 B a row
resident) before the flag goes on; a full rewrite past the cap is refused. Raise `GEODB_DERIVED_BUDGET_MS` and set
`GEODB_DERIVED_BATCH=1` for the full rewrite.

The SQL function `refresh_entity_ranking_scores` writes the type and topic tables itself, so the vote-indexer reads the
rows for the touched entity ids before and after the call and sends the difference (changed or new rows as puts, keys
that left as deletes). By default the pipe takes one row a line and fsyncs each, which suits incremental writes. For
a backfill set `GEODB_DERIVED_BATCH=1`: rows go 256 a request (`putn` / `deleten`, geo-db 14 V11), one fsync per
request. It needs a `geodb-client` that has those verbs (an older one answers the head with a refusal and desyncs the
pipe), so bump the image before turning it on.

Type ranking gate: `geodb-feeder/src/typeRankingGate.ts <postgres.tsv> <scan.tsv>` diffs `entity_type_ranking` against
`geodb-client derived scan 0 0x<type uuid hex> --first 512 --pages 100000` per type; the inputs and the classification
are in that file's header. Rows for entities not refreshed since the flag went on show as Postgres-only until their
next refresh, so run `refresh_ranking_scores_for` over the entity set after turning the flag on.


### 2f-order. Enable order for the derived mirrors (Stage A is live)

Plan 14 §10. Stage A (`geodb.yaml`, #1182, live: `derived_rows_max = 4194304`, serve request 5Gi, limit 7Gi; 144 B a row, ~604 MB)
holds the 2.86M rows counted on 11-06 (collections 0, 1, 8, 9, 10, 11). The full set with collections 2, 3 and 4 is 6.80M and does
not fit it: that is Stage B (`derived_rows_max = 8388608`, serve 6Gi/8Gi, one more restart and its own node-headroom check; the node
had 13.65 GiB allocatable and 11.1 GiB requested on 11-06 with Stage A in; Stage B's +1 GiB request would make it about 12.2 GiB
(89%), which fits but is tight, so re-check before rolling).

Turn the mirrors on one at a time, one flag per PR, each after the serve roll that carries its binding, checking
`geodb_derived_rows` against `geodb_derived_rows_max` between steps:
1. account_weights (§2f-weights, collection 5): one row per voting account, small.
2. account_exclusions (§2f-exclusions, collection 12): smaller still.
3. the scores (§2f-scores, collections 2, 3, 4). Count first: `global_scores` and `space_scores` are small, **`local_scores` is
   about 3.9M rows (one per entity perspective) and does not fit Stage A with the 2.86M already held: it waits for Stage B.** The
   scoring cronjob's `GEODB_DERIVED_MIRROR` mirrors all three tables unless `GEODB_DERIVED_MIRROR_TABLES` narrows it, and a
   full rewrite past the cap is refused partway, so keep the flag `"0"` until the counts show it fits. Under Stage A set
   `GEODB_DERIVED_MIRROR_TABLES=global_scores,space_scores` (the two small tables) together with `GEODB_DERIVED_MIRROR=1`;
   leave `local_scores` out until Stage B, then drop the variable (unset or empty means all three; a name that is not one
   of the three is ignored with a warning, and a value with no valid name mirrors nothing).

### 2f-private. Per-user collections 27, 28, 29 (mirror and private switch in the manifests; enable order below)

Three collections hold one person's private data: 27 `external_interest_signals` (`not_interested_sync`), 28
`user_topic_signals` (`topic_interest` sweep and refit) and 29 `feed_user_seen_daily` (`feed_signals_sync`). Until
geo-db restricts who may read a derived collection (plan 14 V5/V12), any authenticated session can read any of them, so
each mirror needs two switches: `GEODB_DERIVED_MIRROR` and `GEODB_DERIVED_MIRROR_PRIVATE` (`1` or `true`). The shared flag
alone never copies a personal row. Sizes measured 2026-11-06: 0, 32,252 and 10,313 rows (about 6 MB resident together).
Collection 27 is empty today; the others are the whole load.

Order. Do each step, check it, then the next. Never swap steps 2 and 4.
1. **Serve grant.** Roll geo-db serve on an image that has collections 27, 28 and 29 (geo-db final e71d8d4bbf or later),
   and mint the feeder key's binding with `derived=...,27,28,29` added. Check `geodb_derived_rows_max` has room for the
   extra ~43k rows (Stage A holds 4,194,304). Older serve builds do not refuse these writes (nothing on the wire consults
   the registry), so the roll comes first only so the rows are served by the build that knows their layout.
2. **READ-RESTRICT.** The engine's read restriction for per-user collections (W-GP's item; the grant is named
   `derived_read=<collections>` in the current proposal, which W-GP may rename) is landed, rolled, and checked: a session
   without the read grant gets a refusal on `derived_get` / `derived_scan` of 27, 28 and 29, and the readers' keys carry it.
3. **MIRROR on.** Set `GEODB_DERIVED_MIRROR=1` (with `GEODB_KEY_PATH`, `GEODB_BINDING_PATH`, `GEODB_CLIENT_BIN`) on the
   three cronjobs. With `_PRIVATE` still off this changes nothing for 27, 28 and 29. It starts the non-private mirrors.
4. **`_PRIVATE` on.** Set `GEODB_DERIVED_MIRROR_PRIVATE=1` on `not_interested_sync`, `topic_interest` (sweep and refit) and
   `feed_signals_sync`. The first run after it puts every row; collection 28 fills fully only after the nightly refit
   (the sweep puts just the last 10 minutes), so run `topic_interest refit` once by hand if you cannot wait.
5. **Check.** `geodb_derived_rows` rose by about the counts above; `geodb_derived_mirror_errors_total` for
   `external_interest_signals`, `user_topic_signals` and `feed_user_seen_daily` stays 0. Compare a user's group scan with
   Postgres for one id. Collection 29's retention prune and window replace are mirrored as deletes by the identity diff, so
   after a run the row count in geo-db equals `count(*)` of the table.

State in the manifests: steps 1 and 2 are done (serve a476ea1115 carries `derived_restricted = 27,28,29` and the scores
key's `derived=` to 29, geo-db 9b871ed4e4). `not-interested-sync` and the `topic_interest` sweep gained the geodb-client
wiring (init container, scores key, binding) and set `GEODB_DERIVED_MIRROR=1` and `GEODB_DERIVED_MIRROR_PRIVATE=1`; the
`topic_interest` refit and `feed-signals-sync` already had the shared flag and gained `_PRIVATE=1`. Merging and the
`deploy-v2` that applies these CronJobs is steps 3 and 4 in one move. If the restriction is not live, set the four
`_PRIVATE` values back to `"0"` before applying: the shared flag alone copies no personal row.

Rollback: unset `_PRIVATE` first (stops the personal copies), then the shared flag. Rows already copied stay until a
`derived_clear` of the collection (a clear can take a group, which is the per-user erasure path).

### 2f-state-and-loud. Watermark collections 6 and 7, the collection 11 backfill, and loud mirror failures

- **Collections 6 `interest_sweep_state` and 7 `trending_refresh_state`** are one-row watermarks written inside plpgsql
  functions. `topic_interest sweep` reads the row back after each run and puts it (6, key `incremental`, value `last_run_at`
  as i64 microseconds); `agenda_listings_refresh` does the same for `last_full_pass_on` (7, key `last_full_pass_on`, value
  days since 1970-01-01 as i32). Both sit behind `GEODB_STATE_MIRROR` (manifests `"0"`), on top of `GEODB_DERIVED_MIRROR`.
  The scores key's binding needs `derived=6,7` (`geodb/supervisor.sh`, applies when geodb-0 restarts); until then the puts
  are refused and now logged at error level.
- **Collection 11** is mirrored only when a block recomputes (`replace_ranking_scores`), so blocks that have not
  recomputed since the flag went on have no rows. `ranking_scores_backfill` (not scheduled) puts the whole of
  `ranks.ranking_scores`; run it once as a Job from the ranking-indexer image with the geodb-client wiring.
- **A refused or short mirror write is never silent.** `geodb_cursor::derived` logs every call's acknowledged row count
  (`geodb_derived.acknowledged`, info), logs a refusal or pipe failure at error level (`geodb_derived.mirror_failed`), and
  logs and counts a call that got fewer rows acknowledged than asked (`geodb_derived.not_acknowledged`,
  `geodb_derived_mirror_unacknowledged_total{table}`). None of them change a job's exit status. A mirror CronJob's
  `RUST_LOG` must include `geodb_cursor=info`: the lines carry that crate's target, and `ranking_indexer=info` hides them.

### 2f-scores. global_scores / local_scores / space_scores (derived collections 2, 3, 4, GEODB_DERIVED_MIRROR)

Shadow only, nothing reads the rows. The daily 03:00 UTC `scoring-service` CronJob (`scoring-service/cronjob`,
`scoring_data_writer.py`: `_write_global_scores` :89, `_write_local_scores` :112, `_write_space_scores` :146, all upserts
with no deletes) is the only writer of the three tables. With `GEODB_DERIVED_MIRROR=1` (manifest default `"0"`) it rewrites
every row into geo-db after its Postgres commit (`geodb_derived/__init__.py`, #1169): collection 2 group 0 key the 16-byte
entity id; collection 3 group the space id (u128) key entity id then space id (32 bytes); collection 4 group 0 key the
space id; score the table's `score`, no value. A geo-db error is logged, never fails the run.

- Readers (Postgres, unchanged), all inside SQL: `api/src/kg/valueOrderByScorePlugin.ts:14` and `:27` (orderBy LOCAL_SCORE
  and GLOBAL_SCORE subselects), the `entities_ordered_by_score` function (`api/drizzle/0056_entities_ordered_by_score.sql`
  :37/:43/:52/:58, redefined in `0071_vote_kind.sql:121-142`; hidden from GraphQL by `hideProceduresPlugin.ts`), and
  `geodb-feeder/src/replay.ts:64` (`local_scores` top entities per Space). search-indexer gets scores only through Kafka
  (`<ENV>.curation.scores`), not these tables.
- api shadow, `GEODB_SCORES_SHADOW` (manifest `"0"`): the api never serves a row of these tables, so there is no request
  to compare. A background timer (`GEODB_SCORES_SHADOW_INTERVAL_MS`, default 60 s; `_SAMPLE` rows per table, default 20)
  reads rows from Postgres at a random primary-key position (wrapping) and does a derived `get` for each
  (`api/src/geodb/scoresShadow.ts`). `gaia_api_scores_shadow_total{table,result}` (absent until the flag is on): match,
  mismatch (a different score), unsupported (geo-db has no row Postgres has, or the row has a value or another group),
  failed. Expect `unsupported` until the mirror has run once. **Blind spot:** only rows Postgres has are sampled, so a row
  geo-db keeps after Postgres dropped it (the mirror never deletes) is not seen; and a sample is not a full scan.
- Key: the scoring CronJob now mounts the shared `geodb-scores-key` (was an unminted `geodb-derived-key`); `supervisor.sh`
  grants it `derived=0,1,2,3,4,5,11,12,13`. Collections 2-4 were already registered in geo-db (nothing to land there).
- **Snapshot:** the CronJob's own mirror is bounded by a time budget and swallows short sends, so it is not a backfill.
  `mirror_scores_snapshot` (vote-indexer image, `scoring-service/vote-indexer/src/bin/mirror_scores_snapshot.rs`) reads the
  three tables whole in keyset pages of 256 rows, sends each as one batch request through `put_counted`, retries a short
  page three times and exits non-zero if it still cannot be acknowledged. Order: `space_scores`, `global_scores`,
  `local_scores`; `SCORES_SNAPSHOT_TABLES` narrows it. Row layout is the Python mirror's, so the nightly run and the
  snapshot write the same cells. Run it as a Job from `scoring-service/deployment/v2/scores-snapshot-job.yaml` (applied by
  hand; not in any deploy list). Counts on 2026-11-06 per plan 14: `global_scores` 1,821,435, `local_scores` 2,145,026,
  `space_scores` 1,580 (3,968,041 together). Re-count before running; this file is not a source of live numbers.
- **Enable order:** (1) the Stage B serve restart with `derived_rows_max` 12,582,912 (gaia#1267) and a binding that grants
  `derived=2,3,4` (already in `geodb/supervisor.sh`), check `geodb_derived_rows` against `geodb_derived_rows_max`;
  (2) run the snapshot Job; (3) set the flag `"1"` in `scoring-service/deployment/v2/cronjob.yaml` in its own PR (the nightly
  run then repairs drift; it never deletes); (4) the api shadow flag `GEODB_SCORES_SHADOW`. Rollback: set the flags `"0"`.

- api shadow, `GEODB_RANKING_SCORES_SHADOW` (manifest `"0"`, `api/src/geodb/rankingScoresShadow.ts`): `entity_ranking_scores` against
  the engaged-only overlay in derived collection 21 (plan 10.21 section 9). Every `GEODB_RANKING_SCORES_SHADOW_INTERVAL_MS` (60 s)
  it reads the first `_SAMPLE` (20) overlay rows and looks each up in Postgres, and reads `_SAMPLE` random Postgres rows and `get`s
  each from collection 21. `gaia_api_ranking_scores_shadow_total{result}` (absent until the flag is on): match, mismatch,
  absent_ok (un-engaged, no overlay row), missing (engaged, no overlay row), stale (overlay row for an un-engaged or absent Postgres
  row), unsupported (a value that is not 62 bytes of version 1, the old 48-byte layout), failed. Logs counts only. Turn it on only
  after the overlay writer has run once. The overlay direction reads only the highest-scored page.
  **Un-engaged entities** (`_SHADOW_DEFAULT_SAMPLE`, default 20, at most 64 a pass): it also samples random
  un-engaged Postgres rows beyond the top page, sends each one's card (property and relation counts, created time, largest
  type weight) and the ranking config to geo-db's `DerivedScores` op (`geodb-client derived pipe`, `scoresn`) and compares
  the four scores the feed reads (quality, intrinsic, type weight, ranking).
  `gaia_api_ranking_scores_default_shadow_total{result}`: match, mismatch, engaged (geo-db holds an overlay row), no_card
  (no `entities` row), failed (including a missing config row and a damaged overlay row, which geo-db refuses). A `mismatch` is
  benign when the entity's card changed since its row was last refreshed, since the row is a snapshot; a persistent
  mismatch over entities whose card has not changed is a defect in the default formula. Needs a geo-db with wire revision 11;
  an older node makes every read `failed`. Set `_DEFAULT_SAMPLE` to `0` to turn this half off.

### 2f-primer. claim_overlap_samples (derived collection 14, GEODB_DERIVED_MIRROR)

Shadow only, nothing reads the rows. The hourly `ranking-indexer-primer-claims` CronJob (`primer_claims_refresh`) is the only
writer of `public.claim_overlap_samples` (one row a day, never deleted). With `GEODB_DERIVED_MIRROR=1` (manifest default `"0"`)
it reads the whole table after its Postgres work and puts every row (`ranking-indexer/src/primer_mirror.rs`): group 0, key
`sampled_at` as a big-endian i64 of microseconds (8 bytes), score `share` (0.0 when null), value 32 little-endian bytes
(`min_votes`, `min_shared`, `users`, `anchor_set_id` or -1 as i32; `pairs`, `pairs_sharing` as i64). No deletes. Uses the shared
`geodb-scores-key`, whose binding now grants `derived=...,12,13,14,15,16,17,18,19` (`geodb/supervisor.sh`). **Needs serve built with collection 14
registered** (geo-db, after collection 13) and serve restarted for the binding; until then the key's writes are refused and logged.
Rollback: set the flag `"0"`.

Collections 15-19 (`ranking-indexer/src/primer_claims_mirror.rs`) are the rest of the family, written by the same job and flag.
A key is unique in its collection, so each key carries what the group would hold. 15 `anchor_claim_sets`: group 0, key set id
(i32 big-endian), score `effective_from` seconds, value 20 bytes (`effective_from`, `created_at` i64 us; `claim_count` i32; the
`params` jsonb is dropped). 16 `anchor_claim_members`: group set id, key set id then claim id (20 bytes), score minus the rank,
value 28 bytes (`score`, `weighted_voters`, `agree_share` f64; `rank` i32). 17 `primer_claim_stats`: group 0, key claim id,
score the claim's score, value 57 bytes (`voters`, `name_length` i32; `weighted_voters`, `weighted_agree`, `agree_share`,
`information`, `engagement` f64; `computed_at` i64 us; `contested` u8). 18 `primer_claim_topics` and 19 `primer_claim_spaces`:
group the topic or Space id, key that id then the claim id (32 bytes), score the claim's score, empty value; they replace the
`topic_ids` and `space_ids` arrays. Sets and members are only added, so they are put and never deleted. The stats table is
rebuilt each run, so the job reads its membership before the refresh and deletes every claim, (topic, claim) and (Space, claim)
key that left; if that prior read fails the stats mirror is skipped for the run (a delete list without it would be wrong).
**Needs serve built with 14-19 registered** (one batched geo-db landing).

### 2f-weights. account_weights (derived collection 5, GEODB_DERIVED_MIRROR)

Shadow only, nothing reads the rows. The hourly `ranking-indexer-account-weights` CronJob
(`ranking-indexer/src/bin/account_weights_refresh.rs`, `refresh_account_weights()` of migration 0101) is the only writer
of `public.account_weights`. With `GEODB_DERIVED_MIRROR=1` (manifest default `"0"`) it reads the table after the refresh
commits and copies the whole of it into collection 5 (`ranking-indexer/src/account_weights_mirror.rs`): group 0, key the
user id (16 bytes), score the weight, value 28 little-endian bytes (`vote_count` i32, `stance_vote_count` i32,
`first_vote_at` i64 microseconds or `i64::MIN`, `computed_at` i64 microseconds, reason codes u32: 1 excluded, 2
new_account, 4 one_sided; the reasons' `detail` text is not mirrored). Every row is a put (last write wins) and an
account the table held before the refresh and no longer has is a delete. A geo-db error is logged, never fails the Job
(CronJob processes install no metrics recorder, so read the `geodb_derived` warn lines, not counters).

- Reader (Postgres, unchanged): `api/src/pairFit/queries.ts:101` (`readAccounts`, left join) and the plpgsql join at
  `api/drizzle/0101_account_weights.sql:179`. Writer: `0101_account_weights.sql:114` (upsert) and `:136` (delete),
  called from `account_weights_refresh.rs`.
- api shadow, `GEODB_ACCOUNT_WEIGHTS_SHADOW` (manifest `"0"`, `api/k8s/v2/api.yaml`): `/internal/pair-fit` hands the
  `readAccounts` result to `api/src/pairFit/accountWeightsShadow.ts`, which at most every
  `GEODB_ACCOUNT_WEIGHTS_SHADOW_INTERVAL_MS` (10 s) reads `GEODB_ACCOUNT_WEIGHTS_SHADOW_SAMPLE` (5) of the candidates from
  collection 5 through `api/src/geodb/derived.ts` (one held `geodb-client derived pipe`, `get` only; the api's own key
  and binding, since a derived get needs an authenticated non-anonymous session, not a grant) in the background. It
  never changes, delays or fails a response. `gaia_api_account_weights_shadow_total{result}`: match, mismatch (a
  different weight, or a geo-db row Postgres lacks), unsupported (`row_missing` = mirror not run or lagging,
  `bad_layout`, `busy`), failed. `excluded` is compared by a second shadow, see §2f-exclusions. Enable only after the
  mirror has run once; expect `row_missing` until then and mismatches only for accounts changed since the last hourly
  refresh. Refusals end the pipe session (the reader starts a new one on the next read); a refusal on every read means
  the api key's binding line cannot read the derived class, which is a binding change, not an api one.
- Uses the shared `geodb-scores-key`; its binding now grants `derived=0,1,2,3,4,5,11,12,13` (`geodb/supervisor.sh`), which takes
  effect when the geodb StatefulSet restarts. Until then collection 5 and 12 writes are refused and only logged.
- Enable order: restart serve (binding), check `derived_rows_max` has room (account_weights is one row per voting
  account, small; 144 B a row resident), run the Job once by hand with the flag on to backfill (the table is mirrored in
  full each run), then set the flag `"1"` in `account-weights-cronjob.yaml` in its own PR. Rollback: set it `"0"`.

### 2f-exclusions. account_exclusions (derived collection 12, GEODB_DERIVED_MIRROR)

Shadow only, nothing reads the rows. The daily `account-exclusions-sync` CronJob
(`ranking-indexer/src/bin/account_exclusions_sync.rs`, `replace_account_exclusions()` of migration 0101) replaces the
analytics-sourced rows of `public.account_exclusions`; rows with another source (`manual`) are inserted by hand. With
`GEODB_DERIVED_MIRROR=1` (manifest default `"0"`) the sync reads the whole table after the replace commits and copies it
into collection 12 (`ranking-indexer/src/account_exclusions_mirror.rs`): group 0, key the user id (16 bytes), score 0,
value 9 bytes (source tag u8: 0 analytics, 1 manual, 2 other; `synced_at` i64 microseconds; the `reason` text is not
mirrored). Every row is a put and an account the table held before the sync and no longer has is a delete. A manual row
inserted between syncs reaches geo-db at the next sync. `--dry-run` writes and mirrors nothing. A geo-db error is logged,
never fails the Job.

- Readers (Postgres, unchanged): `api/src/pairFit/queries.ts:102` (`readAccounts`, left join) and the plpgsql join at
  `api/drizzle/0101_account_weights.sql:93` (`refresh_account_weights`). Writers: `replace_account_exclusions`
  (`0101_account_weights.sql:150`, called from `account_exclusions_sync.rs`) and hand-run `INSERT`s of manual rows.
- api shadow, `GEODB_ACCOUNT_EXCLUSIONS_SHADOW` (manifest `"0"`): meaningful only with `GEODB_ACCOUNT_WEIGHTS_SHADOW=1`.
  For each sampled account the weights shadow also reads collection 12 and compares `excluded` with whether the row
  exists. `gaia_api_account_weights_shadow_total{result}` gains no new label values; the exclusion verdict is
  `gaia_api_account_exclusions_shadow_total{result}` (match, mismatch, unsupported, failed). A mismatch right after a sync
  is expected until the next hourly `refresh_account_weights` (the flag is the refresh's, the row is the sync's).
- Uses the shared `geodb-scores-key`, now granting collection 12 (`geodb/supervisor.sh`).
- **Enable order: roll serve to a build that registers collection 12 FIRST** (geo-db 12af13b5a7 or later), then the binding
  (the StatefulSet restart does both), then run the sync once by hand with the flag on, then set the flag `"1"` in
  `account-exclusions-sync-cronjob.yaml` in its own PR. An older serve does not refuse a put to 12 (it only checks
  `collection < 64`): it holds and replays the rows but does not count them in `/metrics` or the figures, so a
  write to an older serve is invisible, not harmful. Rollback: set the flag `"0"`.

### 2f-topics. space_topic_suggestions (derived collection 13, GEODB_DERIVED_MIRROR)

Shadow only, nothing reads the rows. The hourly `ranking-indexer-topic-suggestions` CronJob
(`ranking-indexer/src/bin/topic_suggestions_refresh.rs`, `refresh_space_topic_suggestions()` of migration 0095) deletes and
re-inserts the whole table in one transaction. With `GEODB_DERIVED_MIRROR=1` (manifest default `"0"`) it reads the table after
the refresh commits and copies it into collection 13 (`ranking-indexer/src/topic_suggestions_mirror.rs`): group the Space id
as a big-endian u128, key the Space id then the topic id (32 bytes), score minus the rank (a scan walks rank 1 first), value
16 bytes (`links` i32, `rank` i32, `refreshed_at` i64 microseconds, little-endian). `name_key` is not mirrored: it is the
topic name normalised and geo-db has the name. Every row is a put; a (Space, topic) the table held before the refresh and no
longer has is a delete. A geo-db error is logged, never fails the Job. About 5,200 rows (2026-09-29), 144 B a row resident.

- Reader (Postgres, unchanged): the plpgsql `topic_suggestions_for_spaces` (`api/drizzle/0095_space_topic_suggestions.sql`),
  which joins `spaces`, `values` and `entities`. It is not an api shadow candidate until it has a card-read equivalent
  (the plpgsql-reader blocker of plan 10.11 section 3.4), so there is no compare yet.
- Uses the shared `geodb-scores-key`, now granting collection 13 (`geodb/supervisor.sh`).
- **Enable order: roll serve to a build that registers collection 13 FIRST** (geo-db commit that registers it), then the
  binding (the StatefulSet restart does both), run the Job once by hand with the flag on, then set the flag `"1"` in
  `topic-suggestions-cronjob.yaml` in its own PR. An older serve does not refuse a put to 13 (it only checks
  `collection < 64`): it holds and replays the rows but does not count them in `/metrics` or the figures. Rollback: set the
  flag `"0"`.

### 2f-agenda. trending.agenda_listings (derived collection 20, GEODB_DERIVED_MIRROR)

Shadow only, nothing reads the rows. The hourly `ranking-indexer-agenda-listings` CronJob
(`ranking-indexer/src/bin/agenda_listings_refresh.rs`, `trending.refresh_agenda_listings()` of migrations 0113 and 0118)
inserts and deletes listing rows in place. With `GEODB_DERIVED_MIRROR=1` (manifest default `"0"`) it reads the identities
before the refresh and the whole table after the commit, and copies it into collection 20
(`ranking-indexer/src/agenda_listings_mirror.rs`): group the listed entity's id as a big-endian u128, key the agenda id, the
Space id and the entity id (48 bytes, the key maximum), score the `listed_on` day as days since 1970 (a scan of one entity
walks its newest listing first), value 13 bytes (`listed_on` i32, a source mask u8, `written_at` i64 microseconds,
little-endian). The Postgres primary key also holds `source`, which would not fit the key; the mirror folds an identity's
sources into the mask (1 `block_item`, 2 `agenda_claims`, 4 `agenda_topics`, 8 `agenda_debates`). A claim listing is a
mask with a bit other than 4. Every folded row is a put; an identity the table held before and no longer has is a delete.
A geo-db error is logged, never fails the Job. Small (agendas are one per Space per day).

- Readers (Postgres, unchanged): the plpgsql `trending.claim_listings` and `trending.current_debate_agendas`
  (`api/drizzle/0113`, `0115`, `0118`). Not an api shadow candidate until they have an application equivalent
  (the plpgsql-reader blocker of plan 10.11 section 3.4).
- Uses the shared `geodb-scores-key`, now granting collection 20 (`geodb/supervisor.sh`).
- **Enable order: roll serve to a build that registers collection 20 FIRST**, then the binding (the StatefulSet restart does
  both), run the Job once by hand with the flag on, then set the flag `"1"` in `agenda-listings-cronjob.yaml` in its own PR.
  An older serve does not refuse a put to 20 (it only checks `collection < 64`) but does not count it. Rollback: flag `"0"`.

### 2f-feed-signals. personalization.feed_item_signals_hourly (derived collection 22, GEODB_DERIVED_MIRROR)

Shadow only, nothing reads the rows. The hourly `ranking-indexer-feed-signals-sync` CronJob
(`ranking-indexer/src/bin/feed_signals_sync.rs`, migrations 0109 and 0112) replaces a window of hourly buckets and expires
old ones in one transaction. With `GEODB_DERIVED_MIRROR=1` (manifest default `"0"`) it reads the identities before the run
and the whole table after the commit, and copies it into collection 22 (`ranking-indexer/src/feed_signals_mirror.rs`).
Dry runs and `unchanged` runs mirror nothing. Group the item id as a big-endian u128, score the UTC hour in epoch seconds
(a scan of one item walks its newest hour first), key the item id, the hour as a big-endian i64 and a part byte (25 bytes).
A bucket's counters are 72 bytes and a derived value holds 64, so each bucket is two rows: part 0 (48 bytes) holds twelve
little-endian i32 in this order: impressions, impressions_top3, impressions_4_10, impressions_11_30, impressions_31_plus,
opens, votes, plays, explore_plays, plays_with_duration, completed_plays, debate_claim_votes; part 1 (24 bytes) holds
watch_ms i64, explore_watch_ms i64 and watch_fraction_sum f64. Every bucket is two puts; an identity the table held before
and no longer has is a delete of both parts. A geo-db error is logged, never fails the Job.

- Size, measured 2026-11-06: 22,425 rows over 4,170 items, about 345 rows a day, 90-day item retention, so about 31 thousand
  buckets (62 thousand rows) at steady state against `derived_rows_max` 4,194,304. The whole table fits.
- Readers (Postgres, unchanged): `readCandidates` and `readPooledCandidates` in the for-you api, and
  `personalization.feed_item_signals` (migration 0109).
- Uses the shared `geodb-scores-key`, now granting collections 21 and 22 (`geodb/supervisor.sh`).
- **Enable order: roll serve to a build that registers collection 22 FIRST**, then the binding (the StatefulSet restart does
  both), run the Job once by hand with the flag on, then set the flag `"1"` in `feed-signals-sync-cronjob.yaml` in its own PR.

### 2f-cooccurrence. personalization.topic_cooccurrence (derived collection 23, GEODB_DERIVED_MIRROR)

Shadow only, nothing reads the rows. The nightly `ranking-indexer-topic-interest-refit` CronJob
(`ranking-indexer/src/bin/topic_interest.rs refit`, `personalization.refresh_topic_cooccurrence()` of migration 0100)
deletes and re-inserts the whole table in one transaction. With `GEODB_DERIVED_MIRROR=1` (manifest default `"0"`) it reads the
keys before the refit and the whole table after the commit, and copies it into collection 23
(`ranking-indexer/src/topic_cooccurrence_mirror.rs`): group the topic id as a big-endian u128, key the topic id then the
neighbour id (32 bytes), score the `similarity` (a scan of one topic walks its closest neighbour first), value 4 bytes (`support`
i32, little-endian). Every row is a put; a (topic, neighbour) pair the table held before and no longer has is a delete. A geo-db
error is logged, never fails the Job. The two-minute sweep CronJob never writes the table and carries no mirror env.

- Size: at most `spread_max_neighbours` (default 10) rows per topic, so at most 10 times the topics. `entity_topic_ranking` has
  424,701 rows, which bounds the topics, so the worst case is about 4.25M rows (about 612 MB resident at 144 B), and the realistic
  figure is a fraction of that. Under the recommended `derived_rows_max` of 8,388,608 (plan 10.14 sizing).
- Memory: the refit pod's limit is raised to 512Mi because the mirror holds the table and the prior keys in memory.
- Reader (Postgres, unchanged): the `personalization` interest spreading join in migration 0100, for the For you candidates.
- Uses the shared `geodb-scores-key`, now granting collection 23 as well (`geodb/supervisor.sh`).
- **Enable order: roll serve to a build that registers collection 23 FIRST**, then the binding (the StatefulSet restart does
  both), run the Job once by hand with the flag on, then set the flag `"1"` in `topic-interest-cronjob.yaml` in its own PR.
  Rollback: flag `"0"`.

### 2f-topology. scoring_topology_distances (derived collection 24, GEODB_DERIVED_MIRROR)

Shadow only, nothing reads the rows. The `topology-indexer` Deployment (`scoring-service/topology-indexer`, migration 0049)
consumes `CanonicalGraphDiff` events and upserts or deletes each Space's distance from the root Space. With
`GEODB_DERIVED_MIRROR=1` (manifest default `"0"`) it copies each applied change set into collection 24 after the batch
commits, and copies the whole table once at start (`mirror_snapshot`), which repairs any put the best-effort mirror dropped
(`scoring-service/topology-indexer/src/topology_mirror.rs`). Group is 0 (the scoring service loads the whole map), key is the
Space uuid (16 bytes), score is the distance as an f64, value is the distance as a little-endian i32. A Space that is removed
is a delete. A geo-db error is logged, never fails a batch.

- Size, measured 2026-11-06: 101 rows (distances 0 to 4) against `derived_rows_max` 4,194,304. The whole table fits.
- Reader (Postgres, unchanged): `_fetch_scoring_topology_distances` in the scoring-service cronjob. DR-046 may later make
  this an embedder-derived Space, in which case the mirror is retired.
- Uses the shared `geodb-scores-key`, now granting collections 23 and 24 (`geodb/supervisor.sh`).
- The topology-indexer image now builds with `geodb-cursor`; the Deployment gains the `geodb-setup` init container.
- **Enable order: roll serve to a build that registers collection 24 FIRST**, then the binding (the StatefulSet restart does
  both), roll topology-indexer once with the flag on, then set the flag `"1"` in `scoring-service/deployment/v2/topology-indexer.yaml`
  in its own PR. Rollback: flag `"0"`.

### 2f-votes-count. votes_count (derived collection 26, GEODB_DERIVED_MIRROR)

Shadow only, nothing reads the rows. The `vote-indexer` Deployment upserts absolute tallies into `votes_count` in each vote
transaction. With `GEODB_DERIVED_MIRROR=1` (manifest default `"0"`) it copies the tallies of each committed batch into
collection 26 (`scoring-service/vote-indexer/src/votes_count_mirror.rs`), and copies the whole table once at start
(`mirror_votes_counts_snapshot`), which backfills older rows and repairs any dropped put. Group the object id as a big-endian
u128, key the object id, Space id, object type and vote kind (34 bytes), score the net tally (`positive - negative`), value
24 bytes little-endian: positive i64, negative i64, `updated_at` i64 microseconds (the wall clock at the mirror write for a
batch, Postgres' own value for the snapshot). Rows are never deleted, because nothing deletes from `votes_count`. Other
writers of the table (the SQL backfills in `api/scripts`) are not mirrored; the next start's snapshot picks their rows up.
A geo-db error is logged, never fails a batch.

- Size, measured 2026-11-06: 10,090 rows over 9,545 objects (5,873 curation, 3,702 stance, 146 veracity, 368 interested,
  one relation row) against `derived_rows_max` 4,194,304, from 24,081 raw votes. The whole table fits.
- Readers (Postgres, unchanged): `api/src/kg` ordering plugins and `notification-indexer/src/storage.rs`.
- Uses the shared `geodb-scores-key`, now granting collections 25 and 26 (`geodb/supervisor.sh`).
- **Enable order: roll serve to a build that registers collection 26 FIRST**, then the binding (the StatefulSet restart does
  both), roll vote-indexer once with the flag on, then set the flag `"1"` in `scoring-service/deployment/v2/vote-indexer.yaml`
  in its own PR. Rollback: flag `"0"`. Turning the flag on also starts the `entity_ranking_scores` mirror's refresh read-backs
  if that mirror has merged, so check which mirrors the flag enables first.

### 2f-composition. entity_feed_composition_samples (derived collection 25, GEODB_DERIVED_MIRROR)

Shadow only, nothing reads the rows. The hourly `ranking-indexer-feed-composition` CronJob
(`ranking-indexer/src/bin/feed_composition_sample.rs`, migrations 0091, 0092 and 0093) inserts one window sample per
scope (the global feed and each space in `FEED_COMPOSITION_SPACE_IDS`) and one rendered sample. With
`GEODB_DERIVED_MIRROR=1` (manifest default `"0"`) it then reads the samples of the last 6 hours and copies them into
collection 25 (`ranking-indexer/src/feed_composition_mirror.rs`). Group the scope (the space id as a big-endian u128, 0
for the global feed), score the sample time in epoch seconds (a scan walks the newest sample first), key 25 bytes (a
source byte, 0 window or 1 rendered, the sample id as a big-endian i64, then a type id, all zero for the sample's own
row). A sample is a scalar row of 44 bytes (window size i32, median and oldest age i64 with `i64::MIN` for null, the
participation weight, cap and tau as f64, little-endian) plus one row per type of the composition (value the count as
i32). `type_ids` is not stored; the composition carries the types. The table is append-only, so nothing is deleted; a
geo-db error is logged, never fails the Job.

- Size: about ten samples an hour and at most four rows each, no retention, so at most 40 rows an hour, about 350
  thousand a year, against `derived_rows_max` 4,194,304 (8,388,608 planned). Check
  `SELECT count(*) FROM entity_feed_composition_samples` times four before enabling.
- Backfill: set `FEED_COMPOSITION_MIRROR_HOURS=0` on one manual run to send the whole history once, then unset it.
- Reader (Postgres, unchanged): `feed_composition_drift` (migration 0093), logged by the same Job.
- Uses the shared `geodb-scores-key`, now granting collection 25 as well (`geodb/supervisor.sh`).
- **Enable order: roll serve to a build that registers collection 25 FIRST**, then the binding (the StatefulSet restart
  applies both), run the Job once by hand with the flag on, then set the flag `"1"` in `feed-composition-cronjob.yaml` in
  its own change. Rollback: flag `"0"`.

### 2f-ranking. entity_ranking_scores (derived collection 21, GEODB_DERIVED_MIRROR)

Shadow only, nothing reads the rows. The writer is the SQL `refresh_entity_ranking_scores(uuid[])`, an upsert that never
deletes. Its callers are the vote-indexer's vote path and its `new_entity_sweep` / `comment_sweep` binaries, plus the
`api/scripts` backfills. With `GEODB_DERIVED_MIRROR=1` the vote-indexer reads the touched entities' rows right after the
call and puts them (`scoring-service/vote-indexer/src/entity_scores_mirror.rs`, called from `refresh_and_mirror`): group 0,
key the entity id (16 bytes), score `ranking_score` as the float8 the feed orders by, value 48 little-endian bytes
(`quality_score`, `intrinsic_score`, `type_weight` f64; `positive`, `negative` i64; `updated_at` i64 microseconds). A read
error is logged and counted (`geodb_derived_mirror_errors_total{table="entity_scores_readback"}`), never returned.

- Readers (Postgres, unchanged): `api/src/kg/entityOrderByRankingScorePlugin.ts` (a per-entity lookup of `ranking_score` or
  `quality_score`, ordering by the float8) and `entities_ranked_for_feed` (migration 0074 and later).
- **Sizing, read before enabling.** The table is 48.9M rows. Mirrored whole it is about 7.0 GB resident, 20 times the
  recommended `derived_rows_max` (plan 10.14 V4); a put past the cap is refused and logged. The mirror therefore sends only
  the entities a vote batch or a sweep touches and the SQL backfills are not mirrored. Do not load the whole table until
  `derived_rows_max` is sized for it or the top-N-per-group design of plan 10.14 is chosen.
- The two sweep CronJobs (`new-entity-sweep-cronjob.yaml`, `comment-sweep-cronjob.yaml`) carry no mirror env today, so only
  the vote-indexer Deployment mirrors until they are given the volumes and the flag.
- Uses the shared `geodb-scores-key`; its binding grants `derived=...,13,21` (`geodb/supervisor.sh`). **Needs serve built
  with collection 21 registered** and serve restarted for the binding; until then writes are refused and logged. Rollback:
  set the flag `"0"`.

### 2d. Raising the Space cap

`spaces` in `geodb-serve-config` caps the Spaces the store can hold; at the cap, founding refuses and the
feeder dead-letters the Space's edits as `space_cap` (§7). The cap cannot be raised in one jump on a store that
has been running: an open refuses with a bare `ResourceExhausted` when the last close kept more resident
versions than the new cap can load, and a larger cap loads fewer. Raise it in steps of 64, each a clean open and
close, with the StatefulSet scaled to 0 (a failed open does not change the store; this is a geo-db bug, fixed
upstream later):

```bash
$K scale sts/geodb --replicas=0 && $K wait --for=delete pod/geodb-0 --timeout=300s
$K apply -f geodb/k8s/v2/geodb-cap-raise-pod.yaml      # FROM/TARGET in its env
$K logs -f pod/geodb-cap-raise                         # ends "raised to <TARGET>", or "STOPPED at <cap>"
$K delete pod geodb-cap-raise
# set spaces = <TARGET> in geodb.yaml (merged, so a later deploy keeps it), apply the ConfigMap, then
$K scale sts/geodb --replicas=1
```

Never lower the cap below a founded ordinal: that open fails.

### 2e. Entity times (GEODB_ENTITY_TIMES) and the times backfill

The feeder can write each entity's `createdAt` / `updatedAt` into geo-db as two reserved properties, from the edit's
block time (contract: geo-db `docs/api/10-embedded-and-wire.md` §5.6). It is off by default: `GEODB_ENTITY_TIMES`
(`1` or `true` is on). The gateway entity shadow already compares them (`updated_at` / `created_at` leave the
uncompared reasons as soon as geo-db holds a time), so the api needs no switch, and an entity with no time row is
simply still uncompared.

Existing entities have no times until they are edited, so stamp them from Postgres `entities.created_at` /
`updated_at`. Job `geodb-times-backfill` (`geodb/k8s/v2/geodb-times-backfill-job.yaml`,
`node dist/times-backfill-main.js`) reads those two columns (read only), reads each entity back from geo-db, and
writes only the cell that is missing, in every Space where the entity has rows. It never overwrites a live value and
never creates an entity. A cursor on the state volume resumes a rerun. Like §2b it runs with the feeder scaled to 0
and refuses to start while the feeder's consumer group has members.

Order: deploy the feeder image with the flag off, run the Job, then set `GEODB_ENTITY_TIMES` to `1` and scale the
feeder back up.

Two rules for the flag and the Job:

- **Scale the feeder to 0 for the Job, always.** The Job's group check only sees members the broker currently
  lists; a feeder that is between a crash and its restart, or whose session has not yet expired, passes it. Scaling
  the Deployment to 0 and waiting for the pod to be gone (the commands below) is the real guard.
- **Never change `GEODB_ENTITY_TIMES` while a message may be half applied.** The flag changes the intents an edit
  translates to, and a resumed message continues at its saved intent index, so a flip across a crash between two
  chunks of one message would resume at the wrong place (an intent skipped or applied twice). A shutdown also
  aborts between chunks, so scaling to 0 alone is not enough. Flip it only when the feeder is idle: wait for
  `geodb_feeder_consumer_lag` near 0 (no message in flight), scale to 0, edit the manifest, scale to 1. If the
  feeder is crash-looping or was killed mid-message, let it restart under the SAME flag and catch up before
  flipping.

A rerun of the Job resumes its saved cursor and entity lists (`<data>/feeder/times-backfill/`), so it will not see
entities added to Postgres since the first run, nor a different Space set. For a clean re-list set `TIMES_FRESH=1`
in the Job (it discards the cursor and lists first; held cells are still skipped, so it only costs the read-back).
A batch geo-db refuses stops the run with the cursor not advanced and exit non-zero: fix the cause (see
`errors.jsonl`) and rerun, which retries that batch.

What the feeder does not write, and the Job does: an entity first seen only as a relation target or type, one
deleted and recreated, and one first seen while the flag was off carry no `createdAt` from the feeder (geo-db
`10-embedded-and-wire.md` §5.6, "new to the journal"). A Job run, now or later, fills them in; until then the
gateway answers those from Postgres and the shadow counts them uncompared. (Flag on first also works, but an entity the feeder sees first gets a "first seen" createdAt that
the backfill then cannot repair, because the cell is held.)

```bash
$K scale deployment/geodb-feeder --replicas=0
$K wait --for=delete pod -l app=geodb-feeder --timeout=120s
sed -e "s|geodb-feeder:latest|geodb-feeder:$TAG|" -e "s|__SPACES__|mapped|" geodb/k8s/v2/geodb-times-backfill-job.yaml | $K apply -f -
$K logs -f job/geodb-times-backfill   # "times backfill done", deadLettered 0
$K delete job geodb-times-backfill
# then GEODB_ENTITY_TIMES "1" in the feeder manifest, scale to 1.
```

Expected after both: shadow `updated_at` / `created_at` uncompared counts fall, with no `derivedMismatches` for them.
The shadow tolerates a geo-db updatedAt ahead of Postgres's by up to an hour (`TIME_LAG_SECONDS`: the feeder can run
ahead of kg-indexer, which also skips soft-deleted entities and can write out of order); a gap beyond that is a mismatch.
geo-db's updatedAt can lag Postgres's (it stamps only entities an edit changes, Postgres also stamps ones it merely
mentions), which the shadow counts as uncompared, not a mismatch; an updatedAt later than Postgres's, or a createdAt
earlier, is a real mismatch.

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
| `GeoDbFeederMembershipStalled` | `space.membership` lag above 0 for 30m | the repeating error in the feeder log; with `GEODB_RECORD_KEYS`, an `exhausted` founding of the governance Space (§2c-bis) |
| `GeoDbFeederVotesStalled` | `curation.votes` lag above 0 and nothing applied for 30m | the repeating error in the feeder log; an `exhausted` founding of the votes Space (§2c-ter) |
| `GeoDbFeederProposalVotesStalled` | `space.governance` lag above 0 and nothing handled by the proposal-votes consumer for 30m | the repeating error in the feeder log; an `exhausted` founding of the votes Space (§2c-quater) |
| `GeoDbFeederProposalActionsStalled` | `space.governance` lag above 0 and nothing handled by the proposal-actions consumer for 30m | the repeating error in the feeder log; an `exhausted` founding of the governance Space (§2c-quinquies) |
| `GeoDbFeederSubspacesStalled` | `space.trust.extensions` lag above 0 and nothing handled by the subspaces consumer for 30m | the repeating error in the feeder log; an `exhausted` founding of the governance Space (§2c-sexies) |
| `GeoDbFeederSpaceVotingSettingsStalled` | `space.governance` lag above 0 and nothing handled by the space-voting-settings consumer for 30m | the repeating error in the feeder log; an `exhausted` founding of the governance Space (§2c-septies) |
| `GeoDbFeederProposalVersionsStalled` | `space.governance` lag above 0 and nothing handled by the proposal-versions consumer for 30m | the repeating error in the feeder log; an `exhausted` founding of the governance Space (§2c-octies) |
| `GeoDbFeederProposalStatusStalled` | `space.governance` lag above 0 and nothing handled by the proposal-status consumer for 30m | the repeating error in the feeder log; an `exhausted` founding of the governance Space (§2c-novies) |
| `GeoDbFeederSpaceAddressesStalled` | `space.creations` lag above 0 and nothing handled by the space-addresses consumer for 30m | the repeating error in the feeder log; an `exhausted` founding of the governance Space (§2c-duodecies) |
| `GeoDbFeederSpaceTopicsStalled` | `space.topics` lag above 0 and nothing handled by the space-topics consumer for 30m | the repeating error in the feeder log; an `exhausted` founding of the governance Space (§2c-undecies) |
| `GeoDbFeederRanksStalled` | `knowledge.edits` lag above 0 and nothing handled by the ranks consumer for 30m | the repeating error in the feeder log; an `exhausted` write to the votes Space (§2c-decies) |
| `GeoDbFeederConsumerStartFailed` | an enabled optional consumer (`membership_keyed`, `votes`, `proposal_votes`, `proposal_actions`, `subspaces`, `space_voting_settings`, `proposal_versions`, `proposal_status`, `ranks`) threw at boot, for 10m | "consumer failed to start" in the feeder log; usually a missing Kafka ACL on its topic or group (§2c-ter step 3); fix, then restart the feeder |

### serve's own metrics and the outbox mirror (geodb-serve-alerts.yaml)

serve prints `GET /metrics` only on an HTTP listener. The supervisor opens one (`listen = http:0.0.0.0:9091 admits=anonymous routes=metrics,version`,
no grant; `routes=` (geo-db 5fbf4ad5ee) refuses every route but /metrics and /version before a session opens, and
NetworkPolicy `geodb-ingress` also limits 9091 to the `monitoring` namespace) when `GEODB_METRICS_PORT` is set; geodb.yaml sets it and exposes port `metrics` on Service `geodb`, which the
hermes-metrics ServiceMonitor scrapes (job `geodb`). The supervisor is baked into the image, so the series appear only once a
geodb image built from this `supervisor.sh` is pinned and serve is rolled (an older image ignores the variable and the target
reads down). Series: `geodb_derived_rows`, `geodb_derived_rows_max`, `geodb_derived_collection_rows{collection}`,
`geodb_derived_refused_cap_total`. Since 5fbf4ad5ee serve also prints `geodb_work_rows`, `geodb_work_rows_max` and the rest of `geodb_work_*` while the work class is on. Alerts:
`GeoDbWorkRowsHigh` (work rows above 80% of the cap),
`GeoDbDerivedRowsHigh` (rows above 80% of the cap), `GeoDbOutboxMirrorErrors` and `GeoDbOutboxMirrorDropped`
(notification-indexer, port 9464, already scraped).

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
  A founding refused as `ResourceExhausted` does not stall the feeder: the edit or membership event is dead-lettered
  to `errors.jsonl` with `"kind":"space_cap"` (and counted in `geodb_feeder_dead_letters_total{kind="space_cap"}`),
  the gaia Space is logged once ("space not founded: geo-db is at its space cap"), and the partition moves on. Such a
  Space is not retried until the feeder restarts. A `space_cap` line means geo-db never saw that edit or role event,
  so the Space is missing data until the cap is raised and it is replayed. Raising the cap is a geo-db store
  configuration question (`spaces` in `geodb-serve-config`, and serve's memory), not a feeder setting.
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
            requests: {memory: 4Gi, cpu: "500m"}   # the serve's own figures; a smaller limit is refused, see below
            limits: {memory: 6Gi}
          securityContext: {allowPrivilegeEscalation: false, capabilities: {drop: [ALL]}}
EOF
$K wait --for=condition=complete job/geodb-migrate --timeout=45m     # ~18 min here; progress: $K exec job/geodb-migrate -- wc -l /data/migrate.log
$K logs job/geodb-migrate                                              # DONE migrate spaces=<n> versions=<m>, then "swapped"
```

The Job's memory limit must be the serve's (request 4Gi, limit 6Gi), not the migrator's 606 MiB. `geodb-migrate` opens the
new root at the `medium` profile, like the serve, and that open reads the container's memory limit (cgroup `memory.max`)
and refuses a limit below the profile's floor. With a 2Gi limit the Job ended within seconds, `migrate.log` held only
`error: DefiniteRefusal`, and no `store-rev4` was created (the old store is untouched, since the script only swaps after
success). The memory is not used: the migrator peaked at 606 MiB under a 6Gi limit.

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

## 9. Snapshots: the only recovery source

geo-db has no replica, and a rebuild is not a recovery path. `testnet.knowledge.edits` keeps 7 days
(`retention.ms=604800000`), so a rebuild needs the Postgres backfills, takes about 5 h, and comes out with different
statement ids (recovery timing study, 2026-11-05). A restore from a snapshot reached serving in about 20 s and caught
up in under a minute. Two rules follow:

1. A snapshot pair is never more than about 6 days old (inside Kafka's 7 days).
2. **Take a pair after every backfill** (§2b, §2c, §2e, §2b-bis, §2b-ter) and after anything else that writes
   without going through Kafka. Those writes live only in the store and in snapshots; a snapshot from before the
   backfill restores without them (the 2026-11-05 `pretimes-*` clone lacked createdAt/updatedAt on every card).

### 9a. The scheduled pair (geodb/k8s/v2/geodb-snapshot.yaml)

CronJob `geodb-snapshot`, daily at 09:17 UTC. Each run makes one pair, `sched-<stamp>-geodb-feeder-state` then
`sched-<stamp>-data-geodb-0`, labelled `app=geodb-snapshot,snapshot-kind=scheduled,snapshot-stamp=<stamp>`. It waits
for each to be `readyToUse` (10 min timeout each) and exits non-zero otherwise, removing its own partial pair. After a
good run it deletes all but the newest 7 stamps (7 days of history). Retention selects by those labels only: the
hand-made `pre*`/`post*` snapshots are never touched.

Order is the consistency rule. The feeder writes `offsets.json` only after serve has applied the message, so the
state volume at time T1 is always behind the data at T1. Snapshotting the state first (and waiting for it) and the
data after means the saved offsets are never ahead of the saved data. A restore then replays a few edits the data
already holds, which the feeder treats as idempotent (§7), instead of skipping edits the data lacks. Do not reverse
the order and do not restore one volume of a pair without the other.

Alerts (monitoring/k8s/v2/geodb-snapshot-alerts.yaml): `GeoDbSnapshotJobFailed` (a run failed; the Job stays a
week), `GeoDbSnapshotStale` (no success for 36 h, or the CronJob is absent), `GeoDbSnapshotTooOld` (5 days, critical).

```bash
K="kubectl --context do-nyc2-geo-testnet-k8s -n gaia"
$K get cronjob geodb-snapshot
$K get volumesnapshot -l app=geodb-snapshot,snapshot-kind=scheduled --sort-by=.metadata.creationTimestamp
$K logs job/<failed job>
```

### 9b. A manual snapshot (before a risky change, and after every backfill)

Same order as the CronJob: state, wait, data, wait. Name it yourself and leave off the `snapshot-kind=scheduled`
label, so the CronJob's retention never deletes it. Delete it by hand when it has done its job.

```bash
N=postbackfill-$(date -u +%m%d%H%M)        # any prefix that is not sched-
for pvc in geodb-feeder-state data-geodb-0; do
  cat <<EOF | $K apply -f -
apiVersion: snapshot.storage.k8s.io/v1
kind: VolumeSnapshot
metadata: {name: $N-$pvc, namespace: gaia}
spec: {volumeSnapshotClassName: do-block-storage, source: {persistentVolumeClaimName: $pvc}}
EOF
  $K wait volumesnapshot/$N-$pvc --for=jsonpath='{.status.readyToUse}'=true --timeout=600s
done
```

To run the scheduled kind now instead (it will be pruned like any other): `$K create job --from=cronjob/geodb-snapshot
geodb-snapshot-$(date -u +%m%d%H%M)`.

A backfill Job holds the feeder at 0 replicas, so the state volume is quiet and the pair is consistent without any
other step. After step 5 of §2b (backfill finished, feeder still at 0) take the pair, then start the feeder.

### 9c. Restoring

Restore both volumes of one pair together, as in §8b ("After the feeder has applied edits"): scale the feeder and
`statefulset/geodb` to 0, delete PVCs `data-geodb-0` and `geodb-feeder-state`, create each again with the same name,
size (50Gi / 5Gi) and class and `dataSource: {apiGroup: snapshot.storage.k8s.io, kind: VolumeSnapshot, name:
<pair>-<pvc>}`, scale both back up. The api binding needs no change (node and sealer ids are in the store). Before
deleting anything, check the pair's age against Kafka: the feeder resumes at the saved offset, so it must still be
inside the 7-day retention (compare the pair's `offsets.json` partition offsets with the topic's low watermark). A
snapshot older than the last backfill restores without that backfill's writes; run it again (§2b).

**The restored feeder must not reuse the live consumer groups.** kafkajs resumes a group from its committed offset and
only falls back to `fromBeginning` when the group has no commit. The feeder resets a group to earliest only when
`!store.hadOffsets` (`geodb-feeder/src/main.ts`, and the same check in each keyed consumer), and a restored state
volume has `offsets.json`, so it never resets. The live group has committed past the snapshot, so a restored feeder
that joins it starts at the group's commit and silently skips every message between the snapshot's `offsets.json`
and that commit. The `offsets.json` skip (`offset <= lastDone`) is what makes a replay from earliest cheap and safe,
so the fix is to make the group start from earliest. Either of these, before the feeder starts:

1. **A fresh group id for every consumer (preferred).** Set a new, unique value on `KAFKA_GROUP_ID` and on every
   other `KAFKA_*_GROUP_ID` the feeder reads (they are all in `geodb-feeder/src/config.ts`):
   `KAFKA_MEMBERSHIP_GROUP_ID`, `KAFKA_MEMBERSHIP_KEYED_GROUP_ID`, `KAFKA_PROPOSALS_GROUP_ID`, `KAFKA_VOTES_GROUP_ID`,
   `KAFKA_PROPOSAL_VOTES_GROUP_ID`, `KAFKA_PROPOSAL_ACTIONS_GROUP_ID`, `KAFKA_SUBSPACES_GROUP_ID`,
   `KAFKA_SPACE_VOTING_SETTINGS_GROUP_ID`, `KAFKA_PROPOSAL_VERSIONS_GROUP_ID`, `KAFKA_PROPOSAL_STATUS_GROUP_ID`,
   `KAFKA_RANKS_GROUP_ID`, `KAFKA_SPACE_CREATIONS_GROUP_ID`. A consumer you leave on its default name joins the old
   group and skips the gap, so set all thirteen, even the ones whose flag is off today.

   ```bash
   S=restore-$(date -u +%Y%m%d)
   $K set env deploy/geodb-feeder KAFKA_GROUP_ID=geodb-feeder-$S KAFKA_MEMBERSHIP_GROUP_ID=geodb-feeder-membership-$S \
     KAFKA_MEMBERSHIP_KEYED_GROUP_ID=geodb-feeder-membership-keyed-$S KAFKA_PROPOSALS_GROUP_ID=geodb-feeder-proposals-$S \
     KAFKA_VOTES_GROUP_ID=geodb-feeder-votes-$S KAFKA_PROPOSAL_VOTES_GROUP_ID=geodb-feeder-proposal-votes-$S \
     KAFKA_PROPOSAL_ACTIONS_GROUP_ID=geodb-feeder-proposal-actions-$S KAFKA_SUBSPACES_GROUP_ID=geodb-feeder-subspaces-$S \
     KAFKA_SPACE_VOTING_SETTINGS_GROUP_ID=geodb-feeder-space-voting-settings-$S \
     KAFKA_PROPOSAL_VERSIONS_GROUP_ID=geodb-feeder-proposal-versions-$S KAFKA_PROPOSAL_STATUS_GROUP_ID=geodb-feeder-proposal-status-$S \
     KAFKA_RANKS_GROUP_ID=geodb-feeder-ranks-$S KAFKA_SPACE_CREATIONS_GROUP_ID=geodb-feeder-space-creations-$S
   ```

   (`set env` on the Deployment rolls the pod; do it while the feeder is still at 0 replicas, or fold the variables
   into the manifest.) The new groups start at the topic's low watermark and the feeder skips everything at or below
   its saved `offsets.json`. Delete the old groups later (`kafka-consumer-groups --delete
   --group <old>`, only once the old feeder is gone for good) and fold the new ids into the manifest so a later
   restart does not fall back to the defaults.
2. **Reset each existing group to the restored offsets** with `kafka-consumer-groups --bootstrap-server <broker>
   --command-config <client.properties> --group <group> --topic <topic> --reset-offsets --from-file <plan.csv>
   --execute` (the plan is `topic,partition,offset` where offset is `offsets.json` `done` + 1 for each partition),
   or `--to-earliest` (safe: replayed messages are skipped by `offsets.json`). The group must have no active
   members. Do this for every group above, each on its own topic, before starting the feeder.

Check it took effect before trusting the restore: the feeder logs `state loaded` with `group:`, and
`geodb_feeder_consumer_lag` (per partition) must fall toward 0 after start; a lag that is 0 within seconds of a snapshot
that is hours old means the feeder joined a committed group and skipped the gap.

## 10. hermes's cursor key (derived collection 10 only)

hermes mirrors its substreams `meta` cursor into geo-db (flag `GEODB_CURSOR_MIRROR`, off by default). It uses
its own key, `geodb-cursor-key`, not the feeder's: the binding gives it `derived=10` and nothing else
(`read=- write=- publish=- derived=10`), so a leak cannot touch the graph or any other collection. The key is
minted inside the cluster by `geodb/k8s/v2/geodb-cursor-key-job.yaml`; the private half is never printed or
copied out. Needs geo-db `d903c8e017` or later in the geodb image (`derived=`), and a geodb image built from a
gaia ref that includes the supervisor's `GEODB_CURSOR_KEYID` support (an older supervisor ignores it).

```bash
# 1. mint (idempotent: never overwrites an existing Secret)
GEODB_TAG=$(awk '/^ *- name: serve$/{s=1} s && /image: registry.digitalocean.com\/geo\/geodb:/{sub(/.*geodb:/,""); print; exit}' geodb/k8s/v2/geodb.yaml)
sed -e "s|geodb:GEODB_TAG|geodb:$GEODB_TAG|" geodb/k8s/v2/geodb-cursor-key-job.yaml | $K apply -f -
$K -n gaia wait --for=condition=complete job/geodb-cursor-key --timeout=120s
$K -n gaia logs job/geodb-cursor-key          # prints the public key id
# 2. register: restart serve so the supervisor adds the principal (brief reopen; reads shadow meanwhile)
$K -n gaia rollout restart statefulset/geodb && $K -n gaia rollout status statefulset/geodb
# 3. roll hermes so its init container and key mount pick the Secret up
$K -n gaia rollout restart deploy/hermes-pipeline deploy/hermes-ipfs-cache
# 4. only then set GEODB_CURSOR_MIRROR on in the two hermes manifests (a separate change)
```

Rotate: delete Secret `geodb-cursor-key` and Job `geodb-cursor-key`, rerun step 1, then steps 2 and 3.
Revoke: delete the Secret and restart serve; the principal disappears from the binding.

## 11. The notification-indexer's outbox key (work class only)

notification-indexer shadows every new `notification_outbox` row into geo-db's work class (plan 10.16;
flag `GEODB_OUTBOX_MIRROR`, off by default). Postgres stays the source of truth and every reader stays on it;
the shadow is put-if-absent on the outbox idempotency key, so a Kafka replay answers `duplicate` and
writes nothing. It is fire-and-forget: after the Postgres commit the indexer only `try_send`s into a bounded
queue (4096), a background task owns one held `geodb-client work pipe` child, and a failure is logged and
counted, never raised. Metrics: `geodb_outbox_mirror_{entered,duplicates,errors,dropped,skipped}_total`,
`geodb_outbox_mirror_queue`. A payload over 16 KiB is skipped, not truncated. The webhook ids shadowed are
the ones matched at insert; a webhook deleted inside the 30 s cache window is shadowed but has no Postgres
delivery row.

It uses its own key, `geodb-outbox-key`: the binding gives it `work=*` and nothing else
(`read=- write=- publish=- work=*`), so a leak can neither touch the graph nor read the outbox back. Needs a
geodb image with the `work=` binding word and `work pipe` (geo-db commit on `final` after `3fd8883208`; the
geodb-setup init container in `notification-indexer.yaml` is pinned to the older `27e919248f` and must be
bumped to it), a supervisor with `GEODB_OUTBOX_KEYID` support, and `work_rows_max` set in `serve.cfg` (the
class is off without it; size it to the outbox retention, then roll serve).

```bash
# 1. mint (idempotent: never overwrites an existing Secret)
GEODB_TAG=$(awk '/^ *- name: serve$/{s=1} s && /image: registry.digitalocean.com\/geo\/geodb:/{sub(/.*geodb:/,""); print; exit}' geodb/k8s/v2/geodb.yaml)
sed -e "s|geodb:GEODB_TAG|geodb:$GEODB_TAG|" geodb/k8s/v2/geodb-outbox-key-job.yaml | $K apply -f -
$K -n gaia wait --for=condition=complete job/geodb-outbox-key --timeout=120s
$K -n gaia logs job/geodb-outbox-key          # prints the public key id
# 2. set work_rows_max in the geodb-serve-config ConfigMap (geodb.yaml), then restart serve so the
#    supervisor adds the principal (brief reopen; reads shadow meanwhile)
$K -n gaia rollout restart statefulset/geodb && $K -n gaia rollout status statefulset/geodb
# 3. roll notification-indexer so its init container and key mount pick the Secret up
$K -n gaia rollout restart deploy/notification-indexer
# 4. only then set GEODB_OUTBOX_MIRROR to "1" in notification-indexer.yaml (a separate change)
```

Rotate: delete Secret `geodb-outbox-key` and Job `geodb-outbox-key`, rerun step 1, then steps 2 and 3.
Revoke: delete the Secret and restart serve; the principal disappears from the binding.

### Delivery-side shadow reader (delivery-worker)

delivery-worker can read the mirrored units back and compare them with what Postgres delivered (plan
10.16 section 10; flag `GEODB_OUTBOX_SHADOW_READ`, off by default). It uses the same key and the same
`work=*` principal as the indexer, so there is no new secret, and `delivery-worker.yaml` carries the same
geodb-setup init container, volumes and env. It is measurement only: it never delivers and never writes
Postgres. When a delivery reaches a terminal state in Postgres (`delivered`, or `failed` for good) the loop
`try_send`s its key and payload to a background task. That task claims units over a held
`geodb-client work pipe` (`claim`, then `complete`), pairs each with the row for the same ledger key and
webhook id, and counts `delivery_worker_shadow_total{result}`:

- `match`: payloads agree (compared as JSON, since `jsonb` reorders keys).
- `mismatch`: both exist, payloads differ.
- `miss`: Postgres delivered a row geo-db never had, 10 minutes after the delivery
  (`GEODB_SHADOW_MISS_GRACE_SECS`). Rows delivered before the mirror was switched on land here, so read the
  series from the mirror's start; afterwards a `miss` is a dropped or skipped mirror unit.
- `extra`: geo-db holds a unit Postgres never delivered, 3 days after the claim
  (`GEODB_SHADOW_EXTRA_GRACE_SECS`; it must outlast the 2 day retry backoff cap).

Also `delivery_worker_shadow_errors_total{op=claim|complete}`, `delivery_worker_shadow_dropped_total{reason}`
and the gauges `delivery_worker_shadow_pending_{units,rows}`. Memory is bounded (20 000 of each). Needs a
geodb image whose `geodb-client work pipe` knows `claim` and `complete` (geo-db `final` from "client: work
pipe takes claim, complete, retry and fail"); bump the pinned tag in `delivery-worker.yaml` to it first.
Turn it on after the indexer's mirror has been running, as a separate change.

## 12. The score writers' key (derived collections 0, 1, 11): enabling GEODB_DERIVED_MIRROR

The derived-score mirror (§2f) is wired but off: vote-indexer and ranking-indexer carry the `geodb-setup` init
container, the `geodb-key` mount and `GEODB_DERIVED_MIRROR="0"`. Both services share one key, `geodb-scores-key`;
the binding gives it `derived=0,1,2,3,4,5,11,12,13` and nothing else (`read=- write=- publish=- derived=0,1,2,3,4,5,11,12,13`; 2/3/4 are the scoring-service cronjob, §2f-scores; 5 is the account-weights CronJob, §2f-weights; 12 is the account-exclusions-sync CronJob, §2f-exclusions). The key is minted
inside the cluster by `geodb/k8s/v2/geodb-scores-key-job.yaml`, the same create-only pattern as §10; the private half
never leaves the cluster. Needs geo-db `6b34f9feda` or later (init container image, and `derived=` with a list), and a
geodb image (the serve StatefulSet) built from a gaia ref that includes the supervisor's `GEODB_SCORES_KEYID`
support, since the supervisor ships in that image. Check that serve's `derived_rows_max` is above 0 and sized for
these tables (144 B a row resident) before turning the flag on.

```bash
# 1. mint (idempotent: never overwrites an existing Secret)
GEODB_TAG=$(awk '/^ *- name: serve$/{s=1} s && /image: registry.digitalocean.com\/geo\/geodb:/{sub(/.*geodb:/,""); print; exit}' geodb/k8s/v2/geodb.yaml)
sed -e "s|geodb:GEODB_TAG|geodb:$GEODB_TAG|" geodb/k8s/v2/geodb-scores-key-job.yaml | $K apply -f -
$K -n gaia wait --for=condition=complete job/geodb-scores-key --timeout=120s
$K -n gaia logs job/geodb-scores-key          # prints the public key id only
# 2. register: restart serve so the supervisor adds the principal (brief reopen; reads shadow meanwhile)
$K -n gaia rollout restart statefulset/geodb && $K -n gaia rollout status statefulset/geodb
# 3. roll the writers so the key mount is populated (the init container runs at pod start)
$K -n gaia rollout restart deploy/vote-indexer deploy/ranking-indexer
```

4. Backfill first, with `mirror_ranking_snapshot`. The live mirror copies only rows whose score changes after the flag
   is on (§2f; it diffs a before-read against an after-read), so running `refresh_ranking_scores_for` over the
   entity set copies nothing for rows that did not move, and the rows that predate the flag never reach geo-db. The
   `mirror_ranking_snapshot` binary (vote-indexer image, `scoring-service/vote-indexer/src/bin/mirror_ranking_snapshot.rs`)
   instead reads `entity_type_ranking` (collection 0, about 1.84M rows) and `entity_topic_ranking` (collection 1, about
   0.93M rows) whole, in keyset pages of 256 rows, and sends each page through the mirror's own put path as one
   batch request (`GEODB_DERIVED_BATCH=1`, one fsync a request; needs the geodb-client at or above `6b34f9feda`).
   Run it once as a Job from `scoring-service/deployment/v2/ranking-snapshot-job.yaml` (same key mount, `geodb-setup`
   init container and `DATABASE_URL` as the vote-indexer pod; replace `IMAGE_TAG` with the vote-indexer image tag
   that contains the binary; the file is not in any deploy manifest list, so it is only ever applied by hand).
   Needs `derived_rows_max` at or above the two tables' total with headroom: it is 12,582,912 (gaia#1267), against
   about 2.77M rows. It is idempotent, because a derived row is a last-write-wins keyed cell, so a failed or
   repeated run is safe and a run that overlaps live writes converges on Postgres' current value. It logs counts
   only (progress every 100 pages, then rows sent per collection), retries a short page up to three times, and exits
   non-zero if a page still cannot be acknowledged; delete the Job and run it again. Run it before or right after
   step 5. Then diff with the type ranking gate (§2f).
   **Per-table list.** `GEODB_DERIVED_MIRROR` is one switch per service. The type and topic rankings (collections 0 and 1,
   ~1.73M rows) are written by the vote-indexer; the `ranking_scores` rows (collection 11, tens of millions) by the
   ranking-indexer, and mirroring them would pass `derived_rows_max`. `GEODB_DERIVED_MIRROR_TABLES` (comma separated
   table labels, the `table` label of the `geodb_derived_mirror_*` metrics) narrows a service to those tables. For
   the type and topic rankings only: `GEODB_DERIVED_MIRROR_TABLES=entity_type_ranking,entity_topic_ranking` on the
   vote-indexer (and the backfill Job, which then skips a table it leaves out), and on the ranking-indexer either leave
   `GEODB_DERIVED_MIRROR` at `"0"` or list a table it does not write so `ranking_scores` stays out. Unset, empty or only
   commas keeps every table (today's behaviour); a list naming no table the service writes mirrors nothing; the list never
   switches the mirror on. Rows a filtered table would have sent only raise `geodb_derived_mirror_filtered_total{table}`.
   Note the vote-indexer also mirrors `votes_count` and others: a list there drops them, so name every table you want.
5. Set `GEODB_DERIVED_MIRROR` to `"1"` in `scoring-service/deployment/v2/vote-indexer.yaml` and
   `ranking-indexer/k8s/v2/ranking-indexer.yaml` in a separate PR, so the flag change can be reverted alone.
6. Verify after the rollout: `geodb_derived_mirror_puts_total{table}` rises for `entity_type_ranking`,
   `entity_topic_ranking` and `ranking_scores`, while `geodb_derived_mirror_errors_total` and
   `geodb_derived_mirror_dropped_total` stay at 0. Postgres is unaffected either way (errors are counted, never
   returned). A non-zero error count means stop and look: usually the key is not in the binding (step 2 not done) or
   `derived_rows_max` is 0.

Rollback: set the flag back to `"0"` (or delete the Secret and restart serve and the writers). The rows stay in the
store, unread. Rotate: delete Secret and Job `geodb-scores-key`, rerun step 1, then steps 2 and 3. The init
container's image tag moves with the geodb pin; keep it at or above `6b34f9feda`.
