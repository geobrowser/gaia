---
title: fix: Resolve Space.page from topicId
type: fix
status: completed
date: 2026-05-09
---

# fix: Resolve Space.page from topicId

## Summary

Update the PostGraphile `Space.page` computed field so it returns the entity referenced by `spaces.topic_id` when a topic is set, and falls back to the legacy relation-derived front-page lookup only when `topic_id` is null. The implementation should make topic identity authoritative while preserving legacy behavior for spaces without a topic, with deterministic regression coverage for both SQL helper and GraphQL behavior.

---

## Problem Frame

`Space.page` currently resolves through `spaces_page(space spaces)`, which finds an entity with a `Types -> SPACE_TYPE` relation in the queried space. This can return a different entity than `topicId`, even though the protocol docs say a space topic represents the space and determines the front page.

The reported query shows that mismatch directly: `topicId` is `54e749de037947ea9ce1d018d3019a06`, while `page.id` resolves to `b0e7b1ab3a15466484a19239b7085c08`.

---

## Requirements

- R1. `spaces { page { id } topicId }` must return `page.id === topicId` whenever `topicId` is set.
- R2. `Space.page` must fall back to the legacy `Types -> SPACE_TYPE` front-page relation when `topicId` is `null`.
- R3. `Space.page` must return `null` when both `topicId` is `null` and no legacy front-page relation exists.
- R4. The public GraphQL field name and shape must remain unchanged: clients keep querying `page { ... }`.
- R5. The implementation must be covered by deterministic fixtures that prove `topicId` wins over an existing legacy relation-derived page.

---

## Scope Boundaries

- Do not change how space topics are written by the indexers or proposal execution paths.
- Do not remove or migrate old `Types -> SPACE_TYPE` relation data; it remains the fallback when a space has no topic.
- Do not rename GraphQL fields or add a parallel `topic`/`topicPage` API.
- Do not refactor unrelated PostGraphile plugins or computed-column functions.

---

## Context & Research

### Relevant Code and Patterns

- `api/drizzle/0004_functions.sql` defines `public.spaces_page(space spaces)` as the PostGraphile computed field behind `Space.page`.
- `api/src/kg/__tests__/spaces-helpers.test.ts` already tests the `spaces_page` helper, but the current cases are data-dependent and assert the old relation-derived behavior.
- `api/src/kg/postgraphile.ts` uses PostGraphile 4.14.1 with computed columns and the undashed UUID scalar plugin; SQL-level UUID joins can stay dashed while GraphQL responses are dashless.
- `api/src/services/storage/schema.ts` defines `spaces.topicId` as a foreign key to `entities.id` and indexes it with `spaces_topic_id_idx`.
- `api/src/services/storage/schema.ts` already has a Drizzle relation named `topic` from `spaces.topicId` to `entities.id`, confirming topic identity is a first-class space relationship in the storage model.
- `api/src/kg/__tests__/entitySpaceFilterPlugin.test.ts` and `api/src/kg/__tests__/entitiesOrderedByScore.test.ts` show the local pattern for GraphQL integration tests through `graphqlServer.fetch`.

### Institutional Learnings

- `docs/gotchas.md` says custom PostGraphile resolvers are implemented as Postgres stored procedures in Drizzle migrations.
- `docs/protocol/knowledge-graph-ontology.md` states that spaces set a topic representing what the space is about and that the topic determines the space's front page; it also states topic values are onchain, not knowledge-graph relations.

### External References

- External research was skipped because this follows established local PostGraphile stored-procedure and Vitest patterns.

---

## Key Technical Decisions

- Keep the fix database-side in `spaces_page`: PostGraphile already exposes `Space.page` from this SQL function, so redefining the function preserves the public GraphQL contract without adding a TypeScript resolver layer.
- Use `spaces.topic_id` as the authoritative source when present: protocol documentation and storage schema both treat topic identity as the front-page source, while the old relation-derived lookup remains a compatibility fallback only for spaces without a topic.
- Add conflict-based regression fixtures: the core bug only appears when the legacy relation-derived page and `topic_id` disagree, so tests must seed that disagreement explicitly.
- Preserve legacy null-topic semantics: if `topic_id` is null, return the legacy relation-derived page when one exists; otherwise return null.

---

## Open Questions

### Resolved During Planning

- Should this add a new GraphQL field instead of changing `page`? No. The user asked for `page` to resolve from `topicId`, and keeping the field preserves client contracts.
- Should legacy `Types -> SPACE_TYPE` relation data remain a fallback? Yes, but only when `topicId` is not set. When `topicId` exists, it must win over the legacy relation.

### Deferred to Implementation

- Exact fixture IDs: choose deterministic UUIDs that do not collide with existing test fixtures and clean them up idempotently before and after the tests.

---

## Implementation Units

### U1. Redefine the Space.page SQL Resolver

**Goal:** Make `public.spaces_page(space spaces)` return the topic entity when `space.topic_id` is set, otherwise preserve the existing legacy relation-derived page lookup.

**Requirements:** R1, R2, R3, R4

**Dependencies:** None

**Files:**
- Create: `api/drizzle/0057_fix_spaces_page_topic.sql`
- Modify: `api/drizzle/meta/_journal.json`
- Create/Modify: `api/drizzle/meta/0057_snapshot.json` if the repository's migration workflow requires a new snapshot for this function-only migration
- Test: `api/src/kg/__tests__/spaces-helpers.test.ts`

**Approach:**
- Add a migration that redefines `public.spaces_page(space spaces)` rather than editing only the historical `0004_functions.sql` migration.
- Keep the function signature and return type unchanged so PostGraphile continues to expose `Space.page`.
- Prefer an entity lookup keyed by `space.topic_id`.
- Preserve the existing relation-derived lookup as the fallback branch when `space.topic_id` is null.
- Let SQL return `null` when `space.topic_id` is null and no legacy front-page relation exists; with the FK in place, a missing topic entity should only matter for inconsistent local fixtures or pre-FK databases.

**Execution note:** Start with regression coverage that fails against the current relation-derived implementation before changing the function.

**Patterns to follow:**
- `api/drizzle/0004_functions.sql` for computed-field function shape and `LANGUAGE sql STABLE`.
- `api/drizzle/0056_entities_ordered_by_score.sql` for adding a later function migration instead of modifying historical migration intent.

**Test scenarios:**
- Happy path: given a space with `topic_id` set to entity A and no legacy front-page relation, calling `spaces_page(space)` returns entity A.
- Edge case: given a space with `topic_id` set to entity A and a legacy `Types -> SPACE_TYPE` relation pointing entity B, calling `spaces_page(space)` returns entity A, not entity B.
- Edge case: given a space with `topic_id = NULL` and a legacy `Types -> SPACE_TYPE` relation pointing entity B, calling `spaces_page(space)` returns entity B.
- Edge case: given a space with `topic_id = NULL` and no legacy `Types -> SPACE_TYPE` relation, calling `spaces_page(space)` returns `NULL`.
- Integration: after applying migrations to a test database, PostGraphile can still introspect and expose `page` on `Space`.

**Verification:**
- SQL-level tests prove `spaces_page` follows `spaces.topic_id` when present and uses the legacy relation only when `topic_id` is null.
- The existing `Space.page` GraphQL field remains available without schema shape changes.

### U2. Add GraphQL Contract Regression Coverage

**Goal:** Prove the reported GraphQL query returns `page.id` equal to `topicId` through the real Yoga/PostGraphile server.

**Requirements:** R1, R2, R3, R4, R5

**Dependencies:** U1

**Files:**
- Modify: `api/src/kg/__tests__/spaces-helpers.test.ts`
- Reference: `api/src/kg/postgraphile.ts`

**Approach:**
- Extend or rewrite `spaces-helpers.test.ts` to seed deterministic `entities`, `spaces`, and conflicting legacy `relations` rows.
- Use the same fixture cleanup pattern as existing API tests: delete by known UUIDs before seeding and after teardown.
- Add a GraphQL query equivalent to the reported shape: `spaces(filter: { id: { is: ... } }) { page { id } topicId }`.
- Compare GraphQL values in dashless form, matching the API's UUID scalar behavior.

**Patterns to follow:**
- `api/src/kg/__tests__/entitySpaceFilterPlugin.test.ts` for executing GraphQL requests through `graphqlServer.fetch`.
- `api/src/kg/__tests__/entitiesOrderedByScore.test.ts` for deterministic fixture IDs and idempotent cleanup.
- `docs/gotchas.md` for dashless UUID expectations in PostGraphile responses.

**Test scenarios:**
- Happy path: querying a seeded space with `topicId` set returns one space whose `page.id` equals `topicId`.
- Edge case: when the same seeded space also has a legacy relation-derived front-page entity, the GraphQL `page.id` still equals `topicId`, not the legacy entity ID.
- Edge case: querying a seeded space with `topicId = null` and a legacy relation returns the legacy page entity.
- Edge case: querying a seeded space with `topicId = null` and no legacy relation returns `page: null`.
- Integration: the filter form from the user report accepts the dashless space ID and resolves through the real PostGraphile schema without errors.

**Verification:**
- The GraphQL regression fails against the current resolver and passes after U1.
- The test protects the public client-facing contract, not only the helper function.

---

## System-Wide Impact

- **Interaction graph:** The change affects PostGraphile `Space.page` reads only; it does not touch write/indexer paths.
- **Error propagation:** No new error modes are expected because the resolver remains a stable SQL computed field returning an entity or null.
- **State lifecycle risks:** Existing spaces with `topic_id = NULL` continue using old front-page relation data; spaces with `topic_id` set switch to topic-authoritative behavior.
- **API surface parity:** The `page` field's meaning changes to match `topicId`; field name, nesting, and UUID scalar behavior stay unchanged.
- **Integration coverage:** SQL helper coverage is not enough because the user-facing failure is GraphQL; U2 covers the Yoga/PostGraphile path.
- **Unchanged invariants:** `topicId` filtering, storage relations, indexer writes, and subspace topic behavior remain outside the change.

---

## Risks & Dependencies

| Risk | Mitigation |
|------|------------|
| Clients depended on legacy relation-derived `page` when `topicId` was null | Preserve this behavior as the explicit fallback path |
| Function-only migration is not captured by Drizzle schema generation | Follow the repository's existing custom SQL migration pattern and verify the migration is present in the journal used by deployments |
| Regression tests accidentally pass because fixture topic and legacy page are the same entity | Seed distinct topic and legacy entities and assert the legacy entity is not returned |
| GraphQL UUID formatting causes false test failures | Normalize expected UUIDs to dashless strings before comparing GraphQL response values |
| Test fixture cleanup violates FK order | Delete relations and spaces before deleting seeded entities, and keep cleanup scoped to known fixture IDs |

---

## Documentation / Operational Notes

- No public documentation update is required if existing docs already define topic as the front-page source.
- PR notes should call out that `topicId` now takes precedence, while spaces without `topicId` keep the legacy front-page fallback.
- Deployment only requires applying the API database migration before serving the updated schema.

---

## Sources & References

- Related code: `api/drizzle/0004_functions.sql`
- Related code: `api/src/kg/__tests__/spaces-helpers.test.ts`
- Related code: `api/src/kg/postgraphile.ts`
- Related code: `api/src/services/storage/schema.ts`
- Related docs: `docs/protocol/knowledge-graph-ontology.md`
- Related docs: `docs/gotchas.md`
