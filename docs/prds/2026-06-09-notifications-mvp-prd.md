# Notifications MVP

**One-liner:** Stand up a Geo Browser app server that ingests notifications from the Gaia delivery worker, persists them, and delivers them to users via in-app feed, push (AWS SNS), and email (MailerSend) — starting with three governance notification types.

**Status:** Draft for team review · **Owner:** Neo (eng) · Yaniv & Preston (product) · **Date:** 2026-06-09

## Why
- Geo users have no way to find out about space activity without manually opening the app and polling. ([GEO-2172](https://linear.app/geobrowser/issue/GEO-2172/notification-product-requirements))
- The Gaia notification-service (indexer + delivery worker) already produces per-user webhook notifications, but **no app server consumes them yet** — the last mile to the user doesn't exist.
- Product picked the first three notification types to focus the work ([Slack thread](https://defi-wonderland.slack.com/archives/C08K8NM5477/p1780963820491619)): membership requests, editorship requests, new proposals for editors.
- For: Geo Browser users who are **editors** of a space and need to act on incoming governance.

## Scope
**In:**
- A new **Geo Browser app server** with its own Postgres DB that receives webhooks from the Gaia delivery worker and stores notifications.
- Three notification types (all are `proposal_created` events; see "Who gets what" below): **membership requests**, **editorship requests**, **new proposals for editors**.
- Three delivery channels — **in-app feed**, **push (AWS SNS)**, **email (MailerSend)** — abstracted behind a provider interface so the concrete services are swappable. **All channels default on.**
- Per-user, per-channel **preferences**: a user can enable/disable each channel.
- An **identity store** owned by the app server (Geo has no identity service today): the front-end upserts the user on sign-up/login, persisting `privy_user_id` ↔ `user_space_id` (their personal space) ↔ email, plus **0..N SNS device tokens (one per device/browser)**. This is what lets the app server resolve an inbound webhook's `user_space_id` to a reachable user and lets the front-end fetch "my notifications".
- Read APIs: list notifications (newest first, limit 100), unread count (for the badge), mark one-or-many read, mark all read.
- Identity/registration APIs: upsert user, register/unregister a push token.

**Out:**
- Other notification types from GEO-2172 (bounties, points, votes, comments, proposal-outcome-to-proposer, trending) — explicitly deferred.
- Notifying the **requester** when their membership/editorship request is approved or rejected (that's a future proposal-outcome type).
- App servers for any front-end other than Geo Browser. (Each front-end gets its own app server later; we build one now.)
- Changes to the on-chain contracts or the Gaia notification-indexer/delivery-worker (they already emit what we need — see Open questions for the one payload detail to confirm).
- A webhook self-registration API (the Geo Browser webhook is seeded manually into `app_webhooks`, per current v1).

## Architecture
The Gaia notification-service (left of the dashed line) already exists. Everything in the **Geo Browser app server** and the channels/UI (right) is new for this MVP.

```mermaid
flowchart LR
    A[On-Chain Event<br/>proposal_created] --> K[Kafka<br/>space.governance]
    K --> NI[notification-indexer<br/>resolve editors, fan out]
    NI --> OB[(notification_outbox)]
    OB --> DW[delivery-worker<br/>POST webhook]

    subgraph NEW["Geo Browser app server (NEW)"]
        WR[Webhook receiver<br/>verify + dedupe] --> DB[(App Postgres<br/>notifications · identity · prefs)]
        DB --> PA{Provider abstraction<br/>per-user prefs}
        API[Read/Write APIs<br/>Privy-auth]
        DB <--> API
    end

    DW -->|webhook per editor| WR

    PA -->|in-app| FEED[Feed + Badge]
    PA -->|push| SNS[AWS SNS]
    PA -->|email| MS[MailerSend]

    FEED --> U((Editor))
    SNS --> U
    MS --> U
    API <-->|Bearer Privy token| FE[Geo Browser UI]
    FE --> U

    class A,K,NI,OB,DW exist
    classDef exist fill:#eee,stroke:#999,color:#333
    style NEW fill:#f0f7ff,stroke:#3b82f6
```

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
- ✅ **Indexer already emits all three.** The notification-indexer emits `proposal_created` → editors of the space, and the payload **includes the `actions` array** with `type` (`add_member`/`add_editor`/…) and `target_address` (`notification-indexer/src/models.rs`). No indexer or contract change is needed for the MVP.
- 🔨 **To build:** the Geo Browser app server (everything downstream of the webhook) and the Geo Browser UI surfaces. The app server's webhook row must be seeded into `app_webhooks`.

## User flow
- An on-chain governance event (a new proposal — e.g. someone requests membership/editorship) is indexed by the Gaia notification-indexer, which creates one outbox row per editor of the space.
- The delivery worker POSTs a webhook to the Geo Browser app server, one call per editor per event.
- The app server verifies the webhook is authentic, deduplicates, resolves the `user_space_id` to a local user, and persists the notification.
- The app server fans the notification out to the user's **enabled** channels: writes it to the in-app feed, and/or sends a push via SNS to every registered device, and/or an email via MailerSend.
- In Geo Browser, the user sees the notification (feed and/or unread badge) and can mark it read; reads sync back to the app server.

```mermaid
sequenceDiagram
    participant Chain as On-Chain
    participant NI as notification-indexer
    participant DW as delivery-worker
    participant AS as App server
    participant DB as App Postgres
    participant CH as Channels (in-app / SNS / MailerSend)

    Chain->>NI: proposal_created (space_id, actions)
    NI->>NI: resolve editors of space → fan out (1 per editor)
    NI->>DW: outbox rows
    loop per editor
        DW->>AS: POST webhook
        AS->>AS: authenticate webhook
        AS->>DB: persist notification (for user_space_id)
        AS->>DB: read user's channel preferences
        AS->>CH: deliver to enabled channels (push → all devices)
        AS-->>DW: 2xx (ack)
    end
```

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
- **New Geo Browser app server:** webhook receiver (authenticate + dedupe), Postgres persistence, Privy↔user identity mapping, **server-side Privy access-token verification on user-scoped endpoints (net-new for the Geo stack)**, per-channel preferences, a provider-abstracted last-mile delivery layer (MailerSend for email, AWS SNS for push), and the read APIs. Unblocked once the identity-mapping source of truth (below) is decided.

### Smart contracts
- **No work.** The required events already exist on-chain and the indexer already consumes them.

## App server API (feature-level)
For the authenticated user (R = read, W = write):
- **List notifications** (R) — the user's notifications, newest first, limit 100.
- **Unread count** (R) — count of unread notifications (backs the badge).
- **Mark read** (W) — accepts one or more notification IDs.
- **Mark all read** (W) — marks all of the user's notifications read.
- **Preferences** (R/W) — read or update per-channel enable/disable (in-app, push, email).
- **Upsert user** (W) — register/update the caller's identity record. The front-end sends `user_space_id` (personal space); the **`privy_user_id` and email are derived server-side from the verified Privy token** (see Authentication) — the email is **never** trusted from the request body. (Push tokens are managed via the register endpoint below.) Called by the front-end on sign-up/login.
- **Register / unregister push token** (W) — *register* adds an SNS device token (idempotent; dedupe by token), *unregister* removes one. A user can have **multiple registered at once** (one per device).

Plus an **inbound webhook receiver** (not user-facing): authenticates the webhook, dedupes, persists, and fans out to enabled channels.

### Authentication (Privy server-side verification — new capability)
Every user-scoped endpoint identifies the caller by **verifying a Privy access token server-side** — a capability that **does not exist anywhere in the Geo stack today** (geobrowser only validates a Privy session client-side; its Next.js server trusts an `httpOnly` wallet-address cookie that carries no proof-of-possession and is scoped to the geobrowser origin, so it cannot be reused by a separate app server).

- The front-end calls Privy's `getAccessToken()` and sends it as a `Bearer` token.
- The app server verifies the JWT with `@privy-io/server-auth` + `PRIVY_APP_SECRET`, extracts the Privy user ID from the verified token, and resolves it to a `user_space_id` via the identity store.
- **All write/POST endpoints (preferences update, mark-read, mark-all-read, upsert user, push-token register/unregister) MUST verify the token before mutating, and MUST derive the acting user from the verified token — never from a client-supplied user/space ID in the request body.** Reads are scoped to the same verified identity.
- The inbound **webhook receiver is exempt** from Privy auth — it is authenticated as a webhook from the delivery worker instead (see `WEBHOOK_INTEGRATION.md`).

Exact request/response schemas and pagination beyond the 100-item cap are for the app server's own tech-design doc.

```mermaid
sequenceDiagram
    participant FE as Geo Browser UI
    participant Privy as Privy
    participant AS as App server
    participant DB as App Postgres

    Note over FE,DB: Phase 1 — on sign-up / login
    FE->>Privy: login (email) → user.id, embedded wallet
    FE->>FE: resolve personal space → user_space_id
    FE->>AS: POST upsert user {user_space_id} + Bearer access token
    AS->>AS: verify token → privy_user_id (sub claim)
    AS->>Privy: getUserById(privy_user_id) → email
    AS->>DB: store identity (privy_user_id, user_space_id, email)
    FE->>AS: POST register push token (per device) + Bearer
    AS->>DB: add SNS token to user (0..N)

    Note over FE,DB: Phase 2 — any user-scoped API call
    FE->>Privy: getAccessToken()
    FE->>AS: GET/POST + Bearer access token
    AS->>AS: verify JWT (@privy-io/server-auth + PRIVY_APP_SECRET)
    AS->>DB: resolve privy_user_id → user_space_id
    AS-->>FE: notifications / unread count / ack (scoped to verified user)
```

## Cross-team interfaces
Integration points that need a joint technical design before implementation. Name the touchpoint; the shapes live in the follow-up tech design.

- **Delivery Worker → App Server (webhook):** Already specified in `notification-service/WEBHOOK_INTEGRATION.md` (payload, authentication, idempotency, and retry semantics). Integration items: (a) seed the Geo Browser row in `app_webhooks`; (b) confirm the `proposal_created` payload carries the `actions` array so the app server can tell membership vs. editorship vs. generic proposal apart.
- **Identity mapping (App Server DB):** the app server is the source of truth — Geo has no identity service today. The front-end already has the user's Privy identity, email, and personal space ID, and upserts it on sign-up/login. Interface item: the upsert payload shape and when it fires.
- **App Server → UI:** the front-end presents a **Privy access token** (`getAccessToken()`) as a Bearer credential; the app server verifies it server-side (`@privy-io/server-auth` + `PRIVY_APP_SECRET`) and derives the acting user from the token. Interface item: token format/claims the app server expects and error/refresh handling on the UI side. (See [Authentication](#authentication-privy-server-side-verification--new-capability).)
- **App Server → Providers:** a provider abstraction in app-server code with two concrete implementations for MVP — **MailerSend** (email) and **AWS SNS** (push) — so providers can be swapped without touching delivery logic. (Note: SES is reserved for calendar events; not used here.)

## Dependencies & sequencing
1. **Product** resolves channel defaults + in-app surface + identity-mapping source of truth (blocks UI and backend design).
2. **Backend** confirms the webhook `actions` payload detail and seeds the Geo Browser webhook (blocks notification classification).
3. **Backend** builds the app server: webhook receiver + Postgres → identity mapping → preferences → provider-abstracted delivery → read APIs.
4. **UI** builds the feed/badge + preferences screen against the app server APIs (blocked on 1 and the API shape from 3).

## Assumptions
- All three MVP types are `proposal_created` events distinguished by `actions`; the indexer already fans out to editors and the payload already carries `actions`, so no indexer or contract change is needed for MVP. *(Verified against `notification-indexer/src/models.rs`.)*
- Geo Browser authenticates via Privy with **email login**, so every user has a Privy email and an embedded wallet — both available to the front-end to send in the upsert.
- The front-end can resolve the user's personal space ID, and this equals the `user_space_id` notifications are addressed to.
- Push tokens come from device/browser registration on the front-end. Email is **resolved server-side from Privy** via the verified `privy_user_id` (`getUserById`) at upsert and saved — not accepted from the client. Email-login means Privy's email is verified, so the saved value is trustworthy. MailerSend is already configured; AWS SNS is the push provider.
- The Geo Browser webhook is seeded manually into `app_webhooks` (no registration API in v1).
- Privy issues a verifiable access token (`getAccessToken()`) and `@privy-io/server-auth` + `PRIVY_APP_SECRET` can validate it server-side. No existing Geo backend verifies Privy today, so the app server adds this from scratch.

## Open questions
- [backend] **Email freshness:** can users change their email with privy + Geo login?
- [backend] **Push token lifecycle (approach decided):** the front-end registers a device's SNS token on login and on rotation; SNS auto-disables dead endpoints (`Enabled=false`), so the app prunes a token when a publish hits `EndpointDisabledException`, keeping the user's other devices. Detection mechanics live in the tech-design doc.
- [ui] **Auth — remaining detail:** token-refresh/expiry handling on the UI (approach decided — see Authentication)
- [product] Confirm requester-facing outcome notifications ("your request was approved/rejected") are out of MVP. *(Treated as out per current scope.)*
- [backend] **App server hosting:** where should the Geo Browser app server run? Recommendation: **co-locate it on the same infrastructure as the Geo Browser front-end** — shared deploy/ops pipeline, same region/network, and simpler secrets and on-call — rather than standing up separate infra.
- [product] **When should an email be sent?** Assume every notification lands in the in-app feed/badge. Email is the noisier channel — should it fire for every notification, only certain types, or as a batched digest / only when still unread after a delay? Needs a triggering policy (subject to the user's email toggle).

## Out of scope for this PRD
- App-server data model (table columns) and concrete API schemas — belong in the app server tech-design doc. (Auth *direction* is decided here; detailed token validation/refresh handling is for that doc.)
- The other GEO-2172 notification types (bounties, points, votes, comments, trending, proposal-outcome).
- Notifications for anyone other than Geo Browser users — the MVP covers Geo Browser users only (other front-ends each get their own app server later).
- Webhook self-registration API, subscription/event-type filtering, and per-webhook rate limiting (tracked as open questions in the notification-service tech design).
- **Non-Privy / external wallet authentication.** Supporting users who interact with Geo Browser through an external wallet (e.g. MetaMask) instead of Privy email-login. The MVP assumes Privy email-login — every user has a verified Privy email and an embedded wallet, which both the identity store and the server-side email resolution depend on. Adding external wallets would require an alternative auth path — likely a **wallet-signature ownership proof (SIWE / `personal_sign`-style, as used by other DeFi front-ends)** in place of the Privy Bearer token — and treating email as optional (wallet-only users would have no email channel and would rely on in-app + push). Deferred to a future iteration.
