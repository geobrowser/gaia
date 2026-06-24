# Plan: per-webhook notification filtering

**Status:** proposal (not yet implemented). Base branch: `dev`.

## Goal

Let each `app_webhooks` row (one per app/endpoint) optionally restrict which
notifications it receives, on two independent dimensions:

- **notification type(s)** — 1..N
- **space id(s)** — 1..N

Unset/empty on a dimension = **all** (backward compatible). A webhook receives a
notification **iff it matches on both** dimensions.

> Example: a webhook that should only get membership requests + generic proposals
> for spaces A and B → `notification_types = {add_member, proposal_created}`,
> `space_ids = {A, B}`.

## Where it plugs in

Fan-out happens at one choke point — `notification-indexer/src/storage.rs:185`,
inside `insert_notifications_for_users`:

```sql
INSERT INTO notification_deliveries (outbox_id, webhook_id) SELECT $1, id FROM app_webhooks
```

Every notification × every webhook. Filtering = adding a `WHERE` here. We filter
at **fan-out** (not at delivery) so non-matching webhooks never get a delivery
row at all.

## Type taxonomy

`proposal_created` is split by action; every other event keeps its `event_type`.
Crucially, an event maps to a **set** of tokens (not one), so a proposal that does
several things filters correctly:

- **`proposal_created`** → for the actions present:
  - has an `add_member` action → token **`add_member`**
  - has an `add_editor` action → token **`add_editor`**
  - has **neither** `add_member` nor `add_editor` → token **`proposal_created`**
    (covers content publishes, topic/subspace changes, remove_member/editor, voting-settings, …)
- **every other event** → its `event_type` token: `proposal_voted`,
  `proposal_executed`, `bounty_interest`, `bounty_allocated`, `bounty_payout`,
  `bounty_created`, `proposal_comment`, `comment`, `entity_votes_threshold`.

Examples:

| Event | Token set |
|---|---|
| proposal adds a member | `{add_member}` |
| proposal adds a member **and** an editor | `{add_member, add_editor}` |
| proposal only publishes content / sets topic / removes a member | `{proposal_created}` |
| a vote | `{proposal_voted}` |

**Matching rule:** a webhook receives the event iff
`notification_types` is empty **OR** `notification_types ∩ event_token_set ≠ ∅`
(Postgres array overlap, `&&`).

> Note: these tokens are **gaia-native** (derived from `event_type` + `actions`).
> They are intentionally *not* the same vocabulary the geo-notifications webhook
> server uses downstream (`new_proposal` / `editorship_request` /
> `membership_request`) — filtering is a gaia-side concern on the raw event.

## Schema

Add two nullable array columns to `app_webhooks`
(`api/src/services/storage/schema.ts:961`, then `drizzle-kit generate` → new
`api/drizzle/00xx_*.sql`):

```ts
notificationTypes: text("notification_types").array(),  // null/empty = all types
spaceIds: uuid("space_ids").array(),                    // null/empty = all spaces
```

Additive + nullable → no backfill; existing rows keep receiving everything.

## Fan-out query

```sql
INSERT INTO notification_deliveries (outbox_id, webhook_id)
SELECT $1, id FROM app_webhooks w
WHERE (w.notification_types IS NULL OR cardinality(w.notification_types) = 0
       OR w.notification_types && $2::text[])          -- array overlap with the event's token set
  AND (w.space_ids IS NULL OR cardinality(w.space_ids) = 0
       OR $3::uuid = ANY(w.space_ids))                 -- event's space_id in the allow-list
```

- `$2` = the event's token set (`text[]`), computed once per event.
- `$3` = the event's `space_id` (from `payload.space_id`).

## Indexer changes (`notification-indexer`)

1. `models.rs`: `fn event_notification_types(event) -> Vec<String>` implementing the
   taxonomy above (reads `event_type` + the `actions` already in the payload).
2. `storage.rs`: compute the token set + `space_id` once per event in
   `insert_notifications_for_users`, bind them into the fan-out query.
3. *(optional)* persist the token set on `notification_outbox` for observability.

## Unknown-type handling — log, don't reject

- **No `CHECK`/enum.** Unknown/typo'd tokens in a filter simply never overlap any
  event's token set → they have no effect on the valid tokens (a webhook with
  `{new_proposal_typo, add_member}` still gets `add_member`). The only "receives
  nothing" case is a filter whose tokens are *all* unknown — surfaced by the log
  below.
- **Validation logs, off the hot path:** on indexer startup (and on a periodic
  tick), run `SELECT app_name, unnest(notification_types) FROM app_webhooks` and
  `error!` for any token not in the known set
  (`"unknown notification_type in webhook filter — will never match any event"`).
  The per-event fan-out stays pure SQL — we don't load arrays into app memory per
  notification, so no log spam.

## Management (setting filters)

MVP = manual SQL on the row (same place a webhook is added):

```sql
UPDATE app_webhooks
SET notification_types = ARRAY['add_member','proposal_created'],
    space_ids          = ARRAY['<uuid-A>','<uuid-B>']::uuid[]
WHERE app_name = 'my-app';
```

Clear a dimension (→ "all") with `SET notification_types = NULL`. Later: a small
admin endpoint/CLI.

## Limits

- `notification_types`: bounded by the taxonomy (~10 tokens) — no reason to exceed.
- `space_ids`: Postgres arrays are capped only by the ~1 GB field size (tens of
  millions of uuids), and we bind the event's *single* space_id, so there's no
  parameter-count limit. The real ceiling is performance: `= ANY(array)` is a
  linear scan, fine to ~low-thousands of spaces. For very large sets (>~10k),
  switch `space_ids` to a normalized `app_webhook_filters(webhook_id, space_id)`
  table with an index.

## Back-compat & edge cases

- Existing rows (NULL arrays) → unchanged (receive everything).
- No webhook matches an event → that event isn't delivered to any webhook; the
  in-app `notification_outbox` row is still created (unaffected).
- Filtering is read per fan-out → changing a webhook's filter takes effect on the
  next event, no redeploy.

## Testing

- Unit: `event_notification_types` for each event/action combo (incl. add_member +
  add_editor in one proposal; publish-only; each non-proposal event).
- Integration: seed `app_webhooks` with {1 type, many types, 1 space, many spaces,
  unset} and assert `notification_deliveries` rows are created only for matching
  webhooks.

## Rollout

Migration is additive/nullable → ship the migration first, then the indexer.
Existing webhooks keep getting everything until their arrays are populated.

## Open questions / gaps

1. **"Subscribe to ALL proposals."** Under this model an `add_member`/`add_editor`
   proposal is *not* also tagged `proposal_created`, so a webhook that wants every
   proposal must list `{proposal_created, add_member, add_editor}` (and any future
   sub-tokens). Alternative: always emit a base `proposal_created` token on *every*
   proposal_created event (layered), so `{proposal_created}` alone = all proposals
   and the action tokens narrow it. **Chosen here: mutually exclusive** (per the
   stated intent) — revisit if "all proposals" becomes a common subscription.
2. **Other proposal actions** (`remove_member`, `remove_editor`, `set_topic`,
   subspace ops, voting-settings, content publish) currently all collapse to
   `proposal_created` — not individually filterable. Easy to add tokens later
   (e.g. `remove_member`) if needed; flagging that `add_*` are the only split-out
   actions for now.
3. **Space filtering applies to all event types** (bounty/comment/vote events all
   carry a `space_id`) — confirm that's desired (assumed yes).
