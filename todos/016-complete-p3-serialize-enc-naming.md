---
status: complete
priority: p3
issue_id: "016"
tags: [code-review, naming, cleanup, serialize]
dependencies: []
---

# enc()/encOpt() naming in serialize.ts

## Problem Statement

`enc` in `serialize.ts` is a one-line wrapper around `toBase58` that adds zero value. Either inline it or rename to something descriptive. `encOpt` has actual null-handling logic so it earns existence but should be renamed to something like `toBase58Opt`.

## Findings

- `enc()` wraps `toBase58()` with no additional logic — adds indirection without value
- `encOpt()` handles null/undefined and has actual utility
- Short names like `enc` are unclear outside the file context
- Found by: code-simplicity-reviewer, pattern-recognition-specialist, tigerstyle-reviewer

## Proposed Solutions

### Option 1: Inline enc, rename encOpt

**Approach:** Remove `enc`, use `toBase58` directly. Rename `encOpt` to `toBase58Opt` or `toBase58OrNull`.

**Pros:**
- Less indirection
- Clearer naming

**Cons:**
- Minor churn in file

**Effort:** 15 minutes

**Risk:** Low

## Recommended Action

*To be filled during triage.*

## Technical Details

**Affected files:**
- `api/src/versioned/serialize.ts` — `enc` and `encOpt` functions

## Acceptance Criteria

- [ ] `enc` removed or justified
- [ ] `encOpt` has a descriptive name
- [ ] All existing tests pass

## Work Log

### 2026-02-09 - Code Review Discovery

**By:** Claude Code (8-agent review)

**Actions:**
- Identified trivial wrapper adding no value
- Confirmed encOpt has actual null-handling logic worth preserving
