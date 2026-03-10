---
title: "chore: Documentation gaps — ADR index, .env.example files, kg-indexer & ipfs docs"
type: chore
date: 2026-03-09
---

# Documentation Gaps: ADR Index, .env.example Files, kg-indexer & ipfs Docs

## Overview

Follow-up to the [documentation catalog plan](./2026-03-09-chore-documentation-catalog-and-improvement-plan.md) which completed its 4 phases. This plan addresses gaps identified in the post-completion audit: scattered decision records with no central index, missing `.env.example` files for key services, an under-documented critical pipeline service (kg-indexer), a library crate with zero docs (ipfs), and stale link cleanup.

## Context

- Decision records exist across 5 crates in 2 different formats (inline `DECISIONS.md` and standalone `decisions/0001-*.md` files). No central index exists — a new team member can't discover "why did we do X?" without knowing which crate to search.
- Three deployed services (atlas, hermes-pipeline, kg-indexer) have env vars documented in READMEs but no `.env.example` file. Other services (proposal-executor, scoring-service, search-indexer, api, hermes) already have them.
- `kg-indexer` is a critical pipeline service (Kafka → PostgreSQL) with a 57-line README that uses bullet-list env vars and is missing several env vars found in code.
- `ipfs/` is a library crate used by `hermes-ipfs-cache` with zero documentation.
- `docs/runbooks/deploying.md` is a 1-line stub that adds no value over `staging-production.md`.
- `docs/api-architecture.md` has a broken link to `docs/runbooks/monitoring.md` (deleted file).

## Acceptance Criteria

- [ ] `docs/runbooks/deploying.md` is deleted; all references updated to point to `staging-production.md`
- [ ] Broken `monitoring.md` link in `docs/api-architecture.md` is removed
- [ ] `docs/decisions/README.md` exists and indexes every ADR and RFC in the repo
- [ ] `atlas/.env.example` exists with all env vars from code
- [ ] `hermes-pipeline/.env.example` exists with all env vars from code
- [ ] `kg-indexer/.env.example` exists with all env vars from code
- [ ] `kg-indexer/README.md` is rewritten with env var table, architecture section, and file structure
- [ ] `ipfs/README.md` exists following the library README pattern
- [ ] Every `env::var("X")` call in each service's source tree has a corresponding entry in its `.env.example`
- [ ] All internal doc links resolve (no broken references introduced)

## Execution Order

Tasks are ordered by dependency — cleanup first, then `.env.example` files (referenced by later tasks), then README rewrites, then the ADR index (which links to everything).

---

### Phase 1: Stale Link Cleanup (10 min)

#### 1.1 Delete `docs/runbooks/deploying.md`

The file contains one sentence that adds nothing beyond what `docs/runbooks/staging-production.md` already covers in detail. The staging-production runbook includes deployment to staging, promotion to production, hotfix workflow, and rollback procedures.

- **Delete:** `docs/runbooks/deploying.md`
- **Update:** `README.md` line 144 — change `[Deploying](docs/runbooks/deploying.md)` to point to `docs/runbooks/staging-production.md` (or merge into the existing staging-production link)
- **Update:** `docs/api-architecture.md` line 176 — change `[Deploying](./runbooks/deploying.md)` to `[Staging & Production](./runbooks/staging-production.md)`

#### 1.2 Fix broken monitoring link

`docs/api-architecture.md` line 177 links to `docs/runbooks/monitoring.md` which no longer exists and has no replacement.

- **Remove** the monitoring line from `docs/api-architecture.md` (line 177)

---

### Phase 2: `.env.example` Files (30 min)

Create `.env.example` files for the three services that are missing them. Follow the moderate format used by `proposal-executor/.env.example`: section headers with `# === Section ===`, inline comments describing each var, placeholder hints for secrets, optional vars commented out.

**Source of truth is code, not README.** For each service, grep `env::var` calls in the source tree and cross-reference with the README to ensure completeness.

Kafka auth vars (`KAFKA_USERNAME`, `KAFKA_PASSWORD`, `KAFKA_SSL_CA_PEM`) are owned by the `hermes-kafka` shared crate and already documented in `hermes-kafka/README.md`. Do **not** duplicate them in individual service `.env.example` files — developers running against managed Kafka should reference the hermes-kafka README.

#### 2.1 Create `atlas/.env.example`

Atlas has 22 env vars documented in its README configuration table. Verify against code (`atlas/src/`).

**Expected sections:**
- **Stream Source** — `USE_MOCK`, `SUBSTREAMS_ENDPOINT`, `SUBSTREAMS_API_TOKEN`, `SUBSTREAMS_START_BLOCK`, `SUBSTREAMS_END_BLOCK`
- **Kafka** — `KAFKA_BROKER`, `KAFKA_TOPIC`
- **Space Configuration** — `ROOT_SPACE_ID`
- **Checkpoint Persistence** (optional) — `ATLAS_CHECKPOINT_DATABASE_URL`, `ATLAS_INDEXER_ID`, `ATLAS_RUNTIME_COMPATIBILITY_MARKER`, `ATLAS_CHECKPOINT_ALLOW_FRESH_START`, `ATLAS_FAIL_OPEN_BOUND`, `ATLAS_CHECKPOINT_RETRY_ATTEMPTS`, `ATLAS_CHECKPOINT_RETRY_BACKOFF_MS`, `ATLAS_PAUSE_RECOVERY_MAX_ATTEMPTS`
- **Connection Pool** (optional) — `ATLAS_CHECKPOINT_POOL_*` vars
- **Telemetry** (optional) — Sentry vars if used

#### 2.2 Create `hermes-pipeline/.env.example`

Hermes-pipeline has ~15 env vars documented in its README across three tables.

**Expected sections:**
- **Data Source** — `USE_MOCK`, `SUBSTREAMS_ENDPOINT`, `SUBSTREAMS_API_TOKEN`, `SUBSTREAMS_START_BLOCK`, `SUBSTREAMS_END_BLOCK`
- **Kafka** — `KAFKA_BROKER`, `KAFKA_MESSAGE_TIMEOUT_MS`, `KAFKA_SEND_TIMEOUT_MS`
- **Telemetry** (optional) — `SENTRY_DSN`, `SENTRY_TRACES_SAMPLE_RATE`, `SENTRY_SEND_DEFAULT_PII`, `SENTRY_ENVIRONMENT`, `SENTRY_RELEASE`, `SENTRY_DEBUG`

#### 2.3 Create `kg-indexer/.env.example`

kg-indexer has 17 env var reads in code but only 10 documented in the README. The `.env.example` must include all of them.

**Expected sections:**
- **Database** — `DATABASE_URL`
- **Kafka** — `KAFKA_BROKER`, `KAFKA_GROUP_ID`
- **Processing** — `BLOCK_STALE_TIMEOUT_MS`, `TALLY_WORKER_INTERVAL_MS`, `TALLY_WORKER_BATCH_SIZE`
- **Logging** — `LOG_EVENT_IDS`
- **Telemetry** (optional) — `SENTRY_DSN`, `SENTRY_TRACES_SAMPLE_RATE`, `SENTRY_SEND_DEFAULT_PII`, `SENTRY_ENVIRONMENT`, `SENTRY_RELEASE`, `SENTRY_DEBUG`

**Note:** `ENVIRONMENT` is set in k8s manifests but is read by `hermes-kafka::get_topic_prefix()`, not by kg-indexer code directly. Include it only if kg-indexer actually calls `get_topic_prefix()` — verify in code.

**Verification command:** `grep -rn 'env::var\|env::set\|dotenvy\|dotenv' kg-indexer/src/ | grep -v test | grep -v '//'`

---

### Phase 3: kg-indexer README Rewrite (45 min)

#### 3.1 Rewrite `kg-indexer/README.md`

The current README is 57 lines with bullet-list env vars and minimal context. Rewrite following the `atlas/README.md` gold standard pattern.

**Structure:**

1. **Title + one-liner** — "Consumes Hermes Kafka events and indexes them into PostgreSQL for the Knowledge Graph"
2. **Overview** — What it does, where it fits in the pipeline (Hermes → Kafka → kg-indexer → PostgreSQL → API). Mention block-level buffering, single-transaction writes, tally worker.
3. **Architecture** — ASCII diagram showing data flow:
   ```
   hermes-pipeline → Kafka topics → kg-indexer → PostgreSQL
                                         ↑
                                    hermes.blocks
                                    (batch close signal)
   ```
4. **Topics consumed** — Table of Kafka topics and what events they carry (already partially in current README)
5. **Configuration** — Env var table with `Variable | Required | Default | Description` columns. Include ALL vars from code (see Phase 2.3 list). Reference `.env.example`.
6. **Local Development** — `cargo run -p kg-indexer` with prerequisites (PostgreSQL, Kafka via `docker compose up` in `hermes/`)
7. **File Structure** — Brief overview of `src/` layout: `main.rs` (entry + block buffer), `consumer.rs` (Kafka), `storage.rs` (PostgreSQL), `handlers/` (event processing), `models/` (data types)
8. **Documentation** — Links to `docs/GOTCHAS.md`, `docs/DECISIONS.md`, cross-references to `hermes-pipeline`, `hermes-kafka`, `hermes-schema`

**Preserve:** Links to existing `docs/GOTCHAS.md` and `docs/DECISIONS.md`.

---

### Phase 4: ipfs README (20 min)

#### 4.1 Create `ipfs/README.md`

Follow the library README pattern from `hermes-relay/README.md`: one-liner, purpose, consumers, key types, doc links.

**Structure:**

1. **Title + one-liner** — "IPFS client for fetching GRC-20 edit content"
2. **Purpose** — Provides an IPFS gateway client abstraction with trait-based dependency injection for production and test use
3. **Consumers** — `hermes-ipfs-cache` (active). Note: `cache` also depends on this crate but is part of the sunset legacy system.
4. **Key Types** — Brief description with code example:
   - `IpfsFetcher` — trait for async IPFS fetch (get by URI, get raw bytes by CID)
   - `IpfsClient` — production implementation (HTTP gateway)
   - `MockIpfsClient` — test mock with in-memory store
   - `IpfsSource` — config enum (`Mock` / `MockBytes` / `Live`) with `into_fetcher()`
   - `IpfsError` — error types (network, decode, not found, timeout)
5. **Usage example** — Show `IpfsSource::Live { gateway_url }.into_fetcher()` pattern
6. **Design notes** — CID normalization (strips `ipfs://` prefix), uses `wire::deserialize` for GRC-20 decoding

---

### Phase 5: ADR Index (30 min)

#### 5.1 Create `docs/decisions/README.md`

Central navigational index for all decision records and RFCs in the repo. This is an index, not a copy — each entry links to the source document.

**Structure:**

```markdown
# Decision Records & RFCs

Central index of architectural decisions and design proposals across the Gaia codebase.

## How We Record Decisions

Two formats are used:
- **Inline `DECISIONS.md`** — multiple ADRs in one file, used for crate-specific decisions
- **Standalone files in `decisions/`** — one file per decision, used for detailed analyses with options considered

New decisions should follow whichever format the crate already uses. For new crates, prefer inline `DECISIONS.md` unless the decision warrants a detailed options analysis.

## Service Decision Records

| Crate | ID | Title | Status | Link |
|---|---|---|---|---|
| hermes-pipeline | ADR-001 | Event sequencing for cross-topic ordering | Accepted | [link](../hermes-pipeline/docs/DECISIONS.md) |
| kg-indexer | ADR-001 | Per-message processing instead of cross-message batching | Superseded | [link](../kg-indexer/docs/DECISIONS.md#adr-001) |
| kg-indexer | ADR-002 | Block-level buffering for cross-topic ordering | Accepted | [link](../kg-indexer/docs/DECISIONS.md#adr-002) |
| hermes-ipfs-cache | 0001 | Cursor persistence strategy | Accepted | [link](../hermes-ipfs-cache/docs/decisions/0001-cursor-persistence.md) |
| hermes-relay | 0001 | Multiple substreams modules consumers | Accepted | [link](../hermes-relay/docs/decisions/0001-multiple-substreams-modules-consumers.md) |
| hermes-schema | 0001 | Wrapper messages for multi-event topics | Accepted | [link](../hermes-schema/docs/decisions/0001-wrapper-messages-for-multi-event-topics.md) |

## Cross-cutting RFCs

| ID | Title | Link |
|---|---|---|
| RFC-0001 | Canonical graph inputs | [link](rfcs/0001-canonical-graph-inputs.md) |
| RFC-0002 | Graph diff emission | [link](rfcs/0002-graph-diff-emission.md) |
| RFC-0003 | Context-aware versioned diffs | [link](rfcs/0003-context-aware-versioned-diffs.md) |

## Related Architecture Documents

- [System Architecture](architecture.md)
- [API Architecture](api-architecture.md)
- [proposal-executor Architecture](../proposal-executor/ARCHITECTURE.md)
- [search-indexer-repository Architecture](../search-indexer-repository/ARCHITECTURE.md)
```

**Note:** Verify all anchor links (`#adr-001`, `#adr-002`) resolve correctly in the inline DECISIONS.md files. GitHub auto-generates anchors from headings.

#### 5.2 Link ADR index from root README

Add a link to `docs/decisions/README.md` in the root README's Documentation section under "Architecture & Design".

---

## Out of Scope

- **CI/CD documentation** — still a separate effort (46 workflows)
- **CONTRIBUTING.md** — separate plan
- **CLAUDE.md** — separate plan
- **Additional runbooks** (incident response, Kafka ops, database ops) — separate plan
- **Updating the brainstorm coverage matrix** — the brainstorm is historical context, not a living doc
- **Enforcing minimum docs standard** — gold standards are documented, not mandated
- **Env var completeness for hermes-kafka consumers** — Kafka auth vars are documented in hermes-kafka/README.md; individual services reference that README

## Verification

After completing all phases:

1. **Broken links:** `grep -rn 'deploying\.md\|monitoring\.md' --include='*.md'` returns nothing
2. **ADR completeness:** Every file matching `*/DECISIONS.md` or `*/decisions/*.md` has a corresponding entry in `docs/decisions/README.md`
3. **Env var completeness:** For each of atlas, hermes-pipeline, kg-indexer: `grep -rn 'env::var' <service>/src/ | grep -v test | grep -v '//'` — every var appears in `.env.example`
4. **Link check:** All internal links in new/modified docs resolve to existing files
5. **Pattern compliance:** New READMEs follow the gold standard patterns (atlas for services, hermes-relay for libraries)

## References

- **Previous plan:** `docs/plans/2026-03-09-chore-documentation-catalog-and-improvement-plan.md`
- **Brainstorm:** `docs/brainstorms/2026-03-09-documentation-catalog-and-improvement-plan-brainstorm.md`
- **Gold standards:** `atlas/README.md` (service), `hermes-relay/README.md` (library), `proposal-executor/.env.example` (env example)
- **ADR formats:** `hermes-pipeline/docs/DECISIONS.md` (inline), `hermes-ipfs-cache/docs/decisions/0001-cursor-persistence.md` (standalone)
