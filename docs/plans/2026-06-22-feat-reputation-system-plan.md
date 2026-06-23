---
title: "feat: Per-space user reputation (Rep) and reputation-weighted voting"
type: feat
date: 2026-06-22
status: draft
---

# feat: Per-space user reputation (Rep) and reputation-weighted voting

## Overview

**Rep** is a per-space, per-user reputation score signifying a person's demonstrated
knowledge, ability, and contributions *within the context of a space*. Its purpose is a
meritocracy: the more you've demonstrably contributed to a space, the more your vote counts
there. Rep is **stored internally as `0–1`** and **presented as `0–1000`** (or `0–100` with a
decimal). It is computed as a new stage that runs **after** the existing score calculation, and
its output feeds back into the vote `weight` already supported by the scoring engine.

This plan scopes the **gaia backend** (rep computation, storage, vote-weighting) plus the **API
read surface** the frontend needs. The geogenesis frontend flows (verify UI, claim profile, view
rep) are a tracked follow-on (see Phase 5 / Open Questions).

**Spec source:** `Rep` design note (Notion / `/tmp/Rep …md`), status "Curator".

## Problem Statement

Today every vote counts equally (uniform `weight = 1.0`) after anti-sybil filtering. There is no
concept of "this user's vote counts for more because they have demonstrated merit in this space."
There is no per-user reputation, trust, or credibility score anywhere in the system — the
`User` model holds only `member_spaces` / `editor_spaces`.

We want a reputation quantity that:
- is **scoped to a space** (expertise is domain-specific),
- rewards demonstrated contribution (votes/rankings on the person) and verified standing,
- is resistant to sybil/gaming (built on on-chain verification and membership), and
- **weights voting** so that consensus in a space is driven by its proven contributors.

## Background — what we build on

Some of Rep's infrastructure already exists; the verification pass (2026-06-23, see **Verification
findings**) showed the Expert band is **not** a simple reuse — there is no per-user score to read.

| Building block | Where | Reuse for Rep |
|---|---|---|
| Per-entity, per-space scores | `scoring-service` Python cronjob → `local_scores`, `global_scores`, `space_scores` | **Expert band** reads a per-user score. Canonical source = the user's **Person entity** `local_score`, resolved via `spaces.topic_id → TYPES_PROPERTY→PERSON_TYPE` (verified) — confirmed (finding #8), but **unpopulated until people are voted/ranked on**. Interim: authored-content aggregation (finding #6). See *User → expertise score*. |
| Person-entity link | `spaces.topic_id` FK → entity typed `Person`, gated on `subspaces type='verified'` | **Expert band** per-user identity resolution (finding #8) |
| Uniform vote weight hook | `Vote.weight` (default `1.0`) in `scoring-service/cronjob/src/algorithm/models.py` | **Reputation-weighted voting** sets this from Rep |
| On-chain verification | `SUBSPACE_VERIFIED`/`SUBSPACE_UNVERIFIED` → `space.trust.extensions` topic → `kg-indexer` → `subspaces` table (`type='verified'`) | **Verified band** raw edges |
| Transitive verified closure | `atlas` (`atlas/src/graph/transitive.rs`): BFS + visited-set from root, diamond/cycle-safe, reverse-dependency incremental invalidation, revocation cascade; emits `topology.canonical` | **Verified band** transitivity + revocation — *no new graph code* |
| Space membership (private, race-insulated) | `members`/`editors` tables; `ranks.members`/`ranks.editors` fed from `space.membership` | **Low-trust band** + anti-sybil |
| Anti-sybil levers | `filter_non_members`, `use_distance_weighting` in `scoring.py` | Compose with rep weighting — ⚠️ production runs `use_distance_weighting=True`, which already *multiplies* `Vote.weight`; rep-weighting would compound (see Verification findings + Decisions #4). |

Verification is **on-chain and permissionless** — anyone can verify any space id; the chain
enforces authorship. A "user" is identified by their **personal space id**. So "is this user
verified?" == "is this user's personal space in atlas's verified closure from root?"

## Proposed Solution

### Representation

- **Stored:** `rep ∈ [0, 1]` per `(user_space_id, space_id)`.
- **Displayed:** `rep × 1000` (0–1000), or `0–100.0` with one decimal.
- **Vote weight:** the stored `rep` (0–1) is used directly as `Vote.weight`. `rep = 0 ⇒ weight = 0`
  (the vote does not count) — there is **no floor**. To get any weight, a user must reach the
  low-trust band, i.e. have a profile **and** be a member of at least one public space. (Cold-start
  is handled below and does not deadlock — see Decisions #5.)

### Band model

Rep is built from four bands. The internal "points" total (0–1000 scale) is the sum of band
contributions, then divided by 1000 for storage.

```
points = low_trust_base + verified_add + professional_add + expert_add
rep     = clamp(points / 1000, 0, 1)
```

**1. Low trust (0–1 points)**
- `0` initially.
- `1` once the user creates a profile AND joins a public space.
- Forced to `0` if the user is flagged or removed from *all* public spaces.
  - ⚠️ **v1 scope:** only **removal/unverify** is consumable today. "Flagged" exists on-chain
    (`FLAGGED`/`SPACE_FAST_PATH_RESTRICTED` → `space.moderation` topic) but **nothing consumes or
    persists it** — no indexer subscribes to `space.moderation`, no `flagged` column exists. v1
    forces low-trust→0 on removal/unverify only; flag-driven demotion is a follow-on requiring a new
    consumer + column. See Verification findings.

**2. Verified (2–10 points)** — *gated by transitive verification*
- Eligible only if the user's personal space is in atlas's **verified closure from root**
  (verified by a verified user, transitively).
- `verified_add = 2 + 8 × (points_earned / max_points_earned)` → range `[2, 10]`.
  - `points_earned` is a per-space contribution metric **derivable from system properties** —
    specifically **payouts modeled as system entities generated by on-chain actions** (per the call
    on 2026-06-22). We do not yet have a way to *sum* these into a per-space total.
  - **Therefore the variable component (`+8 × …`) is deferred.** Until payout summation exists, the
    Verified band ships as a **flat `2`** for any user in the verified closure (eligibility only).
    The scaling to `[2, 10]` lands once payout entities are summable. See Decisions #2.
- Revocation: handled upstream by atlas. If a verifier is unverified/flagged, atlas removes the
  transitive members; the next rep cycle recomputes those users' Verified band to 0.

**3. Professional (2–10 points)** — *space-governed*
- The user works in the industry / produces high-quality original content per the space's
  standards. Requires roles, skills, projects, socials filled out; known in the space or works
  for a verified project.
- `professional_add = rep value of the user's highest role`, each role's rep value being defined
  **in the knowledge layer**, governed by the space's knowledge governance (not hardcoded).
- May be computed before a profile is claimed.

**4. Expert (0–980 points)** — *the exponential band*

> ℹ️ **Source corrected 2026-06-23.** The original `[-1,1]` range was wrong (it's `(0,1)`-neutral-`0.5`),
> but the Person-entity premise was *right* (finding #1 was a false negative — finding #8 confirms the
> Person entity + resolution). Canonical source = the Person entity's `local_score`; authored-content
> is an interim bootstrap until people are voted/ranked on. See *User → expertise score*.

- "Expertise" = a **per-user, per-space score** — canonically the user's Person-entity `local_score`
  (interim: authored-content shrunk mean `q`; see *User → expertise score*) — mapped to `x ∈ [0,1]`
  via `x = max(0, 2q − 1)`, which folds the `(0,1)`-neutral-`0.5` source: below-neutral → `x = 0`,
  `(0.5, 1]` stretches to `(0, 1]`.
- Users with no scored Person entity (and no scored authored content, if bootstrapping) get
  `expert_add = 0`.
- A below-neutral score yields `expert_add = 0` — it does **not** wipe the rest of rep (Decision #3,
  reversed: the score is relative-within-space, so "below median" ≠ "bad actor").
- Otherwise apply the exponential curve:

```
                 e^(k·x) − 1
expert_add = M · ───────────         k = 5,  x = score ∈ [0, 1],  M = EXPERT_MAX
                  e^k − 1
```

With `M = 980` (decided — see Decisions #1) this yields the reference table (x=0→0, 0.25→17,
0.5→74, 0.75→276, 0.9→592, 1.0→980). The exponential keeps average experts in the low-hundreds
while reserving the top of the range for the very best. `980` gives a true 1000 cap: the
theoretical max `1 (low-trust) + 10 (verified) + 10 (professional) + 980 (expert) = 1001` is
absorbed by the `clamp(points/1000, 0, 1)`, so the displayed ceiling is exactly 1000.

A **compile-time lookup table** (1001 × `u16`) quantizes `x→points` for O(1) evaluation with zero
runtime `exp()` — per the spec's discrete implementation. This is the only band needing real math.

### Reputation-weighted voting and the feedback loop

Rep both **derives from** scores and **weights** the votes that produce scores. This is a coupled
system, resolved **iteratively across cron cycles** (not within one):

```
cycle N:   scores_N  = score(votes, weights = rep_{N-1})     ← scoring-service (existing)
           rep_N     = reputation(scores_N, verified, roles) ← NEW stage, runs AFTER scoring
cycle N+1: weights   = rep_N
```

- **Ordering:** rep is computed strictly after scores in the same cycle (the spec's hard rule).
- **Zero rep ⇒ zero weight (no floor).** A user with `rep = 0` has `weight = 0` and their votes do
  not count. To get any weight, a user must reach the low-trust band — profile + membership in at
  least one public space (Decisions #5).
- **No cold-start deadlock.** Low-trust is achievable *immediately* on joining, independent of any
  earned score: every set-up member of a fresh public space gets low-trust `1` ⇒ `rep ≥ 0.001` ⇒
  `weight > 0`, so scores still compute. The deadlock would only arise if weight required *earned*
  rep — it doesn't. Because the smallest non-zero weight is `0.001`, Phase 4 must normalize weights
  (relative ordering is what matters) so tiny absolute weights don't underflow scoring thresholds.

## Technical Approach

### Architecture

```
                          (existing)                         (new)
  ┌────────────────┐   ┌────────────────────┐   ┌─────────────────────────────┐
  │ user_votes,    │──▶│ scoring-service     │──▶│ reputation stage            │
  │ members/editors│   │ (score entities &   │   │ (runs after scoring)        │
  │ local_scores   │   │  spaces)            │   │  • low-trust  • verified    │
  └────────────────┘   │  uses Vote.weight   │   │  • professional • expert    │
          ▲            └─────────┬───────────┘   │  • exponential curve        │
          │                      │               └──────────────┬──────────────┘
          │   weights = rep_{N-1}│                              │ writes
          └──────────────────────┘                              ▼
                                              ┌──────────────────────────────┐
  atlas ──topology.canonical──▶ verified set  │ user_reputation (0–1, per     │
  subspaces (verified edges)                   │  user_space × space)          │
  KG role config (knowledge layer)             └───────────────┬──────────────┘
                                                                │ read
                                                                ▼
                                                       API (GraphQL): userSpaceRep,
                                                       spacePeopleByRep
```

### Data model (new)

```sql
-- Per-space, per-user reputation. user identified by personal space id.
CREATE TABLE user_reputation (
    user_space_id uuid NOT NULL,        -- the user's personal space id
    space_id      uuid NOT NULL,        -- the space this rep is scoped to
    rep           numeric NOT NULL,     -- stored 0–1
    band          text    NOT NULL,     -- 'low_trust' | 'verified' | 'professional' | 'expert'
    -- breakdown for audit / display / debugging
    low_trust     numeric NOT NULL DEFAULT 0,
    verified_add  numeric NOT NULL DEFAULT 0,
    prof_add      numeric NOT NULL DEFAULT 0,
    expert_add    numeric NOT NULL DEFAULT 0,
    expertise_entity_id uuid,           -- entity whose local_score backed the Expert band (see User → expertise score); null if none
    updated_at    timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (user_space_id, space_id)
);
CREATE INDEX user_reputation_space_rep_idx ON user_reputation (space_id, rep DESC);
```

The verified set can be consumed from atlas's `topology.canonical` into a small materialized
table (e.g. `verified_spaces(space_id)`), or queried from atlas's persisted baseline — TBD with
the atlas owner (see Dependencies). The `subspaces` verified edges + `points_earned` come from
existing tables / the knowledge layer.

### User → expertise score (⚠️ corrected 2026-06-23 — a Person entity *does* exist)

The original plan said the Expert band reads a user's **Person-entity `local_score`**. Our first
verification pass (finding #1) called this refuted — **that was wrong**, a tooling artifact: we
grepped source for the `PERSON_TYPE` constant, but gaia stores types **generically** as
`TYPES_PROPERTY` relations, so Person entities exist in the *data*, not in named code. Yaniv
corrected this; finding #8 confirms the mechanism.

**The Person-entity resolution (confirmed, finding #8).** A user's Person entity is reachable per
space:

```
personal space (must be verified)                       -- subspaces type='verified' gate
  → spaces.topic_id                                      -- real FK column → entities.id (schema.ts:103)
  → entity with a TYPES_PROPERTY relation to PERSON_TYPE (7ed45f2b…)
  → local_scores[person_entity_id, space_id]
```

The join is cheap to wire (mirrors the existing `profileSelectFields` type-relation pattern). So the
*resolution* is no longer a blocker. The remaining issue is **data, not plumbing**:

**Person entities are unpopulated with scores today** (finding #5/#7). Voting/ranking is type-agnostic
so a Person entity *can* be scored, but **nothing votes or ranks on people yet** — so
`local_scores[person_entity]` is empty until an "endorse / rank a person" surface exists. Reading it
now yields `expert_add = 0` for everyone (rep collapses to low-trust+verified+professional, ≤21/1000).

**Decision (reframed — pending Yaniv; the scope question is in his court):**
- **Canonical source = the Person entity's `local_score`** (approach A) — spec-aligned, what Yaniv
  intends, and now confirmed cheap to resolve. This is the long-term Expert source.
- **Interim bootstrap = authored-content aggregation** (approach B) — *optional*, used only to give the
  Expert band signal before the endorse-a-person surface ships; ripped out once A is populated. Built
  on the confirmed `edit_versions.created_by_id` join (finding #6):

```
edit_versions (created_by_id = :member_space_id) → version_key
  = value_versions/relation_versions.valid_from_key → (entity_id, space_id) → local_scores
q = (Σ sₑ + κ·0.5) / (n + κ)     κ ≈ 5, shrink toward neutral 0.5 (anti-gaming/low-volume)
```

Whichever source feeds it, the score is `(0,1)`-neutral-`0.5` (finding #2), so map with
`x = max(0, 2q − 1)` before the exponential (`EXPERT_MAX = 980`). If the endorse-a-person surface is
in this milestone, skip B entirely and ship A. **Open:** is that surface in scope now? (asked Yaniv).

*B's caveats if used:* author coverage is **forward-only** (`created_by_id` null for pre-column
content); the join runs through the **versioned** tables, not the live ones.

### Where the code lives

- **Rep computation:** extend the Python `scoring-service` cronjob with a `reputation` stage that
  runs after `rank_entities`/`rank_spaces`. Reuses its DB connection, data provider, and writer.
- **Exponential curve:** a small, table-driven, unit-tested module (Python; mirror of the spec's
  Rust lookup table). O(1), no runtime `exp()`.
- **Vote weighting:** in the existing scoring path, populate `Vote.weight` from `user_reputation`
  (the previous cycle's values); `rep = 0 ⇒ weight = 0`, no floor (Decision #5). Mind the
  distance-weighting composition (finding #4).
- **Verified set:** a consumer or query bridging atlas `topology.canonical` → rep stage.
- **API:** GraphQL fields `userSpaceRep(spaceId, userSpaceId)` and `spacePeopleByRep(spaceId)`
  reading `user_reputation` (mirrors how `entityGlobalScore` is exposed).

## Implementation Phases

### Phase 1 — Expert-band curve (standalone, testable)
- Implement the exponential lookup table (k=5), with `EXPERT_MAX = 980`.
- Unit tests pinning the reference points (x·1000 = 0, 250, 500, 750, 900, 1000 → 0, 17, 74, 276,
  592, 980).
- No DB, no integration — pure function. *Smallest verifiable unit; de-risks the math.*

### Phase 2 — Data model + read API
- `user_reputation` table (Drizzle migration; `CREATE INDEX CONCURRENTLY` runbook — see Risks).
- GraphQL read fields + tests against seeded rows. Frontend can integrate against this early.

### Phase 3 — Rep computation stage (no weighting yet)
- Verified set bridge from atlas `topology.canonical`.
- **Expert band:** wire the Person-entity resolution (`spaces.topic_id → PERSON_TYPE`, verified) and
  read its `local_score` → `x`-mapping → curve (Decisions #8/#9). If the endorse-a-person surface
  isn't in this milestone, add the authored-content bootstrap (`edit_versions` join → shrunk mean) so
  the band isn't all-zeros. See *User → expertise score*. (Pending Yaniv's scope answer.)
- **Low-trust** (removal/unverify only) + **Verified gating** (flat `2`). The Verified *variable*
  component stays deferred until payout-entity summation exists (Decision #2).
- Write `user_reputation`. Weighting still uniform (`1.0`).
- Characterization tests per band + combined, mirroring the scoring-service test style.

### Phase 4 — Reputation-weighted voting (the feedback loop)
- Wire `Vote.weight` from previous-cycle `user_reputation`; `rep = 0 ⇒ weight = 0`, no floor.
- Set `use_distance_weighting=False, filter_non_members=True` under rep-weighting (Decision #4) — rep
  replaces distance-weighting rather than compounding with it.
- Validate convergence on a seeded multi-cycle fixture; assert no cold-start deadlock.
- Feature-flag the weighting so it can be enabled per environment / rolled back instantly.

### Phase 5 — Frontend (geogenesis, follow-on)
- Verify people (on-chain permissionless action), claim a profile, view rep in personal space,
  view people-by-rep in a public space. Tracked separately; depends on Phase 2 API.

## Acceptance Criteria

- A user's `rep` is computed per space and stored in `[0, 1]`, displayable as 0–1000.
- Expert band matches the reference exponential table within rounding.
- Verified band reflects atlas's transitive closure, including revocation cascade on unverify.
- Members with no derivable expertise score get `expert_add = 0` and a well-defined total.
- With weighting enabled, scores reflect rep-weighted votes; with it disabled, behavior is
  identical to today.
- No cold-start deadlock: a fresh space with no rep still produces scores.

## Dependencies & Risks

- **Atlas integration (dependency).** Confirm the contract for consuming the verified closure
  (`topology.canonical` topic vs. querying atlas's persisted baseline) with the atlas owner. Rep's
  Verified band correctness depends on it.
- **Migration lock (risk).** Adding `user_reputation` + index on a busy DB: use `CREATE INDEX
  CONCURRENTLY` (cannot run in Drizzle's txn wrapper — manual pre-build, mirror the `0064` pattern).
- **Feedback-loop stability (risk).** Coupled rep↔scores could oscillate or amplify. Mitigate with
  the across-cycle ordering, weight normalization, the exponential's damping, and a kill switch
  (Phase 4 feature flag).
- **Knowledge-layer / payout-entity config (dependency).** Professional roles/standards live in the
  knowledge layer; the Verified band's `points_earned` comes from payout system entities (on-chain),
  which are not yet summable. Both must exist before those band components are meaningful. Expert +
  Low-trust + Verified-*gating* (flat `2`) do not depend on either and can ship first.
- **Expert-band data, not plumbing (dependency, not a blocker).** The Person-entity resolution exists
  (`spaces.topic_id → PERSON_TYPE`, verified — finding #8), so the join isn't the problem. The risk is
  that Person entities have **no scores until people are voted/ranked on** — so the canonical Expert
  source is dead-empty until the endorse-a-person surface ships. Mitigation: the authored-content
  bootstrap (finding #6), pending Yaniv's scope answer (see *User → expertise score*). Other bands
  unaffected.

## Decisions (resolved with Yaniv, 2026-06-22)

1. **Cap math → `EXPERT_MAX = 980`.** True 1000 cap. The `+1` from low-trust pushing the theoretical
   max to 1001 is absorbed by the storage clamp, so the displayed ceiling is exactly 1000.
2. **`points_earned` → on-chain payout entities.** Derivable from system properties — payouts
   modeled as system entities generated by on-chain actions. No summation mechanism exists yet, so
   the Verified band's variable `+8 ×` component is **deferred**; the band ships as a flat `2`
   (eligibility only) until payout summation lands.
3. **Negative score → ~~wipe entire rep~~ → no score-driven wipe.** ⚠️ **Reversed by verification
   (diverges from Yaniv's "wipe all" lean — flagged for his review).** The score is *relative within a
   space*: half of all entities sit below `0.5` **by construction**, so "below neutral" means "ranked
   below median this cycle," **not** "bad actor." Erasing earned verified/professional credentials on
   that basis is both wrong and trivially weaponizable (brigade below median → nuke standing). And it's
   unnecessary: the `x = max(0, 2q−1)` mapping already sets `expert_add = 0` below neutral, so a
   downvoted user collapses to just their credential floor (`≤ 21/1000 ≈ 0.02` weight) — effectively
   silenced — *without* touching credentials. **Decision:** no score-driven full-rep wipe; reserve
   true rep-zeroing for explicit moderation (removal/unverify today, flagging when built).
4. **Band stacking → additive** (as modeled). No strong preference expressed; keeping the simpler
   additive model. **Weighting composition → disable distance-weighting under rep-weighting**
   (decided via verification): distance-weighting (`0.8^distance_to_root`) is a *crude proxy* for
   voter trustworthiness; rep is a *direct, earned* measure of the same thing, so it **replaces** the
   proxy rather than stacking (`rep × 0.8^distance` would double-penalize and defy calibration). Under
   rep-weighting Phase 4 runs `use_distance_weighting=False, filter_non_members=True, Vote.weight=rep`
   — a combination the config permits, all behind the Phase-4 flag.
5. **Zero rep → zero weight, no floor.** A user must be in ≥1 public space (with a profile) for
   their vote to count. No deadlock because low-trust is earned on joining, not on accumulated score
   (see the feedback-loop section).
6. **Verification scope → global eligibility.** Verified by *at least one* person in the public
   graph's closure from root ⇒ you get the (base) point. The variable value still scales per-space
   (gated on #2).
7. **Recompute cadence → full recompute first** *(recommendation, open to suggestions).* Ship full
   recompute every scoring cycle in Phase 3 — rep is bounded by the same user×space cardinality
   `scoring-service` already pays to recompute `local_scores`. Add a cardinality metric and only
   move to incremental (atlas-style reverse-dependency invalidation) if data shows it's a
   bottleneck; premature invalidation is exactly what caused the recent ranking-indexer crash-loop.

### Verification-driven decisions (2026-06-23, pending Yaniv review)

8. **Expert-band source → Person-entity `local_score` (canonical); authored-content as optional
   interim bootstrap.** ⚠️ **Reframed 2026-06-23 after Yaniv's correction** (the Person entity exists
   — finding #1 was a false negative; the `spaces.topic_id → PERSON_TYPE` resolution is confirmed,
   finding #8). The spec-canonical source is the Person entity's own `local_score`, now cheap to wire.
   It is **empty until an endorse/rank-a-person surface ships**, so authored-content aggregation
   (Bayesian-shrunk mean, `q = (Σ sₑ + κ·0.5)/(n + κ)`, `κ ≈ 5`; built on the `edit_versions` join,
   finding #6) is an *optional* bootstrap to give the band signal in the meantime — dropped once the
   Person score is populated. **Scope question for Yaniv:** is the endorse-a-person surface in this
   milestone (→ ship A only), or do we want the B bootstrap? Full join in *User → expertise score*.
9. **Expert-band x-mapping → `x = max(0, 2q − 1)`.** Folds the `(0,1)`-neutral-`0.5` source: below
   neutral → `0`, `(0.5,1]` stretches to `(0,1]`. Note `s=1` is an unreachable sigmoid asymptote, so
   the realistic Expert ceiling sits below `980` — which is fine (reserves the top for true outliers).

## Verification findings (codebase pass, 2026-06-23)

Eight plan assumptions were checked against the live code. Findings #6/#7/#8 are follow-up probes;
**#8 corrects #1.** file:line evidence below.

**⚠️ 1. ~~No per-user score / no person entity~~ — PARTLY WRONG, corrected by #8.** Two separate
claims were bundled here; only the first holds:
- *True:* the `scoring-service` `local_scores` table is keyed by the **voted-on entity id**
  (`scoring_data_writer.py:88-93`; perspectives = `DISTINCT entity_id, space_id FROM values`); the
  scoring domain identifies a user only as `member_space_id` (`scoring_data_provider.py:238-300`).
- *Wrong:* "there is no Person entity in gaia." This was a **tooling false negative** — we grepped
  source for the `PERSON_TYPE` *constant*, but gaia stores types **generically** as `TYPES_PROPERTY`
  relations, so Person-typed entities exist in *data* without source ever naming the constant.
  Per Yaniv + finding #8, a user's Person entity **is** resolvable. → *See finding #8 and* User →
  expertise score.

**🔴 2. Score range is `(0,1)` neutral `0.5`, not `[-1,1]`.** Production normalization is
`z_score_sigmoid` (`main.py:292`): within-space z-score → logistic sigmoid
(`models.py:304-314`), range `(0,1)`, mean entity = `0.5`. There is no signed/negative half. →
*Drives the Expert-band `x`-mapping and the redefinition of "negative" in Decision #3 (`< 0.5 − ε`).*

**🟢 3. Tiny vote weights are safe.** `Vote.weight` is a pure linear multiplier summed into
`raw_score` (`models.py:78-91`), then z-scored — and z-score is invariant to a uniform positive
scale. No absolute thresholds, no divide-by-zero (only divisor `std_score` is guarded). A `0.001`
weight preserves relative ordering. → *Confirms Decision #5 (zero-rep→0 weight, no floor).*

**🟡 4. Weighting double-counts with distance weighting.** `apply_distance_weighting`
(`scoring.py:107-160`) overwrites `weight = vote.weight × 0.8^distance` before it's summed; the
config forbids `use_distance_weighting` + `filter_non_members` together (`models.py:244-251`), and
production runs distance weighting on (`main.py:288,293`). → *See Decision #4.*

**🟡 5. "Flagged" is not consumable today.** On-chain `FLAGGED`/`SPACE_FAST_PATH_RESTRICTED` decode
to a `space.moderation` topic (`hermes-pipeline/src/pipelines/moderation.rs`), but **no indexer
subscribes** and no `flagged`/`banned` column exists (`members`/`editors`/`ranks.*` are pure join
tables). Only removal/unverify is consumable. → *See Low-trust band v1 scope.*

**🟢 6. Authorship → content → score join exists** (follow-up probe; unblocks the Expert band). The
publishing author is persisted as `edit_versions.created_by_id` — a UUID **space id** (= the author's
`member_space_id`, no wallet translation), extracted from `HermesEdit.authors[0]`
(`kg-indexer/src/main.rs:793-796`; `schema.ts:813-818`). It fans out to every affected entity via
`version_key = value_versions/relation_versions.valid_from_key`, which carry `(entity_id, space_id)`
— joinable to `local_scores`. ⚠️ Coverage is **forward-only** (`created_by_id` nullable for pre-column
content) and lives only in the **versioned** tables (live `entities`/`values`/`relations` have no
author column). → *Source for the decided v1 Expert band — see* User → expertise score.

**🟢 7. Voting/ranking is type-agnostic — a person CAN be scored directly** (follow-up probe; enables
the v2 endorsement model). A vote targets an arbitrary `(object_type ∈ {Entity, Relation}, object_id)`
with **no entity-type gate** (`scoring-service/vote-indexer/src/handlers/voting.rs:16-196`; scoring
reads all `object_type = 0` votes, `scoring_data_provider.py:312-318`). So a person's profile entity
can receive upvotes/downvotes/`RANK_VOTES` (rankings are a separate `ranks.*` KG primitive, also
untyped — `ranking-indexer/src/detect.rs`) and get a `local_scores[profile_entity, space]` row **with
no backend change** — but nothing *produces* such votes today. → *Backend for the v2 direct-endorsement
model is effectively free; the gap is frontend + cold-start.*

**🟢 8. Person entity IS resolvable — corrects #1** (probe after Yaniv's correction). gaia stores
types generically as `TYPES_PROPERTY` relations, so Person-typed entities exist in data even though
`PERSON_TYPE` (`7ed45f2b…`) is never named in source. The link Yaniv described is confirmed: a space
has a real `topic_id` FK column → an entity (`spaces.topic_id`, `schema.ts:103`; set on-chain via
`SetTopic` → `kg-indexer/src/handlers/topics.rs`), and that topic entity can be typed `Person` via a
`TYPES_PROPERTY` relation. Resolution: `personal space (verified) → spaces.topic_id → entity →
TYPES_PROPERTY→PERSON_TYPE`, gated on `subspaces type='verified'` (`schema.ts:281-300`). The join is
SQL-expressible and mirrors the existing `profileSelectFields` pattern (`api/src/profile/queries.ts`).
⚠️ *But* (cross-ref #5/#7): person entities are **unpopulated with scores today** — nothing votes/ranks
on people, so `local_scores[person_entity]` is empty until an endorse-a-person surface ships. → *Makes
the Person score the canonical Expert source (Decision #8), with authored-content as interim bootstrap.*

## Open / to confirm

The verification pass closed most of these. What remains:

- **🔴 Endorse-a-person surface — in this milestone?** (asked Yaniv.) This decides Decision #8: if the
  vote/rank-a-person UI ships now, Expert reads the Person-entity score directly (A only); if not, add
  the authored-content bootstrap (B) so the band has signal at launch. The single biggest open item.
- **Yaniv sign-off on Decision #3** (reversed from his "wipe all" lean — score is relative-within-space).
- **`κ` shrinkage value** (Decision #8) — tune in Phase 1 against real authored-content distributions.
- **`points_earned` summation** (Decision #2) — still blocked on payout-entity summability; Verified
  band ships flat `2` until then.
- **#7 cardinality** — measure user×space before committing to full-recompute permanently.
- **Discord** — share the design for community feedback (Yaniv's ask), ideally after his sign-off.

*Resolved by verification (was open):* Person-entity resolution (#8 — the join exists; **source choice
still pends Yaniv's scope answer**), `x`-mapping (#9), weighting composition (#4 → disable distance
weighting), negative-score semantics (#3 → no score-driven wipe).

## References

### Internal
- Scoring engine: `scoring-service/cronjob/src/algorithm/{models.py,scoring.py}`, `main.py`
- Vote weight hook: `scoring-service/cronjob/src/algorithm/models.py` (`Vote.weight`)
- Atlas transitive closure: `atlas/src/graph/{transitive.rs,canonical.rs,state.rs}`, `persistence.rs`
- Trust pipeline: `hermes-pipeline/src/pipelines/trust.rs`, `kg-indexer/src/handlers/subspaces.rs`
- Verified edges schema: `subspaces` table (`api/drizzle/0045_hard_annihilus.sql`)
- Scores schema: `api/src/services/storage/schema.ts` (`global_scores`, `local_scores`, `space_scores`)

### Spec
- `Rep` design note (Notion), status "Curator" — bands, exponential distribution, feature list.
