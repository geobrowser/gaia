# Gas Sponsorship Without Losing EOA Identity

## Summary

All major wallet providers (Privy, Openfort, ZeroDev) use the same approach: **EIP-7702 delegation + ERC-4337 infrastructure**. The EOA signs an authorization to delegate to a smart contract (Kernel or OPF7702), which then enables gas sponsorship via 4337 paymasters. **The user's EOA address is preserved as their identity** - no new contract address is created.

The key differences are in DX, openness, and which delegation contract is used - but the underlying architecture is identical.

---

## Motivation

### Goals

- **Preserve EOA as user identity** - Users must have the same address across all apps in the protocol
- **Enable gas sponsorship** - Abstract away gas fees for better UX
- **Avoid smart account address indirection** - No separate contract addresses that differ from the signer

### Non-Goals

- Pure 7702 without 4337 - All providers use the hybrid approach
- Paying gas with ERC-20 tokens (out of scope for this comparison)
- Solana-specific considerations

---

## Provider Comparison

### Overview

| Capability                 | Privy                  | Openfort            | ZeroDev           |
| -------------------------- | ---------------------- | ------------------- | ----------------- |
| **EOA Address Preserved**  | ✅                     | ✅                  | ✅                |
| **Delegation Contract**    | Kernel (ZeroDev's)     | OPF7702 (their own) | Kernel            |
| **Gas Sponsorship**        | ✅ 4337 paymaster      | ✅ 4337 paymaster   | ✅ 4337 paymaster |
| **DX Simplicity**          | ⭐⭐⭐ `sponsor: true` | ⭐⭐ More explicit  | ⭐ Most manual    |
| **Self-Hostable**          | ❌                     | ✅                  | ❌                |
| **Open Source SDK**        | ❌                     | ✅                  | Partial           |
| **Open Source Contracts**  | ❌                     | ✅                  | ✅                |
| **Solana Support**         | ✅                     | ✅                  | ❌                |
| **Server/Backend Wallets** | ✅                     | ✅                  | ❌                |

### Architecture

**All three providers use the same pattern:**

```
EOA ──signs 7702 auth──▶ EOA delegates to smart contract code
                              │
                              ▼
                    EOA now implements IAccount (4337)
                              │
                              ▼
            UserOperation ──▶ Bundler ──▶ EntryPoint
                              │
                         Paymaster sponsors gas
```

**Key points:**

- **7702 delegation**: EOA signs authorization to temporarily delegate to contract code
- **4337 flow**: Delegated EOA implements `IAccount`, enabling UserOperations and paymasters
- **Address preserved**: User keeps their original EOA address (no new contract address)

**This is NOT pure 7702** - pure 7702 is just a new transaction type (0x04) that doesn't involve UserOperations. The hybrid approach layers 4337 on top of 7702 to enable paymaster-based gas sponsorship.

#### Delegation Contracts

| Provider     | Contract         | Features                                                                                |
| ------------ | ---------------- | --------------------------------------------------------------------------------------- |
| **Privy**    | Kernel (ZeroDev) | Session keys via plugins, multi-sig via plugins                                         |
| **Openfort** | OPF7702          | Multi-key native (EOA, WebAuthn, P256), spending limits built-in, session keys built-in |
| **ZeroDev**  | Kernel           | Session keys via plugins, extensive plugin ecosystem                                    |

**Source**: Privy docs explicitly state they use Kernel:

> "Your users receive embedded wallets that are upgraded to [Kernel smart contracts](https://github.com/zerodevapp/kernel)"
> — [Privy Gas Sponsorship Overview](https://docs.privy.io/wallets/gas-and-asset-management/gas/overview)

### Developer Experience

#### Frontend SDK

**Privy (React)** - Simplest

```typescript
import { useSendTransaction } from "@privy-io/react-auth";

const { sendTransaction } = useSendTransaction();

sendTransaction(
  { to: "0x...", value: 100000 },
  { sponsor: true }, // One flag
);
```

**Openfort (React)** - More explicit

```typescript
// 1. Configure provider with policy
<OpenfortProvider
  walletConfig={{
    accountType: AccountTypeEnum.EOA,
    ethereumProviderPolicyId: { [chainId]: 'pol_...' },
  }}
>

// 2. Sign 7702 authorization
const { signAuthorization } = use7702Authorization();
const auth = await signAuthorization({
  contractAddress: '0x...',
  chainId,
  nonce,
});

// 3. Send transaction
```

**ZeroDev (React/viem)** - Most manual

```typescript
const authorization = await account.signAuthorization({
  chainId,
  nonce,
  address: kernelAddress,
});

const kernelClient = createKernelAccountClient({
  account: kernelAccount,
  paymaster: paymasterClient,
});

await kernelClient.sendTransaction({ calls, authorization });
```

#### Server SDK

**Privy**

```typescript
await privy
  .wallets()
  .ethereum()
  .sendTransaction(walletId, {
    caip2: "eip155:1",
    params: { transaction: { to: "0x...", value: "0x..." } },
    sponsor: true,
  });
```

**Openfort**

```typescript
await openfort.transactionIntents.create({
  account: accountId,
  chainId: chainId,
  policy: "pol_...",
  signedAuthorization: serializedAuth,
  interactions: [
    { contract: "con_...", functionName: "mint", functionArgs: ["0x..."] },
  ],
});
```

### Open Source & Self-Hosting

| Aspect                  | Privy       | Openfort                                                                                                                   | ZeroDev                                        |
| ----------------------- | ----------- | -------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------- |
| **SDK Source**          | Closed      | [openfort-js](https://github.com/openfort-xyz/openfort-js), [openfort-node](https://github.com/openfort-xyz/openfort-node) | Partial                                        |
| **Contract Source**     | Uses Kernel | [openfort-7702-account](https://github.com/openfort-xyz/openfort-7702-account)                                             | [Kernel](https://github.com/zerodevapp/kernel) |
| **Self-host Infra**     | ❌          | ✅ [opensigner](https://github.com/openfort-xyz/opensigner)                                                                | ❌                                             |
| **Self-host Paymaster** | ❌          | ✅                                                                                                                         | ❌                                             |

**Openfort is the only provider offering full self-hosting capability** with their opensigner infrastructure.

### Documentation & Maturity

| Aspect                              | Privy    | Openfort                   | ZeroDev  |
| ----------------------------------- | -------- | -------------------------- | -------- |
| **7702 + Gas Sponsorship Docs**     | ✅ Clear | ⚠️ Gap                     | ✅ Clear |
| **Native Paymaster + 7702 Example** | ✅       | ❌ (only Pimlico examples) | ✅       |
| **Production Ready**                | ✅       | ✅                         | ✅       |

**Openfort documentation gap**: Their 7702 contracts implement `IAccount` and should work with their native paymaster (`pm_sponsorUserOperation`), but all examples use Pimlico instead. This is a documentation gap, not a capability gap.

### Customization & Extensibility

| Aspect                   | Privy                                           | Openfort                                         |
| ------------------------ | ----------------------------------------------- | ------------------------------------------------ |
| **UI Components**        | Pre-built, customizable via CSS                 | Pre-built (ConnectKit-based), themeable          |
| **Custom UI**            | Build your own with hooks                       | Build your own with hooks                        |
| **Whitelabel**           | ✅ Full (see [Branding](#branding--whitelabel)) | ✅ Full (see [Branding](#branding--whitelabel))  |
| **Auth Methods**         | Email, SMS, social, passkeys, external wallets  | Email, social, passkeys, guest, external wallets |
| **Third-party Auth**     | ✅ Bring your own auth                          | ✅ Bring your own auth                           |
| **Session Keys**         | ✅ Via signers + policies                       | ✅ Native (EIP-7715)                             |
| **Transaction Policies** | ✅ Rich policy engine                           | ✅ Gas policies + contract rules                 |
| **Key Export**           | ✅                                              | ✅                                               |

#### Policy Engine Comparison

**Privy** has a sophisticated policy engine with:

- Transfer limits (amount caps)
- Allowlists/denylists for recipients, contracts, networks
- Time-bound signers
- Calldata constraints (specific function parameters)
- EIP-712 typed data restrictions
- Multi-chain support (EVM, Solana, Tron, Sui)

```typescript
// Privy policy example - restrict to specific contract + function
{
  rules: [{
    method: 'eth_sendTransaction',
    conditions: [
      { field_source: 'ethereum_transaction', field: 'to', operator: 'eq', value: '0x...' },
      { field_source: 'ethereum_calldata', field: 'function_name', operator: 'eq', value: 'mint', abi: [...] }
    ],
    action: 'ALLOW'
  }]
}
```

**Openfort** has gas policies with:

- Sponsored transactions (pay for user)
- ERC-20 gas payment (dynamic or fixed rate)
- Contract function allowlists
- Rate limits (gas per interval, per tx, count per interval)
- Wildcard/catch-all sponsorship

```typescript
// Openfort policy - sponsor specific contract functions
curl https://api.openfort.io/v1/policy_rules \
  -d type="contract_functions" \
  -d functionName="mint" \
  -d "contract=con_..."
```

#### Session Keys Comparison

**Privy**: Uses "signers" - authorization keys that can take actions within scoped policies. Signers never see the private key; signing happens in secure enclaves.

**Openfort**: Native EIP-7715 session keys with `useGrantPermissions` hook. Supports contract-call permissions, expiry times, and custom policies.

```typescript
// Openfort session key example
const result = await grantPermissions({
  request: {
    signer: { type: "account", data: { id: sessionAccount.address } },
    expiry: 60 * 60 * 24, // 24 hours
    permissions: [
      {
        type: "contract-call",
        data: { address: "0x...", calls: [] },
        policies: [],
      },
    ],
  },
});
```

### Branding & Whitelabel

| Aspect                       | Privy                                   | Openfort                                 |
| ---------------------------- | --------------------------------------- | ---------------------------------------- |
| **Remove provider branding** | ✅ Full whitelabel                      | ✅ Full whitelabel                       |
| **Headless mode**            | ✅ Build your own UI with hooks         | ✅ Build your own UI with hooks          |
| **Pre-built UI theming**     | CSS variables, light/dark/custom themes | ConnectKit themes (7 options) + CSS vars |
| **Custom logo**              | ✅ Or remove entirely with `logo: ''`   | ✅ Via ConnectKit theming                |
| **Custom pages**             | Build from scratch with hooks           | ✅ Replace specific modal pages          |

#### Privy Whitelabel Options

**Fully headless** - build your own UI using hooks:

```typescript
// Complete control - no Privy UI at all
import { useLoginWithEmail, useLoginWithSocial } from "@privy-io/react-auth";

const { sendCode, loginWithCode } = useLoginWithEmail();
const { initOAuth } = useLoginWithSocial();

// Build your own login form, call these functions
await sendCode({ email: "user@example.com" });
await loginWithCode({ code: "123456" });
```

**Or customize their modal** via config:

```typescript
<PrivyProvider
  config={{
    appearance: {
      logo: '', // Empty string = no logo
      landingHeader: 'Welcome to Your App',
      theme: '#1a1a2e', // Custom hex color
    },
    embeddedWallets: {
      showWalletUIs: false, // Hide wallet UIs entirely
    },
  }}
>
```

- [Whitelabel docs](https://docs.privy.io/recipes/react/whitelabel)
- [Whitelabel starter repo](https://github.com/privy-io/examples/tree/main/privy-react-whitelabel-starter)
- [Live whitelabel demo](https://whitelabel.privy.io)

#### Openfort Whitelabel Options

**Fully headless** - use [authentication hooks](https://www.openfort.io/docs/products/embedded-wallet/react/auth) to build custom UI.

**Or customize their modal** (ConnectKit-based):

```typescript
<OpenfortProvider
  uiConfig={{
    theme: 'midnight', // or: soft, retro, minimal, web95, rounded, nouns
    mode: 'dark',
    customTheme: {
      '--ck-font-family': 'Inter, sans-serif',
      '--ck-color-background': '#0a0a0a',
    },
    // Replace specific pages with your own components
    customPageComponents: {
      connected: <YourCustomProfilePage />,
    },
  }}
>
```

- [UI Customization docs](https://www.openfort.io/docs/products/embedded-wallet/react/ui/customization)

### Custom L3 Support

| Aspect                   | Privy                   | Openfort                                                            | ZeroDev                                                    |
| ------------------------ | ----------------------- | ------------------------------------------------------------------- | ---------------------------------------------------------- |
| **Custom chain request** | Contact sales@privy.io  | Contact via [Telegram](https://t.me/joalavedra) - "24hr turnaround" | Contact via [Calendly](https://calendly.com/zerodev/30min) |
| **Contracts to deploy**  | Unknown (closed source) | EntryPoint + OPF7702 + Paymaster V3                                 | EntryPoint + Kernel + plugins                              |
| **Self-deploy option**   | ❌                      | ✅ All contracts open source                                        | ✅ Kernel is open source                                   |
| **Bundler for L3**       | Managed by Privy        | Self-host or use theirs                                             | Self-host or use theirs                                    |
| **Paymaster for L3**     | Managed by Privy        | Self-host or use theirs                                             | Self-host or use theirs                                    |

#### Contracts Required for Custom L3

**For 7702 + 4337 gas sponsorship, you need:**

1. **EntryPoint** (ERC-4337 standard) - Already deployed on most chains, or deploy yourself
2. **Delegation contract** (7702 target):
   - Privy: Kernel (ZeroDev deploys)
   - Openfort: OPF7702 - [addresses](https://www.openfort.io/docs/configuration/addresses)
   - ZeroDev: Kernel
3. **Paymaster** - For gas sponsorship
4. **Bundler** - Off-chain service to submit UserOperations

**Openfort deployed contract addresses:**

- OPF7702 (EPv8): `0x7702000152F33A40E1Fd30C70E708f624113aa68`
- OPF7702 (EPv9): `0x77020901f40BE88Df754E810dA9868933787652B`
- Paymaster V3 (EPv8): `0x8888fee873E7035789Db91C16b5dDDbad7214CDa`
- Paymaster V3 (EPv9): `0x9999feeE50Fc515023F207b1c61aB3eA419e27d0`

#### Support & Maintenance

| Aspect                  | Privy                    | Openfort                                         | ZeroDev                                        |
| ----------------------- | ------------------------ | ------------------------------------------------ | ---------------------------------------------- |
| **Integration contact** | sales@privy.io           | [Telegram: @joalavedra](https://t.me/joalavedra) | [Calendly](https://calendly.com/zerodev/30min) |
| **Support channels**    | Enterprise support plans | Telegram, GitHub                                 | Discord, GitHub                                |
| **SLA options**         | Enterprise tier          | Unknown                                          | Unknown                                        |
| **Self-host docs**      | N/A                      | [OpenSigner docs](https://www.opensigner.dev)    | N/A                                            |

#### Self-Hosting for Custom L3 (Openfort only)

Openfort offers full self-hosting via [OpenSigner](https://www.opensigner.dev):

- **Key management**: Self-hostable wallet infrastructure
- **Contracts**: All open source, deploy yourself
- **Bundler/Paymaster**: Can self-host
- **No vendor lock-in**: Full ownership of infrastructure

This is unique to Openfort - Privy and ZeroDev require using their managed services.

---

## Recommendation

| Priority                                      | Choose                  |
| --------------------------------------------- | ----------------------- |
| **Simplest DX today**                         | Privy                   |
| **Self-hosting / open source**                | Openfort                |
| **Advanced session keys / chain abstraction** | ZeroDev + auth provider |
| **Maximum EVM chain support**                 | ZeroDev (130+ chains)   |

### For Your Use Case

Since you need **EOA identity preservation + gas sponsorship + open source preference**:

**Openfort is the strongest candidate** because:

1. ✅ Preserves EOA address (same as all providers)
2. ✅ Open source SDK and contracts
3. ✅ Self-hostable infrastructure
4. ⚠️ Needs confirmation: native paymaster + 7702 (theoretically works, undocumented)

**Privy is the fallback** if Openfort's native paymaster doesn't work with 7702:

1. ✅ Preserves EOA address
2. ✅ Simplest DX
3. ❌ Closed source, no self-hosting
4. Note: Uses ZeroDev's Kernel under the hood

---

## Open Questions

> **Note**: We want to use hosted infrastructure initially. Self-hosting is a nice-to-have for later, but for now we want to rely on your managed services.

### For Openfort

1. **Can we use your native paymaster with 7702-delegated EOAs?**
   - The `OPF7702` contract implements `IAccount` and works with EntryPoint
   - `pm_sponsorUserOperation` should work, but all examples use Pimlico instead
   - Can you confirm this works and provide an example?

2. **How does `ethereumProviderPolicyId` connect to 7702 authorization in the frontend SDK?**
   - Is the authorization automatically included when using the configured policy?

3. **Is a simpler DX planned?** (e.g., `sponsor: true` flag like Privy)

4. **Custom L3 support?**
   - Can you add our custom L3 to your hosted infrastructure?

### For Privy + ZeroDev

6. **Which SDK do we use on our custom L3?**
   - Privy has two integration paths with ZeroDev:
     - **Native integration**: Privy SDK with `sponsor: true` - ZeroDev under the hood, but only on Privy-supported chains
     - **Custom integration**: Privy for auth + ZeroDev SDK directly for gas sponsorship - works on ZeroDev-supported chains
   - If we pay ZeroDev to support our L3, do we use:
     - Privy's `sponsor: true` (does Privy automatically support chains ZeroDev supports?)
     - Or ZeroDev's SDK directly with Privy just as the signer/auth layer?

### General

8. **Is "pure" 7702 gas sponsorship possible without 4337?**
   - Or is 4337 infrastructure required for paymaster-based sponsorship?

---

## Documentation Links

### Privy

- [Gas Sponsorship Overview](https://docs.privy.io/wallets/gas-and-asset-management/gas/overview)
- [Gas Sponsorship Setup](https://docs.privy.io/wallets/gas-and-asset-management/gas/setup)

### Openfort

- [Native Paymaster](https://www.openfort.io/docs/products/infrastructure/paymaster/evm)
- [Paymaster Endpoints](https://www.openfort.io/docs/products/infrastructure/paymaster/evm/endpoints)
- [7702 Wallets](https://www.openfort.io/docs/products/embedded-wallet/javascript/wallets#eoas--erc-7702-upgrading-the-basic-wallet)
- [7702 Recipe (uses Pimlico)](https://www.openfort.io/docs/recipes/7702)
- [7702 Contracts](https://github.com/openfort-xyz/openfort-7702-account)
- [Supported Chains](https://www.openfort.io/docs/configuration/chains)
- [Entity Addresses (deployed contracts)](https://www.openfort.io/docs/configuration/addresses)
- [Self-Hosted (OpenSigner)](https://www.opensigner.dev)
- **Contact**: [Telegram @joalavedra](https://t.me/joalavedra)

### ZeroDev

- [7702 Examples](https://7702.zerodev.app/)
- [Kernel Contracts](https://github.com/zerodevapp/kernel)
- **Contact**: [Calendly](https://calendly.com/zerodev/30min)
