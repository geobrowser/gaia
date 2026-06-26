---
title: "feat: Per-space user reputation (Rep) and reputation-weighted voting"
type: feat
date: 2026-06-22
status: draft
---

# Per-space user reputation (Rep) and reputation-weighted voting

## Overview

**Rep** is a per-space, per-user reputation score representing a person's demonstrated knowledge,
ability, and contribution *within the context of a space*. Its purpose is meritocratic: the more a
user has demonstrably contributed to a space, the more their vote counts there. Rep is **stored
internally as `0–1`** and **presented as `0–1000`** (or `0–100` with one decimal). It is computed as
a new stage that runs **after** the existing score calculation, and its output feeds back into the
vote `weight` already supported by the scoring engine.

This plan scopes the **gaia backend** (rep computation, storage, vote-weighting) and the **API read
surface** the frontend requires. The geogenesis frontend flows (verification UI, profile claiming,
rep display) are a tracked follow-on — see Phase 5.

**Spec source:** Notion *Rep* design note (status: Curator) — bands, exponential distribution,
feature list.

## Problem statement

Today every vote counts equally (uniform `weight = 1.0`) after anti-sybil filtering. There is no
mechanism for "this user's vote counts for more because they have demonstrated merit in this space,"
and no per-user reputation, trust, or credibility score anywhere in the system — the `User` model
holds only `member_spaces` / `editor_spaces`.

We want a reputation quantity that:

- is **scoped to a space** (expertise is domain-specific);
- rewards demonstrated contribution (rankings and votes on the person) and verified standing;
- is resistant to sybil/gaming (built on on-chain verification and membership); and
- **weights voting**, so that consensus in a space is driven by its proven contributors.

## Background — what we build on

Much of Rep's infrastructure already exists. The verification pass (2026-06-23) plus a follow-up
ranking-pipeline investigation (2026-06-26) clarified one important point: the Expert band's signal
is produced by the **ordinal ranking system**, not by the entity scoring tables that earlier drafts
assumed (see *Expertise signal → the Expert band* and Finding #9).

| Building block | Where | Reuse for Rep |
|---|---|---|
| Ordinal ranking aggregate | `ranks.ranking_scores` (per entity × space), produced by the `ranking-indexer` from `RANK_VOTES` relations | **Expert band** canonical signal — the rankings push populates this directly (Finding #9). |
| Person-entity link | `spaces.topic_id` FK → entity typed `Person`, gated on `subspaces type='verified'` | **Expert band** per-user identity resolution (Finding #8). |
| Per-entity, per-space scores | `scoring-service` cronjob → `local_scores`, `global_scores`, `space_scores` | **Expert band** complementary signal — populated only if a person receives direct up/down votes. |
| Uniform vote-weight hook | `Vote.weight` (default `1.0`) in `scoring-service/cronjob/src/algorithm/models.py` | **Reputation-weighted voting** sets this from Rep. |
| On-chain verification | `SUBSPACE_VERIFIED` / `SUBSPACE_UNVERIFIED` → `space.trust.extensions` topic → `kg-indexer` → `subspaces` table (`type='verified'`) | **Verified band** raw edges. |
| Transitive verified closure | `atlas` (`atlas/src/graph/transitive.rs`): BFS + visited-set from root, diamond/cycle-safe, reverse-dependency invalidation, revocation cascade; emits `topology.canonical` | **Verified band** transitivity and revocation — *no new graph code required*. |
| Space membership (private, race-insulated) | `members` / `editors` tables; `ranks.members` / `ranks.editors` fed from `space.membership` | **Low-trust band** and anti-sybil. |
| Anti-sybil levers | `filter_non_members`, `use_distance_weighting` in `scoring.py` | Compose with rep weighting. **Caution:** production runs `use_distance_weighting=True`, which already multiplies `Vote.weight`; rep-weighting would compound (Decision #4). |

Verification is **on-chain and permissionless** — anyone can verify any space id, and the chain
enforces authorship. A "user" is identified by their **personal space id**, so "is this user
verified?" reduces to "is this user's personal space in atlas's verified closure from root?"

## Proposed solution

### Representation

- **Stored:** `rep ∈ [0, 1]` per `(user_space_id, space_id)`.
- **Displayed:** `rep × 1000` (0–1000), or `0–100.0` with one decimal.
- **Vote weight:** the stored `rep` (0–1) is used directly as `Vote.weight`. `rep = 0 ⇒ weight = 0`
  (the vote does not count); there is **no floor**. To earn any weight, a user must reach the
  low-trust band — a profile **and** membership of at least one public space. Cold-start does not
  deadlock (Decision #5).

### Band model

Rep is built from four bands. The internal "points" total (0–1000 scale) is the sum of the band
contributions, divided by 1000 for storage:

```
points = low_trust_base + verified_add + professional_add + expert_add
rep     = clamp(points / 1000, 0, 1)
```

**1. Low trust (0–1 points)**

- `0` initially.
- `1` once the user creates a profile **and** joins a public space.
- Forced to `0` if the user is flagged or removed from *all* public spaces.
  - **v1 scope:** only **removal / unverify** is consumable today. "Flagged" exists on-chain
    (`FLAGGED` / `SPACE_FAST_PATH_RESTRICTED` → `space.moderation` topic) but nothing consumes or
    persists it — no indexer subscribes to `space.moderation`, and no `flagged` column exists. v1
    forces low-trust → 0 on removal/unverify only; flag-driven demotion is a follow-on requiring a
    new consumer and column (Finding #5).

**2. Verified (2–10 points)** — *gated by transitive verification*

- Eligible only if the user's personal space is in atlas's **verified closure from root** (verified
  by a verified user, transitively).
- `verified_add = 2 + 8 × (points_earned / max_points_earned)` → range `[2, 10]`.
  - `points_earned` is a per-space contribution metric derivable from system properties —
    specifically **payouts modeled as system entities generated by on-chain actions** (Decision #2).
    No mechanism yet exists to *sum* these into a per-space total.
  - **Therefore the variable component (`+8 × …`) is deferred.** Until payout summation exists, the
    Verified band ships as a **flat `2`** for any user in the verified closure (eligibility only).
    The scaling to `[2, 10]` lands once payout entities are summable (Decision #2).
- Revocation is handled upstream by atlas: if a verifier is unverified or flagged, atlas removes the
  transitive members and the next rep cycle recomputes those users' Verified band to `0`.

**3. Professional (2–10 points)** — *space-governed*

- The user works in the industry or produces high-quality original content per the space's
  standards: roles, skills, projects, and socials filled out; known in the space or affiliated with
  a verified project.
- `professional_add = rep value of the user's highest role`, each role's rep value defined **in the
  knowledge layer** and governed by the space's knowledge governance (not hardcoded).
- May be computed before a profile is claimed.

**4. Expert (0–980 points)** — *the exponential band*

The Expert band converts a per-person, per-space expertise signal into points via an exponential
curve. The signal source and its mapping are detailed in *Expertise signal → the Expert band*; in
summary:

- The signal is a user's **normalized standing in the space's ordinal rankings** (`ranks.ranking_scores`),
  mapped to `x ∈ [0, 1]` (S-tier → `x ≈ 1`, below-median / unranked → `x = 0`).
- Users with no ranking standing get `expert_add = 0`.
- A below-median standing yields `expert_add = 0`; it does **not** wipe the rest of rep (Decision #3
  — the signal is relative within a space, so "below median" ≠ "bad actor").
- Otherwise apply the exponential curve:

```
                 e^(k·x) − 1
expert_add = M · ───────────         k = 5,  x ∈ [0, 1],  M = EXPERT_MAX
                  e^k − 1
```

With `M = 980` (Decision #1) this yields the reference table (`x` = 0 → 0, 0.25 → 17, 0.5 → 74,
0.75 → 276, 0.9 → 592, 1.0 → 980). The exponential keeps average experts in the low hundreds while
reserving the top of the range for the very best. `980` produces a true 1000 cap: the theoretical
maximum `1 (low-trust) + 10 (verified) + 10 (professional) + 980 (expert) = 1001` is absorbed by
`clamp(points/1000, 0, 1)`, so the displayed ceiling is exactly 1000.

A **compile-time lookup table** (1001 × `u16`) quantizes `x → points` for O(1) evaluation with no
runtime `exp()`, per the spec's discrete implementation. This is the only band requiring real math.

### Reputation-weighted voting and the feedback loop

Rep both **derives from** scores and **weights** the votes that produce scores. This is a coupled
system, resolved **iteratively across cron cycles** (never within one):

```
cycle N:   scores_N  = score(votes, weights = rep_{N-1})     ← scoring-service (existing)
           rep_N     = reputation(scores_N, rankings, …)     ← NEW stage, runs AFTER scoring
cycle N+1: weights   = rep_N
```

- **Ordering:** rep is computed strictly after scores in the same cycle (the spec's hard rule).
- **Zero rep ⇒ zero weight, no floor.** A user with `rep = 0` has `weight = 0`, and their votes do
  not count. To earn any weight, a user must reach the low-trust band — profile plus membership of
  at least one public space (Decision #5).
- **No cold-start deadlock.** Low-trust is achievable *immediately* on joining, independent of any
  earned score: every member of a fresh public space gets low-trust `1` ⇒ `rep ≥ 0.001` ⇒
  `weight > 0`, so scores still compute. A deadlock would arise only if weight required *earned*
  rep — it does not. Because the smallest non-zero weight is `0.001`, Phase 4 must normalize weights
  (relative ordering is what matters) so that tiny absolute weights do not underflow scoring
  thresholds.

## Technical approach

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
  ranks.ranking_scores ─────────▶ expertise   │  user_space × space)          │
  subspaces (verified edges)                   └───────────────┬──────────────┘
  KG role config (knowledge layer)                             │ read
                                                                ▼
                                                       API (GraphQL): userSpaceRep,
                                                       spacePeopleByRep
```

### Data model (new)

```sql
-- Per-space, per-user reputation. User identified by personal space id.
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
    expertise_entity_id uuid,           -- person entity whose ranking standing backed the Expert band; null if none
    updated_at    timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (user_space_id, space_id)
);
CREATE INDEX user_reputation_space_rep_idx ON user_reputation (space_id, rep DESC);
```

The verified set can be consumed from atlas's `topology.canonical` into a small materialized table
(e.g. `verified_spaces(space_id)`), or queried from atlas's persisted baseline — to be confirmed with
the atlas owner (see Dependencies). The Expert signal reads `ranks.ranking_scores`; `points_earned`
(Verified band) comes from the knowledge layer once summable.

### Expertise signal → the Expert band

The Expert band needs a per-person, per-space measure of "how expert is this person, here," which it
maps through the exponential curve. The choice of source was reconsidered on 2026-06-26 after a
ranking-pipeline investigation (Finding #9) corrected an earlier assumption.

**What the rankings push actually populates.** The upcoming curator-side rankings push seeds the
**ordinal ranking system** (`ranks.*`), not `local_scores`. `RANK_VOTES` relations are consumed by
the `ranking-indexer` into `ranks.rankings` / `ranks.ranking_items` / `ranks.ranking_scores`
(`api/drizzle/0061_create_ranks_schema.sql`) — a schema the scoring-service never reads. Earlier
drafts assumed the push would seed `local_scores[person_entity]`; that does not hold as built.
Reading `local_scores` after the push would still return `0` for anyone who was only *ranked* rather
than up/down-voted.

**Candidate sources, reconsidered:**

| Source | Populated by | Status |
|---|---|---|
| **Ordinal ranking aggregate** — `ranks.ranking_scores[person_entity, space]` | the rankings push (directly) | **Recommended canonical source** — exactly what the push produces. |
| Person-entity `local_score` | direct up/down votes on a person's entity | Complementary / future — requires a rankings→votes integration to capture ranking signal (it does not exist today). |
| Authored-content aggregation | derived from edit authorship (Finding #6) | Deep fallback only. |

**Why the ordinal ranking aggregate is the natural fit — the S-tier framing.** Ordinal rankings
place people into ordered positions; the human-facing form of this is **tiers (S / A / B / C / D)**.
The Expert band's exponential curve was chosen precisely to keep average contributors low and reserve
the top of the range for the very best — the same shape as a tier ladder. The exponential curve is
therefore *itself* the ordinal → points mapping: a person's normalized rank (or tier) becomes
`x ∈ [0, 1]`, and the curve assigns points. Tiers map cleanly onto the reference table:

| Tier | `x` (normalized rank) | `expert_add` |
|---|---|---|
| S | 1.00 | 980 |
| A | 0.90 | 592 |
| B | 0.75 | 276 |
| C | 0.50 | 74 |
| D | 0.25 | 17 |
| below median / unranked | 0 | 0 |

This resolves the ordinal-vs-cardinal question cleanly and preserves every existing design property:

- **Relative within a space** — ordinal rankings are inherently relative, matching rep's
  domain-specific intent.
- **Below-median → 0, no credential wipe** (Decision #3) — low tiers map to `x = 0` ⇒
  `expert_add = 0`, collapsing to the credential floor without erasing verified or professional
  standing.
- **Eligibility inherited** — ranking blocks already carry their membership restriction
  (`restriction_id`); the ranking-indexer's aggregate respects it, so the Expert band inherits the
  anti-sybil gate without re-implementing it.
- **No new integration** — reading `ranks.ranking_scores` needs no rankings→votes bridge; the push
  lands directly in the source the band reads.

**`x`-mapping for the ranking source.** Replace the sigmoid-tuned `x = max(0, 2q − 1)` (specific to
the `(0,1)`-neutral-`0.5` `local_score`) with a within-space normalization of the ranking aggregate:
a percentile/normalized-rank for `ORDINAL` rankings, or the normalized `weight` for `WEIGHTED`
rankings (`ranks` supports both `rank_type`s). Below-median rank ⇒ `x = 0`. The exact normalization
of `ranks.ranking_scores.score` / `position` → `x` is the one detail to pin in Phase 3.

**Open for sign-off.** This revises Decision #8, which previously named the Person-entity
`local_score` as canonical on the now-corrected premise that rankings feed `local_scores`. The
recommendation is to adopt the ordinal ranking aggregate as canonical, retain `local_score` as a
complementary signal if and when people also receive direct up/down votes, and keep authored-content
as a deep fallback.

**Person-entity resolution (confirmed, Finding #8).** Whichever score table is read, the person
entity for a user is resolved per space:

```
personal space (must be verified)                       -- subspaces type='verified' gate
  → spaces.topic_id                                      -- FK column → entities.id (schema.ts:103)
  → entity with a TYPES_PROPERTY relation to PERSON_TYPE (7ed45f2b…)
  → ranks.ranking_scores[person_entity_id, space_id]     -- (or local_scores, complementary)
```

The join is cheap and mirrors the existing `profileSelectFields` type-relation pattern
(`api/src/profile/queries.ts`).

**Authored-content fallback (Finding #6), if ever needed.** Aggregate the scores of content a user
authored, via the `edit_versions.created_by_id` join:

```
edit_versions (created_by_id = :member_space_id) → version_key
  = value_versions/relation_versions.valid_from_key → (entity_id, space_id) → local_scores
q = (Σ sₑ + κ·0.5) / (n + κ)     κ ≈ 5, shrink toward neutral 0.5 (anti-gaming / low-volume)
```

Caveats if used: author coverage is **forward-only** (`created_by_id` is null for pre-column
content), and the join runs through the **versioned** tables, not the live ones.

### Where the code lives

- **Rep computation:** extend the Python `scoring-service` cronjob with a `reputation` stage that
  runs after `rank_entities` / `rank_spaces`. Reuses its DB connection, data provider, and writer.
- **Exponential curve:** a small, table-driven, unit-tested module (Python; a mirror of the spec's
  Rust lookup table). O(1), no runtime `exp()`.
- **Expert signal:** read `ranks.ranking_scores` for the resolved person entity → normalize to `x` →
  curve.
- **Vote weighting:** in the existing scoring path, populate `Vote.weight` from `user_reputation`
  (the previous cycle's values); `rep = 0 ⇒ weight = 0`, no floor (Decision #5). Compose carefully
  with distance weighting (Finding #4 / Decision #4).
- **Verified set:** a consumer or query bridging atlas `topology.canonical` → rep stage.
- **API:** GraphQL fields `userSpaceRep(spaceId, userSpaceId)` and `spacePeopleByRep(spaceId)`
  reading `user_reputation`, mirroring how `entityGlobalScore` is exposed.

## Implementation phases

### Phase 1 — Expert-band curve (standalone, testable)

- Implement the exponential lookup table (`k = 5`, `EXPERT_MAX = 980`).
- Unit tests pinning the reference points (`x × 1000` = 0, 250, 500, 750, 900, 1000 → 0, 17, 74, 276,
  592, 980) and the tier mapping above.
- No DB, no integration — a pure function. The smallest verifiable unit; de-risks the math.

### Phase 2 — Data model and read API

- `user_reputation` table (Drizzle migration; `CREATE INDEX CONCURRENTLY` runbook — see Risks).
- GraphQL read fields plus tests against seeded rows. The frontend can integrate against this early.

### Phase 3 — Rep computation stage (no weighting yet)

- Verified-set bridge from atlas `topology.canonical`.
- **Expert band:** resolve the person entity (`spaces.topic_id → PERSON_TYPE`, verified), read
  `ranks.ranking_scores`, normalize to `x`, apply the curve (Decisions #8 / #9). The band reads `0`
  until the rankings push seeds standing (~mid-July 2026), degrading gracefully (rep = low-trust +
  verified + professional) with no deadlock. Pin the ranking-aggregate → `x` normalization here.
- **Low-trust** (removal/unverify only) and **Verified gating** (flat `2`). The Verified variable
  component remains deferred until payout-entity summation exists (Decision #2).
- Write `user_reputation`. Weighting still uniform (`1.0`).
- Characterization tests per band and combined, mirroring the scoring-service test style.

### Phase 4 — Reputation-weighted voting (the feedback loop)

- Wire `Vote.weight` from the previous cycle's `user_reputation`; `rep = 0 ⇒ weight = 0`, no floor.
- Set `use_distance_weighting=False, filter_non_members=True` under rep-weighting (Decision #4) — rep
  replaces distance weighting rather than compounding with it.
- Validate convergence on a seeded multi-cycle fixture; assert no cold-start deadlock.
- Feature-flag the weighting so it can be enabled per environment and rolled back instantly.

### Phase 5 — Frontend (geogenesis, follow-on)

- Verify people (on-chain, permissionless), claim a profile, view rep in a personal space, and view
  people-by-rep in a public space. Tracked separately; depends on the Phase 2 API.

## Acceptance criteria

- A user's `rep` is computed per space and stored in `[0, 1]`, displayable as 0–1000.
- The Expert band matches the reference exponential table (and the tier mapping) within rounding.
- The Verified band reflects atlas's transitive closure, including the revocation cascade on unverify.
- Members with no ranking standing get `expert_add = 0` and a well-defined total.
- With weighting enabled, scores reflect rep-weighted votes; with it disabled, behavior is identical
  to today.
- No cold-start deadlock: a fresh space with no rep still produces scores.

## Dependencies and risks

- **Atlas integration (dependency).** Confirm the contract for consuming the verified closure
  (`topology.canonical` topic vs. querying atlas's persisted baseline) with the atlas owner. The
  Verified band's correctness depends on it.
- **Ranking-aggregate semantics (dependency).** The Expert band reads `ranks.ranking_scores`; pin the
  exact normalization of its `score` / `position` to `x ∈ [0, 1]` (percentile for ordinal,
  normalized weight for weighted) before Phase 3 lands. The band degrades gracefully to `0` until the
  rankings push populates standing (~mid-July 2026, crypto space first, ahead of the debates launch).
- **Migration lock (risk).** Adding `user_reputation` plus its index on a busy DB: use
  `CREATE INDEX CONCURRENTLY` (which cannot run inside Drizzle's transaction wrapper — pre-build
  manually, mirroring the `0064` pattern).
- **Feedback-loop stability (risk).** The coupled rep↔scores system could oscillate or amplify.
  Mitigated by the across-cycle ordering, weight normalization, the exponential's damping, and a kill
  switch (the Phase 4 feature flag).
- **Knowledge-layer / payout-entity config (dependency).** Professional roles and standards live in
  the knowledge layer; the Verified band's `points_earned` comes from on-chain payout system entities,
  which are not yet summable. Both must exist before those band components are meaningful. The Expert,
  Low-trust, and Verified-gating (flat `2`) components depend on neither and can ship first.

## Decisions

*Resolved in design review on 2026-06-22 unless noted; #3 and #8 confirmed in review on 2026-06-24;
#8 revised on 2026-06-26 following Finding #9 and pending re-confirmation.*

1. **Cap math → `EXPERT_MAX = 980`.** A true 1000 cap. The `+1` from low-trust, which pushes the
   theoretical maximum to 1001, is absorbed by the storage clamp, so the displayed ceiling is exactly
   1000.
2. **`points_earned` → on-chain payout entities.** Derivable from system properties — payouts modeled
   as system entities generated by on-chain actions. No summation mechanism exists yet, so the
   Verified band's variable `+8 ×` component is **deferred**; the band ships as a flat `2`
   (eligibility only) until payout summation lands.
3. **Below-median signal → no score-driven rep wipe** (originally a "wipe entire rep" lean; reversed
   by verification, confirmed in review 2026-06-24). The signal is *relative within a space*: by
   construction roughly half of all subjects sit below the midpoint, so "below median" means "ranked
   below median this cycle," not "bad actor." Erasing earned verified/professional credentials on
   that basis is both incorrect and trivially weaponizable (rank a user below median → nullify their
   standing). It is also unnecessary: the `x`-mapping already sets `expert_add = 0` below the
   midpoint, so a low-ranked user collapses to their credential floor (`≤ 21/1000 ≈ 0.02` weight),
   effectively silenced, without touching credentials. **Decision:** no score-driven full-rep wipe;
   reserve true rep-zeroing for explicit moderation (removal/unverify today, flagging once built).
4. **Band stacking → additive** (as modeled), and **weighting composition → disable distance
   weighting under rep-weighting** (decided via verification). Distance weighting
   (`0.8^distance_to_root`) is a crude proxy for voter trustworthiness; rep is a direct, earned
   measure of the same thing, so it **replaces** the proxy rather than stacking — `rep × 0.8^distance`
   would double-penalize and defy calibration. Under rep-weighting, Phase 4 runs
   `use_distance_weighting=False, filter_non_members=True, Vote.weight=rep`, a combination the config
   permits, all behind the Phase 4 flag.
5. **Zero rep → zero weight, no floor.** A user must be in at least one public space (with a profile)
   for their vote to count. No deadlock, because low-trust is earned on joining rather than on
   accumulated score (see the feedback-loop section).
6. **Verification scope → global eligibility.** Being verified by at least one person in the public
   graph's closure from root earns the base point. The variable value still scales per space
   (gated on Decision #2).
7. **Recompute cadence → full recompute first** *(recommendation, open to revision).* Ship a full
   recompute every scoring cycle in Phase 3 — rep is bounded by the same user × space cardinality the
   scoring-service already pays to recompute for `local_scores`. Add a cardinality metric and move to
   incremental (atlas-style reverse-dependency invalidation) only if data shows it to be a
   bottleneck; premature invalidation is exactly what caused the recent ranking-indexer crash-loop.
8. **Expert-band source → ordinal ranking aggregate (`ranks.ranking_scores`)** *(revised 2026-06-26;
   pending re-confirmation).* The endorse/rank-a-person surface is in scope: the initial curator-side
   rankings push (~mid-July 2026, crypto space first, ahead of the debates launch) seeds it. Finding
   #9 showed the push populates the ordinal ranking system rather than `local_scores`, so the Expert
   band reads `ranks.ranking_scores` directly, with the exponential curve serving as the ordinal-tier
   → points mapping (see *Expertise signal → the Expert band*). The Person-entity `local_score`
   (resolution confirmed, Finding #8) becomes a complementary signal usable only once people receive
   direct up/down votes; authored-content (Finding #6) remains a deep fallback. *Superseded premise:*
   earlier drafts named `local_score` as canonical on the assumption that rankings feed it.
9. **Expert-band `x`-mapping.** Map the source signal to `x ∈ [0, 1]` with below-median → `0` and the
   top of the range → `1`. For the ranking aggregate this is a within-space normalized rank
   (percentile) or normalized weight; for the `local_score` complementary signal it is
   `x = max(0, 2q − 1)`, which folds the `(0,1)`-neutral-`0.5` range. Note that a `local_score` of `1`
   is an unreachable sigmoid asymptote, so its realistic Expert ceiling sits below `980` — acceptable,
   as it reserves the top for true outliers.

## Verification findings

*Codebase pass 2026-06-23 (#1–#8); ranking-pipeline investigation 2026-06-26 (#9). File:line evidence
below. Findings #6–#8 are follow-up probes; #8 corrects #1; #9 corrects part of #7.*

**[Corrected] 1. "No per-user score / no Person entity."** Two claims were bundled here; only the
first holds.

- *True:* the `scoring-service` `local_scores` table is keyed by the **voted-on entity id**
  (`scoring_data_writer.py:88-93`; perspectives = `DISTINCT entity_id, space_id FROM values`), and
  the scoring domain identifies a user only as `member_space_id` (`scoring_data_provider.py:238-300`).
- *Wrong:* "there is no Person entity in gaia." This was a tooling false negative — a grep for the
  `PERSON_TYPE` constant in source, when gaia stores types generically as `TYPES_PROPERTY` relations,
  so Person-typed entities exist in *data* without source ever naming the constant. A user's Person
  entity **is** resolvable (Finding #8 and *Expertise signal → the Expert band*).

**[Caution] 2. Score range is `(0,1)` neutral `0.5`, not `[-1,1]`.** Production normalization is
`z_score_sigmoid` (`main.py:292`): a within-space z-score through a logistic sigmoid
(`models.py:304-314`), range `(0,1)`, mean entity = `0.5`. There is no signed/negative half. Drives
the `x`-mapping and the definition of "below median" in Decision #3.

**[Confirmed] 3. Tiny vote weights are safe.** `Vote.weight` is a pure linear multiplier summed into
`raw_score` (`models.py:78-91`), then z-scored — and the z-score is invariant to a uniform positive
scale. No absolute thresholds and no divide-by-zero (only the divisor `std_score` is guarded). A
`0.001` weight preserves relative ordering. Confirms Decision #5.

**[Caution] 4. Weighting double-counts with distance weighting.** `apply_distance_weighting`
(`scoring.py:107-160`) overwrites `weight = vote.weight × 0.8^distance` before summation; the config
forbids `use_distance_weighting` and `filter_non_members` together (`models.py:244-251`), and
production runs distance weighting on (`main.py:288,293`). See Decision #4.

**[Caution] 5. "Flagged" is not consumable today.** On-chain `FLAGGED` /
`SPACE_FAST_PATH_RESTRICTED` decode to a `space.moderation` topic
(`hermes-pipeline/src/pipelines/moderation.rs`), but no indexer subscribes and no `flagged` /
`banned` column exists (`members` / `editors` / `ranks.*` are pure join tables). Only removal/unverify
is consumable. See the Low-trust band v1 scope.

**[Confirmed] 6. Authorship → content → score join exists** (follow-up probe). The publishing author
is persisted as `edit_versions.created_by_id` — a UUID space id (= the author's `member_space_id`, no
wallet translation), extracted from `HermesEdit.authors[0]` (`kg-indexer/src/main.rs:793-796`;
`schema.ts:813-818`). It fans out to every affected entity via
`version_key = value_versions/relation_versions.valid_from_key`, which carry `(entity_id, space_id)`,
joinable to `local_scores`. Coverage is **forward-only** (`created_by_id` nullable for pre-column
content) and lives only in the **versioned** tables. Source for the authored-content fallback.

**[Confirmed] 7. Voting and ranking are type-agnostic — a person can be scored directly** (follow-up
probe). A vote targets an arbitrary `(object_type ∈ {Entity, Relation}, object_id)` with no
entity-type gate (`vote-indexer/src/handlers/voting.rs:16-196`; scoring reads all `object_type = 0`
votes, `scoring_data_provider.py:312-318`). So a person's entity *can* receive direct up/down votes
and acquire a `local_scores[person_entity, space]` row with no backend change. **Corrected by #9:**
`RANK_VOTES` are **not** in this path — they flow to the separate `ranks.*` schema and do not reach
`local_scores`. Direct up/down voting on people also does not happen today.

**[Confirmed] 8. Person entity is resolvable — corrects #1.** gaia stores types generically as
`TYPES_PROPERTY` relations, so Person-typed entities exist in data even though `PERSON_TYPE`
(`7ed45f2b…`) is never named in source. A space has a real `topic_id` FK column → an entity
(`spaces.topic_id`, `schema.ts:103`; set on-chain via `SetTopic` → `kg-indexer/src/handlers/topics.rs`),
and that topic entity can be typed `Person` via a `TYPES_PROPERTY` relation. Resolution:
`personal space (verified) → spaces.topic_id → entity → TYPES_PROPERTY → PERSON_TYPE`, gated on
`subspaces type='verified'` (`schema.ts:281-300`). The join is SQL-expressible and mirrors the
existing `profileSelectFields` pattern (`api/src/profile/queries.ts`).

**[Corrected] 9. Rankings and entity scores are separate pipelines** (investigation 2026-06-26;
corrects part of #7). `RANK_VOTES` relations are consumed by the `ranking-indexer`
(`ranking-indexer/src/detect.rs`) into a private `ranks.*` schema —
`ranks.rankings` / `ranks.ranking_items` / `ranks.ranking_scores`
(`api/drizzle/0061_create_ranks_schema.sql`), supporting `ORDINAL` and `WEIGHTED` rank types with a
per-item `position` and optional `weight`, aggregated per entity × space into `ranks.ranking_scores`.
The scoring-service reads only `user_votes` filtered to `object_type = 0`
(`scoring_data_provider.py` `_fetch_votes`), fed by the `vote-indexer` from `HermesVoteCast` events;
it never reads `ranks.*`. **Consequence:** the rankings push seeds `ranks.ranking_scores`, not
`local_scores`. The Expert band therefore reads the ranking aggregate directly (Decision #8); feeding
rankings into `local_scores` would require a separate rankings→votes integration (a distinct,
larger piece of work) and is not needed for rep.

## Open / to confirm

- **Re-confirm Decision #8** — the Expert source moves from `local_score` to the ordinal ranking
  aggregate (`ranks.ranking_scores`) on the corrected premise of Finding #9. This is the one item
  needing fresh sign-off.
- **Ranking-aggregate → `x` normalization** — pin the exact mapping of `ranks.ranking_scores`
  (`score` / `position`, and the `WEIGHTED` case) to `x ∈ [0, 1]` in Phase 3.
- **`points_earned` summation** (Decision #2) — blocked on payout-entity summability; the Verified
  band ships flat `2` until then.
- **Recompute cardinality** (Decision #7) — measure user × space before committing to full recompute
  permanently.
- **Community share** — circulate the design for community feedback (a design-review action item),
  ideally after the Decision #8 re-confirmation.

*Resolved:* endorse-a-person surface is in scope (review 2026-06-24); Person-entity resolution
(Finding #8); negative/below-median semantics (Decision #3, confirmed 2026-06-24); weighting
composition (Decision #4); rankings/scoring pipeline separation (Finding #9).

## References

### Internal

- Scoring engine: `scoring-service/cronjob/src/algorithm/{models.py,scoring.py}`, `main.py`
- Vote-weight hook: `scoring-service/cronjob/src/algorithm/models.py` (`Vote.weight`)
- Ranking pipeline: `ranking-indexer/src/{detect.rs,scoring.rs,storage.rs}`,
  `ranks.*` schema (`api/drizzle/0061_create_ranks_schema.sql`)
- Atlas transitive closure: `atlas/src/graph/{transitive.rs,canonical.rs,state.rs}`, `persistence.rs`
- Trust pipeline: `hermes-pipeline/src/pipelines/trust.rs`, `kg-indexer/src/handlers/subspaces.rs`
- Verified-edges schema: `subspaces` table (`api/drizzle/0045_hard_annihilus.sql`)
- Scores schema: `api/src/services/storage/schema.ts` (`global_scores`, `local_scores`, `space_scores`)

### Spec

- Notion *Rep* design note (status: Curator) — bands, exponential distribution, feature list.
