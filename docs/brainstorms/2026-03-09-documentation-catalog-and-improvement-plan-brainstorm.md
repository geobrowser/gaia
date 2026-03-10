# Documentation Catalog & Improvement Plan

**Date:** 2026-03-09
**Status:** Brainstorm
**Goal:** Catalog existing documentation, identify gaps, and create a prioritized plan to make the repo easy to onboard into — with progressive disclosure from the root README downward.

## Context

Gaia is a ~30-crate Rust workspace plus TypeScript/Python services. The team is growing (5-15 people) with regular onboarding. Current onboarding path: root README → `docs/` → figure it out. Legacy crates (indexer, cache, wire, stream, indexer_utils) are actively being sunset and excluded from this effort.

## What We're Building

A **documentation taxonomy** that catalogs every doc type across the repo, assesses coverage, and identifies gaps — plus a plan to redesign the **root README as a progressive disclosure entrypoint** that guides new team members from "what is this?" to "how do I work on X?".

## Key Decisions

1. **Taxonomy-first approach** — catalog by doc type across crates, with coverage matrix
2. **Root README as progressive disclosure entrypoint** — not just a setup guide, but a navigational hub
3. **Skip legacy crates** — indexer, cache, wire, stream, indexer_utils are sunset
4. **Primary audience: new team members** — optimize for humans onboarding, not external contributors

---

## Documentation Taxonomy

### Doc Types Identified

| Doc Type | Purpose | Where It Lives | Gold Standard Example |
|----------|---------|----------------|----------------------|
| **Root README** | System overview, getting started, navigation hub | `README.md` | — (needs redesign) |
| **Crate/Service README** | What it does, how to run it, key concepts | `<crate>/README.md` | [`atlas/README.md`](../../atlas/README.md) |
| **Architecture Doc** | System/component design, data flow, diagrams | `docs/architecture.md` or `<crate>/docs/ARCHITECTURE.md` | [`docs/architecture.md`](../architecture.md), [`proposal-executor/ARCHITECTURE.md`](../../proposal-executor/ARCHITECTURE.md) |
| **ADR (Decision Record)** | Why we chose X over Y | `<crate>/docs/DECISIONS.md` | [`hermes-pipeline/docs/DECISIONS.md`](../../hermes-pipeline/docs/DECISIONS.md) |
| **Gotchas** | Non-obvious pitfalls, operational traps | `<crate>/docs/GOTCHAS.md` or `docs/gotchas.md` | [`docs/gotchas.md`](../gotchas.md) |
| **Spec** | Formal behavioral specification | `docs/specs/` | [`docs/specs/atlas-canonical-graph-spec.md`](../specs/atlas-canonical-graph-spec.md) |
| **RFC** | Proposed design changes | `docs/rfcs/` | [`docs/rfcs/0001-canonical-graph-inputs.md`](../rfcs/0001-canonical-graph-inputs.md) |
| **Runbook** | Operational procedures (deploy, debug, incident) | `docs/runbooks/` or `<service>/RUNBOOK.md` | [`proposal-executor/RUNBOOK.md`](../../proposal-executor/RUNBOOK.md) |
| **Plan** | Implementation plans (time-bounded) | `docs/plans/` or `<crate>/docs/plans/` | Many examples in both locations |
| **Research** | Exploratory analysis, feasibility studies | `docs/research/` | [`docs/research/sharding.md`](../research/sharding.md) |
| **Protocol Doc** | Smart contract / protocol behavior reference | `docs/protocol/` | [`docs/protocol/actions.md`](../protocol/actions.md) |
| **Rustdoc (`//!` / `///`)** | Module-level and item-level inline docs | In source files | `hermes-relay` (225 `//!` lines), `search-indexer-repository` (673 `///` lines) |
| **Test Guide** | How to run tests, what to test, test matrix | `<crate>/docs/TESTING.md` or inline | [`search-indexer/docs/TESTING.md`](../../search-indexer/docs/TESTING.md) |

---

## Coverage Matrix: Active Crates & Services

Legend: ✅ exists | ⚠️ minimal/stale | ❌ missing | ➖ not applicable

### Hermes Ecosystem

| Crate | README | Architecture | ADRs | Gotchas | Inline Docs | Test Guide |
|-------|--------|-------------|------|---------|-------------|------------|
| hermes-pipeline | ✅ | ✅ (in README) | ✅ | ✅ | ✅ excellent | ➖ |
| hermes-relay | ❌ | ❌ | ✅ | ❌ | ✅ good | ➖ |
| hermes-ipfs-cache | ✅ | ✅ | ✅ | ❌ | ⚠️ | ➖ |
| hermes-schema | ✅ | ✅ (in README) | ✅ | ❌ | ✅ good | ➖ |
| hermes-instrumentation | ✅ | ➖ | ➖ | ➖ | ✅ good | ➖ |
| hermes-substream | ✅ | ➖ | ➖ | ➖ | ✅ good | ➖ |
| hermes-kafka | ❌ | ❌ | ❌ | ❌ | ⚠️ | ➖ |
| hermes/ (infra) | ✅ | ✅ | ➖ | ➖ | ➖ | ➖ |

### Atlas

| Crate | README | Architecture | ADRs | Gotchas | Inline Docs | Test Guide |
|-------|--------|-------------|------|---------|-------------|------------|
| atlas | ✅ | ✅ (13 docs!) | ➖ | ✅ | ✅ excellent | ➖ |

### Search System

| Crate | README | Architecture | ADRs | Gotchas | Inline Docs | Test Guide |
|-------|--------|-------------|------|---------|-------------|------------|
| search-indexer | ✅ | ➖ | ➖ | ➖ | ✅ good | ✅ |
| search-indexer-repository | ✅ | ✅ | ➖ | ➖ | ✅ excellent | ➖ |
| search-indexer-shared | ✅ | ➖ | ➖ | ➖ | ⚠️ | ➖ |
| search-admin | ✅ | ➖ | ➖ | ➖ | ⚠️ | ✅ |
| search-indexer-deploy | ✅ | ➖ | ➖ | ➖ | ➖ | ✅ |

### KG Indexer

| Crate | README | Architecture | ADRs | Gotchas | Inline Docs | Test Guide |
|-------|--------|-------------|------|---------|-------------|------------|
| kg-indexer | ✅ | ➖ | ✅ | ✅ | ⚠️ | ➖ |

### Actions System

| Crate | README | Architecture | ADRs | Gotchas | Inline Docs | Test Guide |
|-------|--------|-------------|------|---------|-------------|------------|
| actions-indexer | ✅ | ➖ | ➖ | ➖ | ⚠️ | ➖ |
| actions-indexer-pipeline | ✅ | ➖ | ➖ | ➖ | ✅ good | ➖ |
| actions-indexer-repository | ✅ | ➖ | ➖ | ➖ | ✅ good | ➖ |
| actions-indexer-shared | ✅ | ➖ | ➖ | ➖ | ⚠️ | ➖ |
| actions-substream | ✅ | ➖ | ➖ | ➖ | ✅ good | ➖ |

### Non-Rust Services

| Service | README | Architecture | ADRs | Gotchas | Runbook | Test Guide |
|---------|--------|-------------|------|---------|---------|------------|
| api/ (TypeScript) | ❌ | ✅ (in docs/) | ➖ | ➖ | ➖ | ➖ |
| proposal-executor/ (TS) | ✅ | ✅ | ➖ | ➖ | ✅ | ➖ |
| scoring-service/ (Python) | ✅ | ➖ | ➖ | ➖ | ➖ | ➖ |
| deployer/ (TS) | ✅ | ➖ | ➖ | ➖ | ➖ | ➖ |
| monitoring/ | ✅ | ➖ | ➖ | ➖ | ➖ | ➖ |

### Central `docs/` Directory

| Doc Type | Files | Status |
|----------|-------|--------|
| Architecture | `architecture.md`, `api-architecture.md` | ✅ comprehensive |
| Protocol | 6 files in `docs/protocol/` | ✅ good |
| Specs | 3 files in `docs/specs/` | ✅ good |
| RFCs | 3 files in `docs/rfcs/` | ✅ good |
| Plans | 19 files in `docs/plans/` | ✅ active |
| Runbooks | 2 files in `docs/runbooks/` | ⚠️ minimal |
| Research | 6 files in `docs/research/` | ✅ good |
| Gotchas | `gotchas.md` | ✅ good |
| Issues | `issues.md`, `docs/issues/` | ✅ good |

---

## Gap Analysis (Prioritized)

### P0 — Blocks onboarding

| Gap | Impact | Notes |
|-----|--------|-------|
| **Root README is stale** | New people get a misleading picture of the system. Doesn't mention Hermes, Atlas, search-indexer, kg-indexer, proposal-executor, scoring-service. Broken link to `docs/hermes-architecture.md`. | Redesign as progressive disclosure hub (see below) |
| **`api/` has no README** | The primary service has no top-level entry point. Architecture doc exists in central `docs/` but isn't discoverable from the `api/` directory. | Write a README with setup, architecture link, key concepts |
| **No onboarding guide** | No "start here" document for new team members | Could be part of root README redesign or a separate `docs/onboarding.md` |

### P1 — Makes onboarding significantly harder

| Gap | Impact | Notes |
|-----|--------|-------|
| **`hermes-relay/` has no README** | Core shared library — every Hermes transformer depends on it. Has good inline docs but no entry point. | |
| **`hermes-kafka/` has no README** | Shared Kafka utilities with no documentation at all | |
| **No CI/CD guide** | 46 GitHub Actions workflows with no documentation about the CI system, how deploys work, or how to add new services | Could live in `docs/runbooks/` or a new `docs/ci-cd.md` |
| **Root README broken link** | Links to `docs/hermes-architecture.md` which doesn't exist (actual: `docs/architecture.md`) | Quick fix |

### P2 — Would improve the experience

| Gap | Impact | Notes |
|-----|--------|-------|
| **No CONTRIBUTING.md** | No PR process, coding standards, or contribution guidelines documented | |
| **`ipfs/` has no README** | Good inline docs but no entry point | |
| **Runbooks are thin** | Only 2 runbooks (deploying, staging-production). No incident response, no debugging guides | |
| **No CLAUDE.md** | AI agents lack project-level guidance | Lower priority since primary audience is humans |
| **Inconsistent doc patterns** | Some crates use `docs/DECISIONS.md`, others don't. No standard for what every crate should have. | The gold standards exist (atlas, proposal-executor, hermes-pipeline) but aren't codified |

### P3 — Nice to have

| Gap | Impact | Notes |
|-----|--------|-------|
| **No CHANGELOG** | No version history | |
| **No GitHub templates** | No issue/PR templates | |
| **`docs/brainstorms/` is empty** | Directory exists but unused | |
| **`sdk/` README is minimal** | 20 lines, just constant IDs | |

---

## Root README Redesign: Progressive Disclosure

The root README should serve as a **navigational hub** with progressive disclosure:

### Level 1: "What is this?" (5 min read)
- One-paragraph description of Gaia
- System architecture diagram (or link to `docs/architecture.md`)
- List of major subsystems with one-line descriptions

### Level 2: "How do I run it?" (30 min)
- Prerequisites
- Local setup (docker-compose, migrations, env vars)
- Running each service
- Verifying it works

### Level 3: "Where do I go deeper?" (links)
- **By subsystem:** Links to each crate/service README
- **By concern:** Links to architecture docs, protocol docs, specs
- **Operational:** Links to runbooks, gotchas, CI/CD guide
- **Historical:** Links to RFCs, ADRs, research

This structure means a new person reads top-to-bottom on Day 1, and then uses it as a reference hub for the links in Level 3 as they go deeper over their first weeks.

---

## Existing Patterns Worth Replicating

These are the "gold standard" patterns already in the repo that should be adopted more broadly:

1. **`DECISIONS.md` (ADRs)** — `hermes-pipeline`, `kg-indexer` use numbered decision records. Valuable for "why did we do it this way?"
2. **`GOTCHAS.md`** — `hermes-pipeline`, `kg-indexer`, root `docs/`. Captures operational traps and non-obvious behavior.
3. **`ARCHITECTURE.md`** — `proposal-executor`, `search-indexer-repository`. Component-level architecture with diagrams.
4. **`RUNBOOK.md`** — `proposal-executor`. Operational procedures for the service.
5. **Rich module-level `//!` docs** — `hermes-relay`, `hermes-pipeline`, `atlas`. Module docs that explain purpose, concepts, and architecture.

---

## Resolved Questions

1. **Onboarding guide location:** In the root README — the README IS the onboarding guide via progressive disclosure.
2. **Minimum docs standard:** No — focus doc effort where it matters most (high-traffic crates/services), don't mandate across the board.
3. **Docs hygiene:** Archive completed/stale plans to `docs/archive/` to reduce noise.

## Open Questions

1. Is there appetite for rustdoc (`cargo doc`) as a browsable reference, or is that overkill?
