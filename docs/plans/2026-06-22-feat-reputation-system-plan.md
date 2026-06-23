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
| Per-entity, per-space scores | `scoring-service` Python cronjob → `local_scores`, `global_scores`, `space_scores` | **Expert band** — ⚠️ `local_scores` is keyed by the *voted-on content entity id*, **not** by a person/user. There is no person-entity score to read. Needs a new user-scoring derivation — see Verification findings. |
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

> ⚠️ **Blocked — premise refuted by verification (2026-06-23).** Two assumptions below are wrong
> against the live system; this band needs a design decision before it can be built. See
> **Verification findings**.
> - *No per-user score exists.* `local_scores` is keyed by the voted-on **content entity id**, not
>   by a person/user. There is no "person entity" in gaia's identity/scoring path; a user is a
>   `member_space_id`, never converted to a scored entity. A user-level score must be **newly
>   derived** (e.g. aggregate the scores of content the user authored / was voted on).
> - *Score range is not `[-1,1]`.* The produced `local_score` is a within-space z-score → sigmoid,
>   range `(0,1)`, **neutral at `0.5`**. There is no signed/negative half. "Downvoted" = `< 0.5`.

- "Expertise" = a **per-user, per-space expertise score** (to be defined — see above), mapped onto
  `x ∈ [0,1]` for the curve. The mapping must account for the `(0,1)`-neutral-`0.5` source range:
  e.g. remap `(0.5, 1] → (0, 1]` and treat `≤ 0.5` as zero.
- Users with no derivable expertise score get `expert_add = 0`.
- **Negative/downvoted score wipes the entire rep, not just this band.** "Negative" here means below
  the neutral midpoint by a deadband (`score < 0.5 − ε`), since the source has no true negatives.
  Rationale and guard: see Decisions #3.
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

### User → expertise score (⚠️ rewritten — verification refuted the original plan)

The original plan assumed the Expert band could read a user's **person entity `local_score`** via
`account/personal space → person entity id → local_scores[...]`. **Verification refuted this**
(finding #1): there is no person entity in gaia, and `local_scores` is keyed by the *voted-on content
entity id*, not by a user. The user identity in scoring is `member_space_id`.

So there is no existing per-user score to read; one must be **derived**. Candidate approaches (to
decide — this is the top open item):
- **Authored-content aggregation:** map `user → content entities they authored/edited in this space`
  (via `values`/`relations` provenance) → aggregate those entities' `local_scores`. Closest to "your
  contributions earned merit," but needs a reliable author→entity link.
- **Vote-target aggregation:** if a user *is* represented by a voted-on entity (e.g. their profile/
  front-page entity receives votes), read that entity's `local_score`. Depends on whether people get
  voted on as entities at all (not currently the case).
- **New Person/Account ingestion:** add a person-entity ingestion path (the geo-sdk has
  `PERSON_TYPE`/`ACCOUNT_TYPE`, unused by gaia) so users become first-class scored entities. Largest
  scope.

Whatever the source, its output range is `(0,1)`-neutral-`0.5` (finding #2) and must be remapped
before the exponential curve.

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
- **Per-user expertise score derivation** (🔴 blocked on the design decision — see *User → expertise
  score*; was "User → person entity resolution", which verification refuted).
- Compute the bands that are unblocked first: **Low-trust** (removal/unverify only) + **Verified
  gating** (flat `2`). Expert and the Verified variable component land once their blockers clear.
- Write `user_reputation`. Weighting still uniform (`1.0`).
- Characterization tests per band + combined, mirroring the scoring-service test style.

### Phase 4 — Reputation-weighted voting (the feedback loop)
- Wire `Vote.weight` from previous-cycle `user_reputation`; `rep = 0 ⇒ weight = 0`, no floor.
- Resolve distance-weighting composition (finding #4): disable `use_distance_weighting` or accept
  `rep × 0.8^distance` compounding.
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
- **Per-user expertise score (🔴 blocker, was "person-entity join").** Verification (finding #1)
  showed the assumed account→person-entity→score path does not exist — `local_scores` is
  content-keyed and there is no person entity. The Expert band cannot be built until a per-user
  expertise derivation is chosen (see *User → expertise score*). This blocks Phase 3's Expert band
  (other bands are unaffected).

## Decisions (resolved with Yaniv, 2026-06-22)

1. **Cap math → `EXPERT_MAX = 980`.** True 1000 cap. The `+1` from low-trust pushing the theoretical
   max to 1001 is absorbed by the storage clamp, so the displayed ceiling is exactly 1000.
2. **`points_earned` → on-chain payout entities.** Derivable from system properties — payouts
   modeled as system entities generated by on-chain actions. No summation mechanism exists yet, so
   the Verified band's variable `+8 ×` component is **deferred**; the band ships as a flat `2`
   (eligibility only) until payout summation lands.
3. **Negative score → wipe entire rep** (not Expert-band-only). ⚠️ *Revised by verification:* the
   source score has **no negative half** — it is `(0,1)` with neutral `0.5`. So "negative" must mean
   "downvoted below neutral by a deadband": `score < 0.5 − ε`. *Guard:* the deadband prevents
   noise-zeroing a verified professional; pushing a score below neutral already requires rep-weighted
   downvotes, which is somewhat self-protecting. Deadband value still open.
4. **Band stacking → additive** (as modeled). No strong preference expressed; keeping the simpler
   additive model. ⚠️ *Verification caveat on weighting composition:* production runs
   `use_distance_weighting=True` (with `filter_non_members=False`; the config forbids both). Distance
   weighting already **multiplies** `Vote.weight` by `0.8^distance`, so seeding `Vote.weight` from rep
   compounds multiplicatively (`rep × 0.8^distance`). Phase 4 must either disable distance weighting
   or consciously accept the compounding. ("Composes cleanly" only holds vs. `filter_non_members`.)
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

## Verification findings (codebase pass, 2026-06-23)

Five plan assumptions were checked against the live code. Three were refuted — two of them
materially. file:line evidence below.

**🔴 1. No per-user score exists — the Expert band has no data source.** `local_scores` is keyed by
the **voted-on content entity id**, not by a person/user (`scoring_data_writer.py:88-93`;
perspectives = `DISTINCT entity_id, space_id FROM values`, `scoring_data_provider.py:359-381`). A
user is identified throughout scoring by `member_space_id` (lowercased), never converted to an
entity id (`scoring_data_provider.py:238-300`; `models.py:33-39`). There is **no "person entity"** in
gaia's path: `PERSON_TYPE` exists in the geo-sdk but is referenced nowhere in gaia; profiles resolve
to a personal space's *front-page entity* (`TYPES_PROPERTY→SPACE_TYPE`), not a Person
(`api/src/profile/queries.ts:82-133`). The hypothesized join `user_space_id → person_entity_id →
local_scores` would return **null for essentially every user**. → *The Expert band requires a
new per-user expertise derivation (e.g. aggregate the local_scores of content the user authored / was
voted on). This is net-new work, not reuse.*

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

## Open / to confirm

- **🔴 Expert-band user score (finding #1)** — biggest open design item. How do we derive a per-user,
  per-space expertise score, given there is no person entity and `local_scores` is content-keyed?
- **Expert-band `x`-mapping (finding #2)** — confirm the `(0.5,1] → (0,1]`, treat-`≤0.5`-as-zero
  remap (or an alternative) once the user score is defined.
- **Weighting composition (finding #4)** — disable `use_distance_weighting` under rep-weighting, or
  accept `rep × 0.8^distance` compounding?
- **#3 deadband value** — what `ε` trips the full-rep wipe (now relative to `0.5`).
- **#7** — confirm full-recompute-first is acceptable; revisit after a cardinality measurement.
- **Discord** — share this proposed design in Discord for community feedback (Yaniv's ask).

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
