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
- rewards demonstrated contribution (authored work, rankings, and votes on the person) and verified standing;
- is resistant to sybil/gaming (built on on-chain verification and membership); and
- **weights voting**, so that consensus in a space is driven by its proven contributors.

## Background — what we build on

Much of Rep's infrastructure already exists. The verification pass (2026-06-23) and a follow-up
ranking-pipeline investigation (2026-06-26) mapped the available signals; a design discussion on
2026-06-30 then settled how the Expert band consumes them: **contribution is the canonical baseline
signal, with the ordinal ranking system as a complementary overlay** (Decision #8 and *Expertise
signal → the Expert band*). Both are distinct from the entity scoring assumptions earlier drafts
made (Finding #9).

| Building block | Where | Reuse for Rep |
|---|---|---|
| Authored-content / entity scores | `scoring-service` cronjob → `local_scores`, `global_scores`, `space_scores`, joinable to authorship via `edit_versions.created_by_id` (Finding #6) | **Expert band** canonical *baseline* signal (contribution) — scales to every editor, no curator effort (Decision #8). |
| Ordinal ranking aggregate | `ranks.ranking_scores` (per entity × space), produced by the `ranking-indexer` from `RANK_VOTES` relations | **Expert band** complementary *overlay* signal — high-signal where curation is feasible; the rankings push populates it directly (Finding #9). |
| Person-entity link | `spaces.topic_id` FK → entity typed `Person`, gated on `subspaces type='verified'` | **Expert band** per-user identity resolution (Finding #8). |
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

**The Expert band is the score; the other three are an eligibility premium.** The points budget is
deliberately lopsided: Expert spans `0–980`, while Low-trust, Verified, and Professional together
contribute at most `21` points (`≈ 0.021` rep). For any ranked user, `rep ≈ expert_add / 1000`; the
other three bands function as a small floor and a gate, not as independent sources of standing. A
fully verified, credentialed professional with **no expertise standing** therefore lands at
`rep ≈ 0.02` — effectively silenced. This is intentional (standing is *earned through demonstrated
expertise in the space*, not conferred by credentials), but it means the system's behaviour is
governed almost entirely by the Expert band's signal and curve.

The `EXPERT_MAX = 980` cap and the curve steepness `k` are therefore the principal **policy levers**
of the whole system, not implementation constants chosen by feel: together they set how steeply
standing converts to influence (a near-`980:1` weight ratio between the top-ranked voter and one just
above the median). Treat them as explicit, reviewable policy. The weighting that consumes them ships
behind a feature flag (Phase 4), so the curve can be retuned before it affects live consensus.

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

- The signal is a user's **normalized expertise standing in the space**, mapped to `x ∈ [0, 1]`
  (top → `x ≈ 1`, below-median / no standing → `x = 0`). The canonical baseline is **contribution**
  (authored-content / person `local_score`), enriched by an **ordinal ranking overlay**
  (`ranks.ranking_scores`) where rankings exist (Decision #8).
- Users with no standing under either signal get `expert_add = 0`.
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
  rep — it does not. Phase 4 normalizes weights so that what reaches the scorer is the *relative*
  ordering across users, not the raw absolute magnitudes. This is **not** about underflow — Finding #3
  confirms z-scoring is invariant to a uniform positive scale and imposes no absolute threshold — but
  about controlling the *ratio* between the strongest and weakest votes (a near-`980:1` spread is a
  policy choice, not a given) and keeping it stable across cycles.

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
  contribution + ranks ────────▶ expertise    │  user_space × space)          │
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
the atlas owner (see Dependencies). The Expert signal reads the contribution baseline (`local_scores`)
plus the ranking overlay (`ranks.ranking_scores`); `points_earned`
(Verified band) comes from the knowledge layer once summable.

### Expertise signal → the Expert band

The Expert band needs a per-person, per-space measure of "how expert is this person, here," which it
maps through the exponential curve. The source was reconsidered twice: a ranking-pipeline
investigation on 2026-06-26 (Finding #9) corrected which tables the rankings push populates, and a
design discussion on 2026-06-30 settled the signal model — **contribution as the canonical baseline,
ordinal rankings as a complementary overlay** (Decision #8).

**Why contribution is the baseline.** Expertise should be legible from what a user has demonstrably
contributed, and that signal *scales*: it is computed from data the system already has (authored
content joined to its scores, Finding #6; or a person entity's own `local_score`), covers every
editor who has contributed, and needs no human effort to produce. Rankings, by contrast, require
curators to rank people — which does not scale to spaces with hundreds of editors (a curator can only
rank the subset they have context on). So rankings *enrich* the signal where they exist; they do not
define it.

**Why rankings are a strong overlay (not the base).** A ranking ballot is partial: a curator ranks
only the people they know, "ranked at all" carries a floor, and the `ranking-indexer` aggregates many
partial ballots into one ordered result per `(entity, space)` — so no single curator must rank
everyone. Where a space *is* small enough to curate, that is high-quality signal. But coverage is the
catch: in a large space, editors nobody ranked get no ranking standing regardless of contribution, so
rankings alone would be blind to contribution they cannot see. Layered on top of the contribution
baseline, they add signal without that blind spot.

**Sequencing caveat.** The two signals do not arrive together. The curator-side rankings push
(~mid-July 2026) populates `ranks.ranking_scores` directly; the contribution baseline
(authored-content aggregation, Finding #6) must be built and is **forward-only**
(`edit_versions.created_by_id` is null for pre-column content). So v1 may bootstrap the Expert band on
the ranking overlay while the contribution baseline is built out, and the `x`-mapping must accept
either signal present (or both). Note also that reading `local_scores` after the rankings push still
returns `0` for anyone only *ranked* — rankings and entity scores are separate pipelines (Finding #9).

**How ranks are produced** (the overlay pipeline, for reference). A "rank" is a user-submitted
*ballot*, not a side effect of ordinary edits:

- **Submission (GRC-20 edit).** A `Rank` entity (typed via `TYPES`, carrying a `rank_type` of
  `ORDINAL` or `WEIGHTED`); `RANK_VOTES` relations from it to each ranked entity, each with a
  fractional `position` and a `to_space` perspective (WEIGHTED ballots carry a weight on the reified
  vote entity); a `Ranking Block` entity (name, filter, submission window, membership restriction);
  and a `RANK_BLOCK` relation linking rank → block.
- **Detect** (`ranking-indexer/src/detect.rs`) extracts those ops from the `knowledge.edits` stream
  into `ranks.rankings` / `ranks.ranking_items` / `ranks.ranking_blocks`.
- **Recompute** (`recompute.rs`), per affected block, full and order-independent:
  **dedup** (latest submission per author wins) → **eligibility** (membership restriction +
  submission window) → **scoring** → **publish**.
- **Scoring** (`scoring.rs`) normalizes each ballot's items into `[0.5, 1.0]` (ordinal: linear by
  position, best `1.0` / worst `0.5`; weighted: min-max of weights), **sums** per `(entity, space)`
  across eligible ballots, sorts descending, and assigns a 1-based `position` → the
  `{entity_id, space_id, score, position}` rows in `ranks.ranking_scores` that the Expert band reads.

**Candidate sources, reconsidered:**

| Source | Populated by | Role |
|---|---|---|
| **Authored-content aggregation** — content a user authored, joined to `local_scores` (Finding #6) | edit authorship (`edit_versions.created_by_id`) | **Canonical baseline.** Scales to every contributor; forward-only coverage, needs building. |
| **Person-entity `local_score`** | direct up/down votes on a person's entity | Baseline (alternative/complement) — usable once people receive direct votes; none today. |
| **Ordinal ranking aggregate** — `ranks.ranking_scores[person_entity, space]` | the rankings push (directly) | **Overlay.** High-signal where curation is feasible; available first (mid-July push). |

**The S-tier framing — why the curve suits the ranking overlay.** Ordinal rankings place people into
ordered positions; the human-facing form of this is **tiers (S / A / B / C / D)**. The Expert band's
exponential curve was chosen precisely to keep average contributors low and reserve the top of the
range for the very best — the same shape as a tier ladder. For the ranking overlay the curve is
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

**Tiers are illustrative, not the operative input.** The table shows how familiar tier labels land on
the curve; it is not the implemented mapping. The operative input is the continuous within-space
normalized rank described above, and the tier rows are convenient reference points on it (`C` sits at
the median, `x = 0.5`, immediately above the below-median cliff to `0`). If tiers are ever surfaced
in product or governance as the real unit, the mapping must be redefined as a discrete tier → `x`
step function and reconciled with the continuous normalization — they are not interchangeable at the
boundaries.

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

**`x`-mapping for the ranking source — the central design lever.** Because `rep ≈ expert_add / 1000`
for any ranked user, the mapping from `ranks.ranking_scores` to `x ∈ [0, 1]` **is** the reputation
algorithm; it is not a detail to be settled late. The baseline replaces the sigmoid-tuned
`x = max(0, 2q − 1)` (specific to the `(0,1)`-neutral-`0.5` `local_score`) with a within-space
normalization of the ranking aggregate: a percentile / normalized-rank for `ORDINAL` rankings, or the
normalized `weight` for `WEIGHTED` rankings (`ranks` supports both `rank_type`s), with below-median
rank ⇒ `x = 0`. Two consequences must be decided deliberately, not inherited from the arithmetic:

- **Thin-ranked spaces.** A percentile is unstable when few people are ranked: with five ranked
  subjects, the "median" and the tier boundaries swing on every new rank, and a single ranking can
  move a user's weight by a large multiple. Phase 3 must define behaviour below a minimum-ranked-
  population threshold (e.g. suppress the Expert band, or shrink `x` toward `0`, until the ranked set
  is large enough to be meaningful).
- **Half the population muted by construction.** Below-median ⇒ `x = 0` ⇒ `expert_add = 0` ⇒
  `rep ≈ 0.02`, which combined with no-floor zero-weighting means roughly **half of all ranked
  participants carry near-zero vote weight every cycle**. That is a strong, deliberate consensus
  policy — state it as such and confirm it is the intended dynamic, rather than letting it fall out
  of the curve.

Pinning the exact `ranks.ranking_scores.score` / `position` → `x` function (and the `WEIGHTED` case)
is the gating design task for Phase 3.

**Resolved (2026-06-30).** Decision #8 is settled on the contribution-baseline + ranking-overlay
model above: contribution (authored-content / person `local_score`) is the canonical baseline because
it scales and covers every contributor; the ordinal ranking aggregate is a complementary overlay,
strong where curation is feasible and available first via the mid-July push. The remaining open task
is the `x`-mapping — how each signal normalizes to `x ∈ [0, 1]` and how the two combine (Phase 3).

**Person-entity resolution (confirmed, Finding #8).** Whichever score table is read, the person
entity for a user is resolved per space:

```
personal space (must be verified)                       -- subspaces type='verified' gate
  → spaces.topic_id                                      -- FK column → entities.id (schema.ts:103)
  → entity with a TYPES_PROPERTY relation to PERSON_TYPE (7ed45f2b…)
  → local_scores[person_entity_id, space_id]             -- contribution baseline
    + ranks.ranking_scores[person_entity_id, space_id]   -- ranking overlay
```

The join is cheap and mirrors the existing `profileSelectFields` type-relation pattern
(`api/src/profile/queries.ts`).

**Authored-content aggregation (Finding #6) — the contribution baseline.** Aggregate the scores of
content a user authored, via the `edit_versions.created_by_id` join:

```
edit_versions (created_by_id = :member_space_id) → version_key
  = value_versions/relation_versions.valid_from_key → (entity_id, space_id) → local_scores
q = (Σ sₑ + κ·0.5) / (n + κ)     κ ≈ 5, shrink toward neutral 0.5 (anti-gaming / low-volume)
```

This is the canonical baseline signal (Decision #8). Two caveats shape v1: author coverage is
**forward-only** (`created_by_id` is null for pre-column content), and the join runs through the
**versioned** tables, not the live ones — which is why the ranking overlay, available first, may
carry the Expert band until this is built out.

### Where the code lives

- **Rep computation:** extend the Python `scoring-service` cronjob with a `reputation` stage that
  runs after `rank_entities` / `rank_spaces`. Reuses its DB connection, data provider, and writer.
- **Exponential curve:** a small, table-driven, unit-tested module (Python; a mirror of the spec's
  Rust lookup table). O(1), no runtime `exp()`.
- **Expert signal:** read the contribution baseline (`local_scores` for the resolved person entity /
  authored-content aggregation) and, where present, the ranking overlay (`ranks.ranking_scores`);
  combine and normalize to `x` → curve.
- **Vote weighting:** in the existing scoring path, populate `Vote.weight` from `user_reputation`
  (the previous cycle's values); `rep = 0 ⇒ weight = 0`, no floor (Decision #5). Compose carefully
  with distance weighting (Finding #4 / Decision #4).
- **Verified set:** a consumer or query bridging atlas `topology.canonical` → rep stage.
- **Read & publish surface:** see *Read & publish surface* below — a `user_reputation` table (system
  of record) plus an optional published `Reputation` system property for product/graph consumption.

### Read & publish surface

Rep needs to be both *consumed internally* (the vote-weighting loop) and *exposed* to product. The
codebase offers two established patterns, and rep uses them the way the ranking-indexer already does —
both at once:

- **`user_reputation` table — system of record (required).** The vote-weighting feedback loop reads
  rep straight from this table each cycle, and it holds the band breakdown
  (`low_trust` / `verified_add` / `prof_add` / `expert_add`) for audit and display. This is the
  internal source of truth and does not change regardless of the product surface. Scores
  (`local_scores` / `global_scores`) follow this pattern — private tables surfaced via a query plugin
  (`entityGlobalScore`), not graph properties.
- **Published `Reputation` system property — optional, for product (Decision #10).** The
  ranking-indexer publishes its aggregate into the public graph as a protected, indexer-owned property
  (`Rank position value`, on a reified `RANK_POSITION` relation — `ranking-indexer/src/publish.rs`,
  reserved in `PROTECTED_PROPERTY_IDS`). Rep can do the same: write a per-`(person, space)`
  `Reputation` value on the Person entity, so product reads it through the generic entity/graph API
  (sortable, composable with other queries) rather than a bespoke field. It fits cleanly — graph
  values are already space-scoped (`values.space_id`), so `value(person_entity, Reputation, space)`
  maps 1:1 with rep's key. Publish the display integer (0–1000), keep the breakdown private in the
  table, and upsert the whole set each recompute with deterministic IDs (the `publish.rs` shape).
- **GraphQL fields (either way).** `userSpaceRep(spaceId, userSpaceId)` and `spacePeopleByRep(spaceId)`
  read `user_reputation`, mirroring how `entityGlobalScore` is exposed.

Whether to also publish the system property is gated on product's consumption model (Decision #10):
publish it if they want rep in the generic graph API; skip it if a dedicated field suffices, since
writing into the canonical graph every cycle is real churn for no gain.

## Implementation phases

**What actually ships, and when.** Be clear-eyed about the v1 surface. The Verified variable
component is deferred (Decision #2), Professional depends on knowledge-layer role config that does
not yet exist, and the Expert band reads `0` until the rankings push lands (~mid-July 2026) *and* the
`x`-normalization is pinned. Until both arrive, computed rep is essentially the low-trust bit plus a
flat Verified `2` — i.e. `≈ 0.001–0.003` for everyone. Phases 1–3 therefore ship the *machinery*
(curve, table, API, computation stage, atlas/ranks bridges) and a flat eligibility signal; the
system's *meaningful* output is gated on the rankings push and the normalization decision, both
downstream. This is deliberate, low-risk sequencing — but "degrades gracefully" should not obscure
that the interesting behaviour arrives only once those inputs do.

### Phase 1 — Expert-band curve (standalone, testable)

- Implement the exponential lookup table (`k = 5`, `EXPERT_MAX = 980`).
- Unit tests pinning the reference points (`x × 1000` = 0, 250, 500, 750, 900, 1000 → 0, 17, 74, 276,
  592, 980) and the tier mapping above.
- No DB, no integration — a pure function. The smallest verifiable unit; de-risks the math.

### Phase 2 — Data model and read API

- `user_reputation` table (Drizzle migration; `CREATE INDEX CONCURRENTLY` runbook — see Risks).
- GraphQL read fields plus tests against seeded rows. The frontend can integrate against this early.

### Phase 3 — Rep computation stage (no weighting yet)

> **Blocked on the `x`-mapping.** Decision #8 (signal model = contribution baseline + ranking overlay)
> is resolved; the open gate is how each signal normalizes to `x ∈ [0, 1]` and how the two combine
> (below). This drives ~98% of rep — do not begin Phase 3 computation until it is pinned.

- Verified-set bridge from atlas `topology.canonical`.
- **Expert band:** resolve the person entity (`spaces.topic_id → PERSON_TYPE`, verified), read the
  contribution baseline (`local_scores` / authored-content) and the ranking overlay
  (`ranks.ranking_scores`), combine, normalize to `x`, apply the curve (Decisions #8 / #9). The band
  degrades gracefully to `0` where neither signal has standing (rep = low-trust + verified +
  professional) with no deadlock. Pin the signal → `x` normalization and the baseline/overlay combine
  here. (v1 may lean on the ranking overlay first; the contribution baseline is forward-only and needs
  building.)
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
- Members with no expertise standing (neither contribution nor ranking) get `expert_add = 0` and a
  well-defined total.
- With weighting enabled, scores reflect rep-weighted votes; with it disabled, behavior is identical
  to today.
- No cold-start deadlock: a fresh space with no rep still produces scores.

## Dependencies and risks

- **Atlas integration (dependency).** Confirm the contract for consuming the verified closure
  (`topology.canonical` topic vs. querying atlas's persisted baseline) with the atlas owner. The
  Verified band's correctness depends on it.
- **Signal normalization (dependency).** The Expert band combines the contribution baseline
  (`local_scores` / authored-content) with the ranking overlay (`ranks.ranking_scores`); pin each
  signal's normalization to `x ∈ [0, 1]` (percentile for ordinal, normalized weight for weighted,
  `max(0, 2q − 1)` for `local_score`) and the combine rule before Phase 3 lands. The band degrades
  gracefully to `0` until a signal populates standing — the ranking overlay arrives first via the
  push (~mid-July 2026, crypto space first, ahead of the debates launch).
- **Migration lock (risk).** Adding `user_reputation` plus its index on a busy DB: use
  `CREATE INDEX CONCURRENTLY` (which cannot run inside Drizzle's transaction wrapper — pre-build
  manually, mirroring the `0064` pattern).
- **Feedback-loop stability (risk).** The coupled rep↔scores system could oscillate or amplify.
  Mitigated by the across-cycle ordering, weight normalization, the exponential's damping, and a kill
  switch (the Phase 4 feature flag). Note this addresses *numerical* instability only — see the next
  item for the sociological one.
- **Entrenchment / mobility (risk).** Top-ranked users receive on the order of `50–980×` the vote
  weight of median users, rep has no time decay, and there is no built-in mobility mechanism. If the
  rankings that drive the Expert band are themselves shaped by the same high-rep cohort, that cohort
  can entrench: its votes dominate the scores and rankings that would otherwise let newcomers displace
  it. The across-cycle ordering and the exponential's damping address numerical oscillation, not this
  attractor. Mitigations to consider (not yet designed): rep decay over time, a cap on any single
  cohort's share of total weight, or periodic re-normalization. At minimum, instrument the per-space
  rep distribution and watch its top-k / Gini share across cycles.
- **Knowledge-layer / payout-entity config (dependency).** Professional roles and standards live in
  the knowledge layer; the Verified band's `points_earned` comes from on-chain payout system entities,
  which are not yet summable. Both must exist before those band components are meaningful. The Expert,
  Low-trust, and Verified-gating (flat `2`) components depend on neither and can ship first.

## Decisions

*Resolved in design review on 2026-06-22 unless noted; #3 and #8 confirmed 2026-06-24; #8 revised
2026-06-26 (Finding #9) and re-settled 2026-06-30 (contribution baseline + ranking overlay); #10
added 2026-06-30.*

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
8. **Expert-band signal → contribution baseline + ranking overlay** *(re-settled 2026-06-30).*
   Contribution is the canonical baseline: it scales to every editor and needs no curator effort,
   sourced from authored-content aggregation (Finding #6) or a person entity's own `local_score`
   (Finding #8). The ordinal ranking aggregate (`ranks.ranking_scores`, Finding #9) is a complementary
   overlay — high-signal where curation is feasible, but unable to cover large editor sets on its own
   (a curator can only rank people they have context on), so it enriches rather than defines the
   signal. The curve doubles as the ordinal → points mapping for the overlay (see *Expertise signal →
   the Expert band*). **Sequencing:** rankings land first (mid-July push); the contribution baseline is
   forward-only and must be built, so v1 may bootstrap on the overlay. *Superseded:* the 2026-06-22
   draft named `local_score` canonical assuming rankings feed it; the 2026-06-26 revision named the
   ranking aggregate canonical — both are folded into the baseline+overlay model here.
9. **Expert-band `x`-mapping** *(open — the Phase 3 gate).* Map each signal to `x ∈ [0, 1]` with
   below-median → `0` and the top of the range → `1`, then combine baseline and overlay. For the
   contribution baseline (`local_score`) it is `x = max(0, 2q − 1)`, folding the `(0,1)`-neutral-`0.5`
   range (a `local_score` of `1` is an unreachable sigmoid asymptote, so its realistic ceiling sits
   below `980` — acceptable, it reserves the top for true outliers). For the ranking overlay it is a
   within-space normalized rank (percentile) or normalized weight. The combine rule (max? weighted
   blend? overlay-dominates-where-present?) and the thin-ranked-space guard are open for Phase 3.
10. **Rep exposure → table (required) + optional published system property** *(added 2026-06-30).*
    `user_reputation` is the system of record (drives the vote-weighting loop, holds the band
    breakdown). Whether to *also* publish a protected `Reputation` system property into the KG (the
    `publish.rs` pattern) is gated on product's consumption model: publish it if product reads rep
    through the generic entity/graph API; skip it if a dedicated `userSpaceRep` field suffices, since
    per-cycle graph writes are real churn. See *Read & publish surface*.

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
`local_scores`. The Expert band's ranking *overlay* therefore reads the aggregate directly
(Decision #8); feeding rankings into `local_scores` would require a separate rankings→votes
integration (a distinct,
larger piece of work) and is not needed for rep.

## Open / to confirm

- **Signal → `x` mapping and baseline/overlay combine** (Decision #9) — pin how the contribution
  baseline (`local_score` / authored-content) and the ranking overlay (`ranks.ranking_scores`) each
  normalize to `x ∈ [0, 1]`, the combine rule, and the thin-ranked-space guard. The Phase 3 gate.
- **Contribution-baseline build-out** — authored-content aggregation (Finding #6) needs implementing
  and is forward-only; decide whether v1 ships it or bootstraps the Expert band on the ranking overlay
  until it lands.
- **Rep exposure as a system property** (Decision #10) — confirm with product whether rep is consumed
  through the generic entity/graph API (publish the `Reputation` property) or a dedicated field
  (table + GraphQL only).
- **`points_earned` summation** (Decision #2) — blocked on payout-entity summability; the Verified
  band ships flat `2` until then.
- **Recompute cardinality** (Decision #7) — measure user × space before committing to full recompute
  permanently.
- **Curve steepness / cap as policy** — `k = 5` and `EXPERT_MAX = 980` set a near-`980:1` influence
  ratio and are currently chosen by shape, not argument. Decide the intended top-to-median ratio
  explicitly before Phase 4 enables weighting; treat `k` and the cap as reviewable policy.
- **Community share** — circulate the design for community feedback (a design-review action item).

*Resolved:* endorse-a-person surface is in scope (review 2026-06-24); Person-entity resolution
(Finding #8); negative/below-median semantics (Decision #3, confirmed 2026-06-24); weighting
composition (Decision #4); rankings/scoring pipeline separation (Finding #9); Expert-band signal model
— contribution baseline + ranking overlay (Decision #8, 2026-06-30); rep exposure (Decision #10,
2026-06-30).

## References

### Internal

- Scoring engine: `scoring-service/cronjob/src/algorithm/{models.py,scoring.py}`, `main.py`
- Vote-weight hook: `scoring-service/cronjob/src/algorithm/models.py` (`Vote.weight`)
- Ranking pipeline: `ranking-indexer/src/{detect.rs,recompute.rs,scoring.rs,publish.rs,storage.rs}`,
  `ranks.*` schema (`api/drizzle/0061_create_ranks_schema.sql`)
- Atlas transitive closure: `atlas/src/graph/{transitive.rs,canonical.rs,state.rs}`, `persistence.rs`
- Trust pipeline: `hermes-pipeline/src/pipelines/trust.rs`, `kg-indexer/src/handlers/subspaces.rs`
- Verified-edges schema: `subspaces` table (`api/drizzle/0045_hard_annihilus.sql`)
- Scores schema: `api/src/services/storage/schema.ts` (`global_scores`, `local_scores`, `space_scores`)

### Spec

- Notion *Rep* design note (status: Curator) — bands, exponential distribution, feature list.
