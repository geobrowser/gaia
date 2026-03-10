---
title: "chore: Documentation catalog and improvement plan"
type: chore
date: 2026-03-09
brainstorm: docs/brainstorms/2026-03-09-documentation-catalog-and-improvement-plan-brainstorm.md
---

# Documentation Catalog & Improvement Plan

## Overview

Make the Gaia repo easy to onboard into by redesigning the root README as a progressive disclosure hub, filling critical documentation gaps, and archiving stale docs. Primary audience: new team members joining a growing team (5-15 people, regular onboarding).

## Context

- The repo has strong docs in depth (architecture, RFCs, ADRs, gotchas, specs) but weak discoverability
- Gold standard patterns exist in atlas, proposal-executor, and hermes-pipeline — they should be replicated, not reinvented
- Legacy crates (indexer, cache, wire, stream, indexer_utils) are sunset — skip them
- Hermes has fully replaced the legacy system — the root README describes an architecture that no longer exists
- People run individual services locally, with Kafka + Kafka UI via docker-compose in `hermes/`

## Acceptance Criteria

- [ ] Root README accurately describes the current Hermes-based architecture and serves as a progressive disclosure hub
- [ ] `api/` has a README with setup instructions and links to existing architecture docs
- [ ] `api/.env.example` exists with documented env var names
- [ ] All references to `docs/hermes-architecture.md` are fixed across the repo (8 files, not just root README)
- [ ] `hermes-relay/` and `hermes-kafka/` have READMEs
- [ ] Completed/stale plans are archived to `docs/archive/plans/`
- [ ] Each new README follows gold standard patterns from the repo

## Execution Order

Tasks are ordered by dependency — each builds on the previous.

---

### Phase 1: Archive Stale Plans (30 min)

#### 1.1 Archive stale plans
- **Create:** `docs/archive/plans/`
- **Move criteria:** Plans without a `2026-*` date prefix, plus any dated plan whose feature is confirmed shipped. When uncertain, leave in place.
- Review `docs/plans/` (19 files) and move completed/stale ones
- Update any cross-references that point to moved files (check root README, crate docs)

---

### Phase 2: Root README Rewrite (P0)

#### 2.1 Fix all broken references to `docs/hermes-architecture.md`

The broken link exists in **8 files** across the repo, not just the root README:
- `README.md`
- `atlas/README.md`
- `hermes/README.md`
- `hermes-pipeline/README.md`
- `hermes-pipeline/docs/plans/0001-complete-action-support.md`
- `hermes-pipeline/docs/plans/0002-edits-published-support.md`
- `hermes-ipfs-cache/docs/decisions/0001-cursor-persistence.md`

Change all to `docs/architecture.md`. Use `grep -r 'hermes-architecture' --include='*.md'` to catch any missed references.

#### 2.2 Rewrite `README.md` as progressive disclosure hub

The root README is the single most impactful doc change. It replaces the stale legacy description with the current Hermes architecture and serves as the navigational entrypoint.

**Structure (3 levels of progressive disclosure):**

**Level 1 — "What is this?" (first 5 min)**
- One-paragraph description of Gaia (knowledge graph data service for the Geo protocol)
- System architecture diagram (ASCII, like `docs/architecture.md` and `atlas/README.md` use)
- Map of **entry-point subsystems** grouped by domain — keep this to 5-6 groups (Hermes pipeline, Indexers, Search, API/Services, Infrastructure) rather than listing every crate. Link to individual crate READMEs for details. A smaller table is more likely to stay accurate.

**Level 2 — "How do I run it?" (first 30 min)**
- Prerequisites (Rust, Bun, PostgreSQL, Docker)
- Local development pattern: individual services, not a monolithic docker-compose
  - Kafka + Kafka UI: `docker compose up` in `hermes/`
  - Each Rust service: `cargo run -p <crate>` with relevant env vars
  - API: `bun install && bun run dev` in `api/`
- Link to each service's README for service-specific setup
- Verify each command works locally before writing it into the README

**Level 3 — "Where do I go deeper?" (links by category)**
- Architecture docs, protocol docs, specs, RFCs, runbooks, gotchas, decision records, research
- Documentation conventions note: `docs/` = cross-cutting system docs; `<crate>/docs/` = crate-specific docs; `<crate>/README.md` = crate entry point

**Legacy section:** Collapsed `<details>` block titled "Legacy System (pre-Hermes)" with a brief note that `indexer/`, `cache/`, `wire/`, `stream/`, `indexer_utils/` are sunset.

**Sustainability note:** Include a line in Level 3: "When adding a new crate, update the subsystem table above and add a crate README."

**Pattern to follow:** Atlas README structure (one-liner → overview → local dev → config → architecture diagram → domain concepts → doc links). See `atlas/README.md`.

---

### Phase 3: API README (P0)

#### 3.1 Write `api/README.md`

The API is the primary consumer-facing service and currently has no top-level documentation.

Follow the atlas README pattern. Must include: setup with `.env.example`, `bun run dev` instructions, and links to existing `docs/api-architecture.md`, `api/docs/database-configuration.md`, and `api/src/services/search/QUERY_ARCHITECTURE.md`.

#### 3.2 Create `api/.env.example`
- Create from scratch using env var names only — do NOT copy `api/.env` and redact
- List each variable with a comment describing its purpose and a placeholder value
- Match the `.env.example` pattern used by `scoring-service/`, `proposal-executor/`, `search-indexer/`

---

### Phase 4: Library READMEs (P1)

Libraries need a different README template than services: purpose, public API surface, consumers, and design decisions (no "how to run" or deployment).

#### 4.1 Write `hermes-relay/README.md`

`hermes-relay` already has excellent inline docs (225 `//!` lines) and `docs/decisions/`. Write a short pointer README: one-liner, purpose, consumers (find via `grep -r 'hermes-relay' */Cargo.toml`), link to module-level docs and `docs/decisions/`.

#### 4.2 Write `hermes-kafka/README.md`

`hermes-kafka` has no documentation at all. Write a short README: one-liner, purpose, consumers, key types, Kafka configuration.

---

## Out of Scope

These are documented gaps from the brainstorm that are intentionally deferred:

- **CI/CD documentation** — P1 in brainstorm but large scope (46 workflows). The staging-production runbook already maps services to workflows. **Follow-up:** Create a separate plan for CI/CD docs within 2 weeks of completing this plan.
- **CONTRIBUTING.md** — P2, deferred. The staging-production runbook already covers branch strategy and deployment workflow.
- **CLAUDE.md** — P2. Primary audience is humans.
- **Additional runbooks** — P2. More runbooks (incident response, Kafka ops, database ops) would help but are a separate effort.
- **`ipfs/` README** — P2. Has good inline docs but no entry point.
- **Minimum docs standard per crate** — P2, deferred per brainstorm decision. Gold standard patterns are documented but not mandated.
- **rustdoc (`cargo doc`)** — Open question from brainstorm. Not in scope.
- **GitHub issue/PR templates** — P3.
- **CHANGELOG** — P3.

## Verification

After completing all phases:

1. **Links check:** All documentation links across the repo resolve (`grep -r 'hermes-architecture' --include='*.md'` returns nothing)
2. **Onboarding test:** A team member unfamiliar with the new docs can answer from the root README alone: "What does Gaia do?", "How do I run the API locally?", "Where are the architecture docs?"
3. **API setup test:** A developer can set up and run the API locally using only the api/ README instructions
4. **No broken references:** Archived plans don't leave dangling links

## References

- **Brainstorm:** `docs/brainstorms/2026-03-09-documentation-catalog-and-improvement-plan-brainstorm.md`
- **Gold standards:** `atlas/README.md` (service README), `proposal-executor/ARCHITECTURE.md` + `RUNBOOK.md` (architecture + operations), `hermes-pipeline/README.md` (comprehensive README)
- **System architecture:** `docs/architecture.md`
- **API architecture:** `docs/api-architecture.md`
- **Staging/production runbook:** `docs/runbooks/staging-production.md`
