# Notifications MVP

**One-liner:** Stand up a Geo Browser app server that ingests notifications from the Gaia delivery worker, persists them, and delivers them to users via in-app feed, push (AWS SNS), and email (MailerSend) — starting with three governance notification types.

**Status:** Draft for team review · **Owner:** Yaco (eng) / Preston (product) · **Date:** 2026-06-09

## Why
- Geo users have no way to find out about space activity without manually opening the app and polling. ([GEO-2172](https://linear.app/geobrowser/issue/GEO-2172/notification-product-requirements))
- The Gaia notification-service (indexer + delivery worker) already produces per-user webhook notifications, but **no app server consumes them yet** — the last mile to the user doesn't exist.
- Product picked the first three notification types to focus the work ([Slack thread](https://defi-wonderland.slack.com/archives/C08K8NM5477/p1780963820491619)): membership requests, editorship requests, new proposals for editors.
- For: Geo Browser users who are **editors** of a space and need to act on incoming governance.

## Scope
**In:**
- A new **Geo Browser app server** with its own Postgres DB that receives signed webhooks from the Gaia delivery worker and stores notifications.
- Three notification types (all are `proposal_created` events; see "Who gets what" below): **membership requests**, **editorship requests**, **new proposals for editors**.
- Three delivery channels — **in-app feed**, **push (AWS SNS)**, **email (MailerSend)** — abstracted behind a provider interface so the concrete services are swappable. **All channels default on.**
- Per-user, per-channel **preferences**: a user can enable/disable each channel.
- An **identity store** owned by the app server (Geo has no identity service today): the front-end upserts the user on sign-up/login, persisting `privy_user_id` ↔ `user_space_id` (their personal space) ↔ email ↔ push token(s). This is what lets the app server resolve an inbound webhook's `user_space_id` to a reachable user and lets the front-end fetch "my notifications".
- Read APIs: list notifications (newest first, limit 100), unread count (for the badge), mark one-or-many read, mark all read.
- Identity/registration APIs: upsert user, register/unregister a push token.

**Out:**
- Other notification types from GEO-2172 (bounties, points, votes, comments, proposal-outcome-to-proposer, trending) — explicitly deferred.
- Notifying the **requester** when their membership/editorship request is approved or rejected (that's a future proposal-outcome type).
- App servers for any front-end other than Geo Browser. (Each front-end gets its own app server later; we build one now.)
- Changes to the on-chain contracts or the Gaia notification-indexer/delivery-worker (they already emit what we need — see Open questions for the one payload detail to confirm).
- A webhook self-registration API (the Geo Browser webhook is seeded manually into `app_webhooks`, per current v1).

## Who gets what (MVP notification types)
All three MVP types originate from a single on-chain event the indexer already handles — `proposal_created` — and the indexer fans out **one notification per editor of the proposal's space**. They differ only by what the proposal *does* (its `actions`):

| MVP notification type | Underlying event | Distinguished by | Recipients |
|---|---|---|---|
| **Membership request** | `proposal_created` | proposal contains an `add_member` action | **Editors of the space** (they vote on it) |
| **Editorship request** | `proposal_created` | proposal contains an `add_editor` action | **Editors of the space** (they vote on it) |
| **New proposal for editors** | `proposal_created` | any proposal | **Editors of the space** |

**Net:** for the MVP, every recipient is an **editor of the proposal's space**. The app server classifies which of the three labels to show by inspecting the proposal's `actions` array (confirmed present on `proposal_created` payloads — see Implementation status). Members (non-editors) and the requesting user receive nothing in the MVP.

**Classification rule (recommended):** if the proposal's `actions` contain an `add_editor` → *editorship request*; else if they contain an `add_member` → *membership request*; else → *new proposal*. A proposal with both is labeled by the higher-privilege action (editor > member). Final labeling/copy is a product decision.

### Implementation status (what exists today)
- ✅ **Indexer already emits all three.** The notification-indexer emits `proposal_created` → editors of the space, and the payload **includes the `actions` array** with `type` (`add_member`/`add_editor`/…) and `target_address` (`notification-indexer/src/models.rs`). No indexer or contract change is needed for the MVP. *(Note: `TECH_DESIGN.md`'s field table omits `actions`; the code and `WEBHOOK_INTEGRATION.md` are authoritative — worth a one-line doc fix.)*
- 🔨 **To build:** the Geo Browser app server (everything downstream of the webhook) and the Geo Browser UI surfaces. The app server's webhook row must be seeded into `app_webhooks`.

## User flow
- An on-chain governance event (a new proposal — e.g. someone requests membership/editorship) is indexed by the Gaia notification-indexer, which creates one outbox row per editor of the space.
- The delivery worker POSTs a signed webhook to the Geo Browser app server, one call per editor per event.
- The app server verifies the HMAC signature, deduplicates on `idempotency_key`, resolves the `user_space_id` to a local user, and persists the notification.
- The app server fans the notification out to the user's **enabled** channels: writes it to the in-app feed, and/or sends a push via SNS, and/or an email via MailerSend.
- In Geo Browser, the user sees the notification (feed and/or unread badge) and can mark it read; reads sync back to the app server.

## Team breakdown
Product drives the decisions here — the open questions below should be resolved before backend/UI lock their own designs.

### Product
- **Why this matters:** editors miss governance they're expected to vote on; proactive notifications drive participation and are the foundation for the broader engagement strategy in GEO-2172.
- **Success signal:** editors receive membership/editorship/proposal notifications within seconds of the on-chain event, across their enabled channels, with no duplicates.
- **Decisions product owns (block everyone else):**
  - ~~**Channel default:**~~ **Decided:** all channels (in-app, push, email) **default on** for a new user.
  - ~~**In-app surface this iteration:**~~ **Decided:** **both** — a badge showing unread count, and a feed listing the user's notifications by time.
  - **Notification copy/labeling:** how a membership request vs. editorship request vs. generic proposal reads to the user (classification rule recommended above).
  - **Requester notifications:** confirm these are out of MVP (no "your request was approved" yet).

### UI (Geo Genesis web)
- Build two in-app surfaces — a **badge** (unread count, from the unread-count API) and a **feed** (user's notifications newest-first, from the list API) — plus a preferences screen to toggle channels. On sign-up/login, **upsert the user to the app server** with their Privy ID, personal space ID (`user_space_id`), email, and push token. Unblocked once the API shape is locked.

### Backend
- **Gaia notification-service:** already emits `proposal_created` → editors; no indexer change expected for MVP. Seed the Geo Browser webhook into `app_webhooks`. (Confirm the payload detail in Open questions.)
- **New Geo Browser app server:** webhook receiver (HMAC verify + idempotency), Postgres persistence, Privy↔user identity mapping, per-channel preferences, a provider-abstracted last-mile delivery layer (MailerSend for email, AWS SNS for push), and the read APIs. Unblocked once the identity-mapping source of truth (below) is decided.

### Smart contracts
- **No work.** The required events already exist on-chain and the indexer already consumes them.

## App server API (feature-level)
For the authenticated user:
- **List notifications** — the user's notifications, newest first, limit 100.
- **Unread count** — count of unread notifications (backs the badge).
- **Mark read** — accepts one or more notification IDs.
- **Mark all read** — marks all of the user's notifications read.
- **Preferences** — read/update per-channel enable/disable (in-app, push, email).
- **Upsert user** — register/update the caller's identity record: `privy_user_id`, `user_space_id` (personal space), email, and optional push token. Called by the front-end on sign-up/login.
- **Register / unregister push token** — add or remove an SNS device token for the user.

Plus an **inbound webhook receiver** (not user-facing): verifies `X-Geo-Signature`, dedupes on `idempotency_key`, persists, and fans out to enabled channels.

Exact request/response schemas, auth, and pagination beyond the 100-item cap are for the app server's own tech-design doc.

## Cross-team interfaces
Integration points that need a joint technical design before implementation. Name the touchpoint; the shapes live in the follow-up tech design.

- **Delivery Worker → App Server (webhook):** Already specified in `notification-service/WEBHOOK_INTEGRATION.md` (payload, `X-Geo-Signature` HMAC, `idempotency_key`, retry/409 semantics). Integration items: (a) seed the Geo Browser row in `app_webhooks`; (b) confirm the `proposal_created` payload carries the `actions` array so the app server can tell membership vs. editorship vs. generic proposal apart.
- **Identity mapping (App Server DB):** the app server is the source of truth — Geo has no identity service today. The front-end already holds the full binding (`usePrivy()` → Privy `user.id` + email; `usePersonalSpaceId()` → `user_space_id`; embedded wallet address) and upserts it on sign-up/login. Interface item: the upsert payload shape and when it fires.
- **App Server → UI:** how the front-end authenticates "my notifications" / unread-count / mark-read calls and how preferences are read/updated.
- **App Server → Providers:** a provider abstraction in app-server code with two concrete implementations for MVP — **MailerSend** (email) and **AWS SNS** (push) — so providers can be swapped without touching delivery logic. (Note: SES is reserved for calendar events; not used here.)

## Dependencies & sequencing
1. **Product** resolves channel defaults + in-app surface + identity-mapping source of truth (blocks UI and backend design).
2. **Backend** confirms the webhook `actions` payload detail and seeds the Geo Browser webhook (blocks notification classification).
3. **Backend** builds the app server: webhook receiver + Postgres → identity mapping → preferences → provider-abstracted delivery → read APIs.
4. **UI** builds the feed/badge + preferences screen against the app server APIs (blocked on 1 and the API shape from 3).

## Assumptions
- All three MVP types are `proposal_created` events distinguished by `actions`; the indexer already fans out to editors and the payload already carries `actions`, so no indexer or contract change is needed for MVP. *(Verified against `notification-indexer/src/models.rs`.)*
- Geo Browser authenticates via Privy with **email login**, so every user has a Privy email and an embedded wallet — both available to the front-end to send in the upsert.
- The front-end's `usePersonalSpaceId()` returns the user's personal space ID and this equals the `user_space_id` notifications are addressed to.
- Push tokens come from device/browser registration on the front-end; email comes from Privy (or the upsert). MailerSend is already configured; AWS SNS is the push provider.
- The Geo Browser webhook is seeded manually into `app_webhooks` (no registration API in v1).

## Open questions
- [product] **Labeling/copy** for membership request vs. editorship request vs. generic proposal (classification rule recommended above is the default).
- [backend] **Email source per send:** read the user's email from the stored identity record, or fetch live from Privy via stored `privy_user_id` at send time? (Storing it on upsert is simplest; Privy stays the fallback.)
- [backend] **Push token lifecycle:** how web push tokens are obtained/refreshed and registered with SNS (browser push vs. native), and how stale tokens are pruned.
- [ui] **Auth for app-server calls:** what credential the front-end presents (Privy access token verified server-side?) so the app server can trust the caller's identity.
- [product] Confirm requester-facing outcome notifications ("your request was approved/rejected") are out of MVP. *(Treated as out per current scope.)*
- [all] Each cross-team interface above needs a joint tech-design session before implementation starts.

## Out of scope for this PRD
- App-server data model (table columns), auth scheme, and concrete API schemas — belong in the app server tech-design doc.
- The other GEO-2172 notification types (bounties, points, votes, comments, trending, proposal-outcome).
- Multi-app-server fan-out (additional front-ends each get their own app server later).
- Webhook self-registration API, subscription/event-type filtering, and per-webhook rate limiting (tracked as open questions in the notification-service tech design).
