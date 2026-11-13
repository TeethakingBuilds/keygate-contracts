# keygate-contracts

[![CI](https://github.com/TeethakingBuilds/keygate-contracts/actions/workflows/rust.yml/badge.svg)](https://github.com/TeethakingBuilds/keygate-contracts/actions/workflows/rust.yml)
[![license: Apache-2.0](https://img.shields.io/badge/license-Apache--2.0-blue)](./LICENSE)
[![status: unaudited 0.1](https://img.shields.io/badge/status-unaudited%200.1-orange)](./SECURITY.md)

Capability-based access control for [Soroban](https://soroban.stellar.org) smart contracts on the Stellar network.

Instead of every business contract hard-coding "is this the owner?", a keygate deployment answers one question: **may this address perform this action on this resource right now?**

## The model

A **capability** is a triple `(resource, action, holder)` carrying an expiry. `resource` and `action` are free-form strings chosen by the integrator (`"vault:main"` / `"withdraw"`, `"issuer:acme"` / `"mint"`), so the registry is not tied to any particular notion of a role.

The capability contracts compose into an authorisation stack:

```
caller ──▶ keygate-policy ──▶ keygate-core ──▶ keygate-circuit-breaker
            "do we permit       "does the        "has this resource
             this action at      holder still     exceeded its budget
             all right now?"      hold it?"        in this window?"
```

| crate | role |
| --- | --- |
| [`keygate-core`](./keygate-core) | Capability registry: issue, renew, revoke, check. Two-step admin handover, freeze switch. |
| [`keygate-policy`](./keygate-policy) | Allow/deny rules per `(resource, action)` in front of a registry. Deny takes an action out of service without touching hundreds of grants. |
| [`keygate-circuit-breaker`](./keygate-circuit-breaker) | Fixed-window budgets that **latch closed** when breached, and stay closed until an explicit reset. |
| [`keygate-multisig`](./keygate-multisig) | m-of-n administration. Relays arbitrary calls (`target`, `func`, encoded `args`) so the registry's admin key is never held by one person. Owner rotation invalidates every open proposal, so removed owners' approvals cannot survive it. |
| [`keygate-payments`](./keygate-payments) | Non-custodial USDC SAC payments and Circle CCTP V2 burns with daily/30-day limits, recipient and route allowlists, threshold co-signers, pause, deadlines and replay protection. |

## Why the layers are separate

Keeping policy apart from the registry answers two different questions. The registry asks *does this holder have the capability*; policy asks *do we want that capability to be usable at all right now*. An incident response is one `Deny` rule rather than a bulk revocation. `remove_rule` removes that override and evaluation uses the **current** default: `Deny` denies access, while `Allow` still requires a live capability from the registry. An `Allow` rule overwritten by the `Deny` is not restored.

The breaker trips instead of merely rejecting. A `require(amount <= limit)` line stops one withdrawal; a latched breaker stops the draining that follows.

## Getting started

```bash
cargo build --workspace          # host build, for tests
make optimize                    # release wasm for deployment
make check                       # fmt + clippy -D warnings + test
```

Requires a current stable Rust toolchain; `rust-toolchain.toml` pins `rustfmt`, `clippy` and the `wasm32-unknown-unknown` target, so `rustup` provisions everything on first build. `Cargo.lock` is committed and CI builds `--locked` — the SDK's own dependency ranges do not resolve to a compilable graph on their own, so the lockfile is part of the build, not a nicety.

### Deploying

```bash
cargo build --release --target wasm32-unknown-unknown
# then, with the Soroban CLI:
soroban contract deploy --wasm target/wasm32-unknown-unknown/release/keygate_core.wasm
soroban contract invoke --id <core> -- initialize --admin <GADDRESS>
soroban contract invoke --id <core> -- issue \
  --resource "vault:main" --action "withdraw" --holder <GADDRESS> --ttl 3600
soroban contract invoke --id <core> -- check \
  --resource "vault:main" --action "withdraw" --holder <GADDRESS>
# true
```

## Composition inside another contract

`require_access` is the entry point other contracts should call — it authenticates the caller and fails closed:

```rust
let core = KeygateCoreClient::new(env, &core_address);
core.require_access(&caller, &resource, &action);

let breaker = KeygateBreakerClient::new(env, &breaker_address);
if breaker.enforce(&resource, &amount) == Outcome::Breached {
    // Skip the transfer, do not fail the call. A caller that returns an error
    // here reverts the breaker's own frame, and the latch goes with it.
    return Ok(());
}
```

Admin operations on keygate contracts should be routed through `keygate-multisig` and authorised by the multisig's own contract address rather than by a single user key.

## USDC payments and CCTP

`keygate-payments` is constructor-bound to Circle's official network-specific USDC SAC and CCTP V2 TokenMessengerMinter addresses. It never holds funds: the payer signs a direct SAC transfer or a CCTP burn. Stellar-origin burns support only Circle's verified Ethereum (0), Solana (5), and Base (6) domains, require Standard finality, and reject amounts with a non-zero seventh decimal rather than letting CCTP silently truncate it.

A classic `G…` account can always transfer USDC outside keygate. The policy governs calls routed through this contract; complete account-level enforcement requires a smart-contract account that delegates authorization to it.

## Testnet payment interface

The web interface has dedicated Home (`index.html`), Send (`send.html`), Receive/activity (`receive.html`), Recovery (`recovery.html`), and Developers (`developers.html`) pages. It uses a locally bundled Freighter SDK, shared navigation, native page transitions, keyboard-operable route controls, and reduced-motion-aware animations. Node 22.12+ and Corepack are required for the frontend build; the local serving command uses Python:

```bash
corepack pnpm install --frozen-lockfile --ignore-scripts
corepack pnpm run build:web
corepack pnpm run test:web
corepack pnpm audit --prod
corepack pnpm run serve:web
```

Open `http://127.0.0.1:4173/index.html` in a normal browser with Freighter, and select Stellar Testnet in the wallet. Use test assets only. `build-pages.js` generates the five HTML pages and `web.css` from `site-template.html`; edit that template or the page-specific layouts in the builder, then run `build:web`. Motion lives in `motion.css` and `site.js`; wallet behavior lives in `index.js` and `wallet.js`. Rebuild `wallet.bundle.js` whenever `wallet.js`, `web-core.js`, or wallet dependencies change; there is no runtime CDN import.

- Connection requests public-address access, reads the official Testnet USDC SAC balance, and blocks signing on other network passphrases.
- The default policy path is unavailable until a verified Keygate contract deployment is configured. The separate **Direct USDC SAC — outside Keygate policy** option constructs and simulates an unsigned Testnet transfer. It does not enforce Keygate limits, allowlists, or quorum rules.
- An explicit review and checkbox precede Freighter signing. The returned XDR must match the reviewed transaction and contain a valid payer signature. Transactions expire after three minutes; signing does not guarantee ledger success.
- Activity stores account/hash references only. Status is queried from Stellar RPC; missing transactions, transport failures, and timeouts remain unknown, not failed or settled. Do not resend blindly.
- Ethereum, Solana, and Base destination selection includes address validation and optional injected-wallet recipient autofill. Circle Standard fees and source-burn messages are queried from Iris Sandbox. Attestation availability and Circle-reported forwarding do not prove destination settlement.
- **CCTP burn submission, destination claims, policy-routed signing, and Mainnet execution are not enabled.** Additional account authorization is rejected rather than bypassed.

Frontend unit tests cover amount precision, asset binding, address validation, transaction construction, signing integrity, and status semantics. Browser checks cover responsive controls and provider error states; a real Freighter-signed end-to-end transfer has not yet been verified. There is no fabricated balance, transfer, or successful bridge state.

## Status and scope

Version 0.1.0. Unit tests are present for all five crates against the SDK's own `Env`; the new payment crate still requires Rust/CI verification. **This code is unaudited** — read [SECURITY.md](./SECURITY.md) before holding real value with it. Deliberate limitations:

- Capability expiry is reclaimed lazily, on the first `check` of an expired grant. There is no background sweep, so a grant that is never queried stays in storage.
- `keygate-policy` trusts the registry address it was configured with. Repointing it is an admin action and is emitted as an event for indexers.
- A breaker breach is a **value** (`Outcome::Breached`), not an error, because a call that errors loses its writes and would lose the latch with them. The caller must honour that value. `enforce` is also public, so an untrusted caller can consume a known resource's budget; do not use this generic breaker as the authoritative payment limit. `keygate-payments` consumes its own limits atomically with the transfer or burn.
- Payment limits use UTC daily and fixed 30-day buckets, not sliding windows. The boundary behavior is explicit and tested.
- Authorisation itself (`require_auth`) is enforced by the Soroban host. Unit tests mock all auths, so the cryptography of a signature is the host's to prove, not ours. What the tests do pin down is the authority side: every entry point that calls `require_auth` has a test asserting, through `env.auths()`, which address had to sign and for which function and arguments.

## Documentation

- [CONTRIBUTING.md](./CONTRIBUTING.md) — development setup, code style, how to add a crate
- [SECURITY.md](./SECURITY.md) — vulnerability reporting and threat model
- [SUPPORT.md](./SUPPORT.md) — where to ask questions
- [CODE_OF_CONDUCT.md](./CODE_OF_CONDUCT.md)

## License

Apache-2.0. See [LICENSE](./LICENSE).
