# Gas Sponsorship Without Losing EOA Identity

## TL;DR

- **All providers (Privy, Openfort, ZeroDev) use EIP-7702 + ERC-4337** to enable gas sponsorship while preserving your EOA address
- **Two wallet types exist**: Embedded (user-controlled, non-custodial) and Backend (developer-controlled, custodial)
- **Embedded wallets require client-side auth** - you cannot use them from pure server/CLI without the user authenticating on a client first
- **Cross-app access** (web + CLI with same wallet) requires either: (a) a backend wallet, or (b) user grants permission once via browser
- **Privy** has the simplest DX (`sponsor: true`), **Openfort** is fully open source and self-hostable

---

## Key Concepts

### Wallet Types: Embedded vs Backend

This is the most important distinction to understand:

| Aspect                | Embedded Wallet               | Backend Wallet                        |
| --------------------- | ----------------------------- | ------------------------------------- |
| **Who controls it**   | The user                      | You (the developer/org with API keys) |
| **Who can sign**      | User must authenticate        | Your server, anytime                  |
| **Custody model**     | Non-custodial                 | Custodial                             |
| **Key storage**       | Split between user + provider | Provider infrastructure, your access  |
| **Created by**        | User during signup            | Your server via API                   |
| **Use cases**         | User assets, DeFi, gaming     | Treasury, bots, automation, agents    |
| **Frontend required** | ✅ Yes (for auth)             | ❌ No                                 |

**They are NOT interchangeable.** Different addresses, different control models, different use cases.

```
EMBEDDED WALLET                         BACKEND WALLET
===============                         ==============

     User                                Your Server
       │                                      │
       │ (must authenticate)                  │ (has full access)
       ▼                                      ▼
  ┌──────────┐                           ┌──────────┐
  │  Wallet  │ ◄─ User controls          │  Wallet  │ ◄─ You control
  │ 0xUSER.. │    User owns assets       │ 0xSRVR.. │    You own assets
  └──────────┘                           └──────────┘
```

### Why Embedded Wallets Require Client-Side Auth

Embedded wallets are **non-custodial by design**. The private key is split:

- Part held by user (decrypted via their auth)
- Part held by provider

The key is **only reconstructed in the client** (browser, mobile app). This is intentional:

- If your server could access embedded wallets without user auth, so could anyone with your API keys
- That would make it custodial, defeating the purpose

**Bottom line**: You cannot spin up a server-side version of an embedded wallet. Users must authenticate on a client at least once.

### Gas Sponsorship Architecture

All providers use the same pattern:

```
EOA ──signs 7702 auth──▶ EOA delegates to smart contract
                              │
                              ▼
                    EOA now implements IAccount (4337)
                              │
                              ▼
            UserOperation ──▶ Bundler ──▶ EntryPoint
                              │
                         Paymaster sponsors gas
```

**Your EOA address is preserved** - no new contract address is created.

---

## Provider Comparison

### Overview

| Capability                | Privy                  | Openfort            | ZeroDev        |
| ------------------------- | ---------------------- | ------------------- | -------------- |
| **EOA Address Preserved** | ✅                     | ✅                  | ✅             |
| **Gas Sponsorship DX**    | ⭐⭐⭐ `sponsor: true` | ⭐⭐ More explicit  | ⭐ Most manual |
| **Delegation Contract**   | Kernel (ZeroDev's)     | OPF7702 (their own) | Kernel         |
| **Open Source SDK**       | ❌                     | ✅                  | Partial        |
| **Open Source Contracts** | ❌                     | ✅                  | ✅             |
| **Self-Hostable**         | ❌                     | ✅                  | ❌             |
| **Embedded Wallets**      | ✅                     | ✅                  | ❌ (auth only) |
| **Backend Wallets**       | ✅                     | ✅                  | ❌             |
| **Custom Auth (BYOA)**    | ✅                     | ✅                  | N/A            |

### Frontend DX

**Privy** - Simplest

```typescript
import { useSendTransaction } from "@privy-io/react-auth";

const { sendTransaction } = useSendTransaction();
await sendTransaction({ to: "0x...", value: 100000 }, { sponsor: true });
```

**Openfort** - More explicit

```typescript
// Configure provider with policy
<OpenfortProvider
  walletConfig={{
    ethereumProviderPolicyId: { [chainId]: 'pol_...' },
  }}
>

// Sign 7702 authorization + send
const { signAuthorization } = use7702Authorization();
await signAuthorization({ contractAddress, chainId, nonce });
```

**ZeroDev** - Most manual

```typescript
const authorization = await account.signAuthorization({
  chainId,
  nonce,
  address: kernelAddress,
});
const kernelClient = createKernelAccountClient({
  account,
  paymaster: paymasterClient,
});
await kernelClient.sendTransaction({ calls, authorization });
```

### Backend Wallet DX (Server-Controlled Wallets)

These are wallets **you control** - no user authentication needed.

**Privy**

```typescript
import { PrivyClient } from "@privy-io/node";

const privy = new PrivyClient({ appId, appSecret });

// Create a backend wallet owned by your authorization key
const { id, address } = await privy.wallets().create({
  chain_type: "ethereum",
  owner: { public_key: "your-p256-public-key" },
});

// Send a sponsored transaction
await privy
  .wallets()
  .ethereum()
  .sendTransaction(id, {
    caip2: "eip155:8453",
    params: {
      transaction: {
        to: "0x...",
        value: "0x2386F26FC10000",
      },
    },
    sponsor: true,
  });
```

**Openfort**

```typescript
import Openfort from "@openfort/openfort-node";

const openfort = new Openfort(secretKey, { walletSecret });

const account = await openfort.accounts.evm.backend.create({
  name: "Treasury",
});
await account.signMessage({ message: "Hello" });
```

---

## Cross-App Wallet Access

### Apps You Control (Easy)

If you control multiple apps (web, mobile, etc.), just implement the Privy/Openfort provider in each app. **Same user = same wallet address** across all your apps automatically.

```typescript
// App A (web)
<PrivyProvider appId="your-app-id">  // Same app ID
  {/* User logs in → gets wallet 0xABC... */}
</PrivyProvider>

// App B (mobile)
<PrivyProvider appId="your-app-id">  // Same app ID
  {/* Same user logs in → gets same wallet 0xABC... */}
</PrivyProvider>
```

No special setup needed - embedded wallets are tied to the user's identity, not the specific app instance.

### Leaving the Client (Hard)

The problem arises when you need to access an embedded wallet **outside a client environment** - like a CLI tool, server script, or third-party app you don't control.

| Wallet Type  | Client Apps    | Server / CLI                             |
| ------------ | -------------- | ---------------------------------------- |
| **Embedded** | ✅ Full access | ❌ No access (keys only exist in client) |
| **Backend**  | ✅ Via API     | ✅ Full access                           |

Embedded wallet keys are reconstructed client-side only. A server or CLI can't access the wallet at all - the keys simply don't exist outside the client.

### Solutions for Server/CLI Access

| Approach                   | Same Address | Non-custodial    | Client Required  |
| -------------------------- | ------------ | ---------------- | ---------------- |
| **Backend wallet**         | ✅           | ❌ (you control) | Never            |
| **Session keys / Signers** | ✅           | ✅               | Once (to grant)  |
| **Unified login portal**   | ✅           | ✅               | Once per device  |
| **Export private key**     | ✅           | ❌ (key exposed) | Once (to export) |

### Unified Login Portal (for access outside web apps we control)

Both Privy and Openfort support **custom auth / bring-your-own-auth**. You can:

1. Build a thin login portal using Privy/Openfort's auth UI
2. User logs in via browser → gets session token
3. CLI uses that token for wallet operations

```
CLI                         Login Portal                  Privy/Openfort
 │                              │                              │
 │  $ mycli login               │                              │
 │  Opens browser ──────────────►                              │
 │                              │                              │
 │                    User logs in ────────────────────────────►
 │                              │                              │
 │                    Gets token ◄─────────────────────────────│
 │                              │                              │
 │  Token returned ◄────────────│                              │
 │  (saved locally)             │                              │
 │                              │                              │
 │  $ mycli sign "hello"        │                              │
 │  Uses token ────────────────────────────────────────────────►
```

**Login Portal (Privy)**

```typescript
import { usePrivy } from '@privy-io/react-auth';

export default function CLILogin() {
  const { ready, authenticated, getAccessToken } = usePrivy();
  const { redirect_uri, state } = useRouter().query;

  useEffect(() => {
    if (ready && authenticated) {
      getAccessToken().then(token => {
        window.location.href = `${redirect_uri}?token=${token}&state=${state}`;
      });
    }
  }, [ready, authenticated]);

  return <div>Complete login to continue...</div>;
}
```

**Login Portal (Openfort)**

```typescript
import { useOpenfort } from '@openfort/react';

export default function CLILogin() {
  const { client, user, isLoading } = useOpenfort();
  const { redirect_uri, state } = useRouter().query;

  useEffect(() => {
    if (!isLoading && user) {
      client.getAccessToken().then(token => {
        window.location.href = `${redirect_uri}?token=${token}&state=${state}`;
      });
    }
  }, [isLoading, user]);

  return <div>Complete login to continue...</div>;
}
```

**CLI usage after login (accessing user's embedded wallet from server)**

```typescript
// Privy - use token in authorization context
await privy
  .wallets()
  .ethereum()
  .signMessage(walletId, {
    message: "Hello from CLI",
    authorization_context: { user_jwts: [savedToken] },
  });
```

Note: Openfort's server-side access to user embedded wallets requires encryption sessions - see their [automatic recovery session docs](https://www.openfort.io/docs/products/embedded-wallet/server/automatic-recovery-session).

---

## Customization

### Whitelabel / Branding

Both providers allow full whitelabel - remove all provider branding:

| Capability          | Privy               | Openfort                |
| ------------------- | ------------------- | ----------------------- |
| **Remove branding** | ✅                  | ✅                      |
| **Headless mode**   | ✅ Build with hooks | ✅ Build with hooks     |
| **Custom themes**   | CSS variables       | ConnectKit themes + CSS |

### Policy Engines

**Privy**: Rich rule-based policies - transfer limits, allowlists, calldata constraints, time-bound signers

**Openfort**: Gas policies - rate limits, contract function allowlists, ERC-20 gas payment

### Session Keys

**Privy**: "Signers" - authorization keys that sign within scoped policies

**Openfort**: Native EIP-7715 session keys with `useGrantPermissions`

---

## Migration & Portability

### Export (Leaving a Provider)

Both providers allow users to export their private keys and leave:

| Provider     | Export Embedded Wallet               | Export Backend Wallet |
| ------------ | ------------------------------------ | --------------------- |
| **Privy**    | ✅ `usePrivy().exportWallet()`       | ✅ Node SDK           |
| **Openfort** | ✅ `useWallets().exportPrivateKey()` | ✅ Node SDK           |

Exported keys can be imported into MetaMask, Rainbow, or any wallet that accepts raw private keys. **Same address preserved.**

### Import (Joining a Provider)

| Provider     | Import to Embedded Wallet          | Import to Backend Wallet |
| ------------ | ---------------------------------- | ------------------------ |
| **Privy**    | ✅ `useImportWallet()` or Node SDK | ✅ Node SDK              |
| **Openfort** | ❌ New address only                | ✅ Node SDK              |

**Privy** allows importing existing private keys into embedded wallets - same address preserved.

**Openfort** does not support importing keys into embedded wallets. Users get a new address and must transfer assets.

### Migration Scenarios

| From → To                 | Same Address? | Process                              |
| ------------------------- | ------------- | ------------------------------------ |
| Privy → Self-custody      | ✅ Yes        | Export key → import to MetaMask      |
| Openfort → Self-custody   | ✅ Yes        | Export key → import to MetaMask      |
| Privy → Openfort          | ❌ No         | New Openfort wallet, transfer assets |
| Openfort → Privy          | ✅ Yes        | Export key → import to Privy         |
| Any → Privy               | ✅ Yes        | Import key preserves address         |
| Any → Openfort (embedded) | ❌ No         | New address, transfer assets         |

### Key Insight

- **Privy**: Good exit AND good entry - import preserves addresses
- **Openfort**: Good exit, limited entry - embedded wallets always get new addresses

For migrations to Openfort, plan for users to transfer assets from old wallets to new ones.

---

## Open Questions

### For Openfort

1. Can native paymaster work with 7702-delegated EOAs? (Examples only show Pimlico)
2. Is a simpler DX planned? (e.g., `sponsor: true`)
3. Can you add our custom L3 to hosted infrastructure?

### For Privy

4. If we pay ZeroDev to support our L3, does Privy's `sponsor: true` automatically work?
5. Can signers be added without any frontend interaction?

### General

6. Is pure 7702 gas sponsorship possible without 4337 infrastructure?

---

## Documentation Links

### Privy

- [Gas Sponsorship](https://docs.privy.io/wallets/gas-and-asset-management/gas/overview)
- [Server Wallets](https://docs.privy.io/wallets/wallets/create/create-a-wallet)
- [Signers](https://docs.privy.io/wallets/using-wallets/signers/overview)
- [Custom Auth (JWT)](https://docs.privy.io/authentication/user-authentication/jwt-based-auth/overview)
- [Access Tokens](https://docs.privy.io/authentication/user-authentication/access-tokens)
- [Whitelabel](https://docs.privy.io/recipes/react/whitelabel)

### Openfort

- [7702 Wallets](https://www.openfort.io/docs/products/embedded-wallet/javascript/wallets)
- [Backend Wallets](https://www.openfort.io/docs/products/server)
- [Session Keys](https://www.openfort.io/docs/products/embedded-wallet/react/wallet/session-keys)
- [Shield (Key Splitting)](https://github.com/openfort-xyz/shield)
- [Custom Auth](https://www.openfort.io/docs/configuration/custom-auth/auth-token)
- [Self-Hosted (OpenSigner)](https://www.opensigner.dev)

### ZeroDev

- [7702 Examples](https://7702.zerodev.app/)
- [Kernel Contracts](https://github.com/zerodevapp/kernel)
