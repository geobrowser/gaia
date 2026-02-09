---
status: complete
priority: p3
issue_id: "020"
tags: [code-review, debugging, security, uuid]
dependencies: []
---

# toUuid error message discards input (intentional vs debuggability tradeoff)

## Problem Statement

`toUuid` throws `"Invalid UUID format"` without any input context. Compare with `encodeBase58` which includes input. For debugging, including a truncated input hint would help — but this was an intentional security decision in commit 3 to not echo raw user input.

## Findings

- Commit `24c0a92` intentionally removed raw input from error messages
- Review agents disagree: tigerstyle and pattern-recognition want input context; security-sentinel appreciates the omission
- Middle ground: include only length/format hint (e.g. `"Invalid UUID format (length=47, starts with '0x')"`) without echoing full input
- Found by: tigerstyle-reviewer, pattern-recognition-specialist

## Proposed Solutions

### Option 1: Include format hint without full input

**Approach:** `throw new Error(\`Invalid UUID format (length=${value.length})\`)` — gives debugging context without echoing potentially malicious input.

**Effort:** 10 minutes

**Risk:** Low

---

### Option 2: Leave as-is (security-first)

**Approach:** Keep current behavior. Error messages should not include user input.

**Effort:** 0

**Risk:** Low

## Recommended Action

*To be filled during triage.*

## Technical Details

**Affected files:**
- `api/src/utils/uuid.ts` — `toUuid` error throw

## Acceptance Criteria

- [ ] Error message provides useful debugging context OR decision to keep as-is is documented
- [ ] No raw user input in error messages

## Work Log

### 2026-02-09 - Code Review Discovery

**By:** Claude Code (8-agent review)

**Actions:**
- Noted intentional security decision from commit 3
- Documented reviewer disagreement on approach
