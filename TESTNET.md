# Deploying Keygate to Stellar Testnet

This is the runbook for deploying all five contracts to Stellar Testnet and
verifying each one answers. It is written to be followed top to bottom by a
maintainer with no prior deployment: every command is copyable, and every
value that has to be recorded is marked. Nothing here touches mainnet, and no
secret is ever needed — the deployer identity is a fresh testnet key funded
for free by Friendbot.

Testnet state is disposable. Re-deploying creates new contract IDs; when you
do, update the table in [README.md](./README.md#testnet-deployments) and
note the date.

## Prerequisites

- Rust with the `wasm32v1-none` target (the repo pins its toolchain
  in `rust-toolchain.toml`):
  ```sh
  rustup target add wasm32v1-none
  ```
- The Stellar CLI (install it **outside** this repository directory, or with
  `cargo +stable install` — the repo's `rust-toolchain.toml` confuses a plain
  `cargo install` run from inside the repo):
  ```sh
  cargo install --locked stellar-cli
  stellar --version
  ```

## 1. Build the release wasm

```sh
make optimize
```

This builds all five contracts. The artifacts are:

| Contract | Wasm file |
| --- | --- |
| keygate-core | `target/wasm32v1-none/release/keygate_core.wasm` |
| keygate-policy | `target/wasm32v1-none/release/keygate_policy.wasm` |
| keygate-circuit-breaker | `target/wasm32v1-none/release/keygate_circuit_breaker.wasm` |
| keygate-multisig | `target/wasm32v1-none/release/keygate_multisig.wasm` |
| keygate-payments | `target/wasm32v1-none/release/keygate_payments.wasm` |

## 2. Create and fund the deployer identity

```sh
stellar keys generate keygate-deployer --fund
stellar keys address keygate-deployer
```

Record the address — it is the bootstrap address for the multisig and the
admin for the contracts that are not administered by the multisig. The secret
key lives in the CLI's key store (`~/.config/stellar/identity/`); never commit
it, never paste it anywhere, and never reuse this identity on mainnet.

If the CLI's `--fund` cannot reach Friendbot, fund the address directly:

```sh
curl "https://friendbot.stellar.org/?addr=<address>"
```

For the multisig you need two more owner identities (threshold 2 of 3):

```sh
stellar keys generate keygate-owner-b --fund
stellar keys generate keygate-owner-c --fund
```

## 3. Deploy the multisig first

The multisig is deployed first so `keygate-core` can be administered by it
from the start — that is the setup the multisig's end-to-end test covers
(issue #3). Its constructor takes the bootstrap address, which is the
deployer key; the owner set is recorded afterwards with `initialize`.

```sh
DEPLOYER=$(stellar keys address keygate-deployer)

MULTISIG=$(stellar contract deploy \
  --wasm target/wasm32v1-none/release/keygate_multisig.wasm \
  --source-account keygate-deployer \
  --network testnet \
  -- --bootstrap "$DEPLOYER")
echo "multisig: $MULTISIG"
```

Initialise it with the three owners and threshold 2:

```sh
OWNER_B=$(stellar keys address keygate-owner-b)
OWNER_C=$(stellar keys address keygate-owner-c)

stellar contract invoke \
  --contract-id "$MULTISIG" \
  --source-account keygate-deployer \
  --network testnet \
  -- initialize \
  --owners "[\"$DEPLOYER\", \"$OWNER_B\", \"$OWNER_C\"]" \
  --threshold 2
```

## 4. Deploy keygate-core — and initialise it through the multisig

`keygate-core` is administered by the multisig, not by a single key. Its
`initialize` calls `admin.require_auth()`, and a contract can only authorise
a call it initiated — so the deployer cannot initialise core directly. The
multisig must do it, by quorum proposal: submit the proposal as owner A,
approve it as owner B, execute it as owner A. (The same three calls are the
general pattern for every admin action on core; step 8 shows it again for
`pause`.)

```sh
CORE=$(stellar contract deploy \
  --wasm target/wasm32v1-none/release/keygate_core.wasm \
  --source-account keygate-deployer \
  --network testnet)
echo "core: $CORE"

INIT_ID=$(stellar contract invoke \
  --contract-id "$MULTISIG" \
  --source-account keygate-deployer \
  --network testnet \
  -- submit \
  --caller "$DEPLOYER" \
  --target "$CORE" \
  --func initialize \
  --args "[\"$MULTISIG\"]")

stellar contract invoke \
  --contract-id "$MULTISIG" \
  --source-account keygate-owner-b \
  --network testnet \
  -- approve --caller "$OWNER_B" --id "$INIT_ID"

stellar contract invoke \
  --contract-id "$MULTISIG" \
  --source-account keygate-deployer \
  --network testnet \
  -- execute --caller "$DEPLOYER" --id "$INIT_ID"
```

## 5. Deploy and initialise keygate-policy

The policy points at the core registry deployed in step 4:

```sh
POLICY=$(stellar contract deploy \
  --wasm target/wasm32v1-none/release/keygate_policy.wasm \
  --source-account keygate-deployer \
  --network testnet)
echo "policy: $POLICY"

stellar contract invoke \
  --contract-id "$POLICY" \
  --source-account keygate-deployer \
  --network testnet \
  -- initialize \
  --admin "$DEPLOYER" \
  --registry "$CORE"
```

## 6. Deploy and initialise the circuit breaker

```sh
BREAKER=$(stellar contract deploy \
  --wasm target/wasm32v1-none/release/keygate_circuit_breaker.wasm \
  --source-account keygate-deployer \
  --network testnet)
echo "breaker: $BREAKER"

stellar contract invoke \
  --contract-id "$BREAKER" \
  --source-account keygate-deployer \
  --network testnet \
  -- initialize \
  --admin "$DEPLOYER"
```

## 7. Deploy keygate-payments

Payments has no `initialize`: its deployment is bound at construction to
the official network-specific USDC SAC and CCTP v2 TokenMessengerMinter.
The constructor rejects anything else, so the addresses below are not a
choice — they are the values the contract itself enforces
(`keygate-payments/src/lib.rs`, `TESTNET_USDC` / `TESTNET_CCTP`).

```sh
PAYMENTS=$(stellar contract deploy \
  --wasm target/wasm32v1-none/release/keygate_payments.wasm \
  --source-account keygate-deployer \
  --network testnet \
  -- --admin "$DEPLOYER" \
     --network Testnet \
     --token CBIELTK6YBZJU5UP2WWQEUCYKLPU6AUNZ2BQ4WWFEIE3USCIHMXQDAMA \
     --cctp CDNG7HXAPBWICI2E3AUBP3YZWZELJLYSB6F5CC7WLDTLTHVM74SLRTHP)
echo "payments: $PAYMENTS"
```

## 8. Verify every deployment

Read-only calls need no `--source`. Each one should answer without error:

```sh
stellar contract invoke --contract-id "$CORE" --network testnet -- admin
stellar contract invoke --contract-id "$POLICY" --network testnet -- registry
stellar contract invoke --contract-id "$BREAKER" --network testnet -- admin
stellar contract invoke --contract-id "$MULTISIG" --network testnet -- owners
stellar contract invoke --contract-id "$MULTISIG" --network testnet -- threshold
stellar contract invoke --contract-id "$PAYMENTS" --network testnet -- network
stellar contract invoke --contract-id "$PAYMENTS" --network testnet -- token
```

Then prove the multisig actually administers core end to end: propose a
`pause` on core, approve it with the second owner, execute it, confirm core
is paused, and unpause it the same way so the deployment is left usable.
A proposal targets a contract by its ID, names the function, and carries the
arguments as a JSON array of encoded values:

```sh
# Submit the pause proposal as owner A (the deployer). It auto-approves for A.
PAUSE_ID=$(stellar contract invoke \
  --contract-id "$MULTISIG" --source-account keygate-deployer --network testnet \
  -- submit \
  --caller "$DEPLOYER" \
  --target "$CORE" \
  --func pause \
  --args '[]')

# Second owner approves; quorum (2 of 3) is now met.
stellar contract invoke \
  --contract-id "$MULTISIG" --source-account keygate-owner-b --network testnet \
  -- approve --caller "$OWNER_B" --id "$PAUSE_ID"

# Execute the proposal; core pauses.
stellar contract invoke \
  --contract-id "$MULTISIG" --source-account keygate-deployer --network testnet \
  -- execute --caller "$DEPLOYER" --id "$PAUSE_ID"

stellar contract invoke --contract-id "$CORE" --network testnet -- paused

# Leave the deployment usable: unpause through the same flow.
UNPAUSE_ID=$(stellar contract invoke \
  --contract-id "$MULTISIG" --source-account keygate-deployer --network testnet \
  -- submit \
  --caller "$DEPLOYER" \
  --target "$CORE" \
  --func unpause \
  --args '[]')
stellar contract invoke \
  --contract-id "$MULTISIG" --source-account keygate-owner-b --network testnet \
  -- approve --caller "$OWNER_B" --id "$UNPAUSE_ID"
stellar contract invoke \
  --contract-id "$MULTISIG" --source-account keygate-deployer --network testnet \
  -- execute --caller "$DEPLOYER" --id "$UNPAUSE_ID"

stellar contract invoke --contract-id "$CORE" --network testnet -- paused
```

(The paused state is read back with `paused`, the read-only accessor on
`keygate-core`.)

## 9. Record the deployment

Update the "Testnet deployments" table in
[README.md](./README.md#testnet-deployments) with the five contract IDs and
the date, in a commit of its own so the deployment is reviewable:

```sh
git add README.md
git commit -m "docs: record testnet deployment <YYYY-MM-DD>"
```

### Deployment history

- **v0.1.0 — 2026-11-25 (current).** Tagged release of `main`
  (`aeabfbf`), deployed as the verified contract set for issue #18.
  Core `CBZRZMZQASGW552RBNDG4YUEM7EHMWFSNOZTM3XYDF2DVFLC3IYJLXP5`,
  policy `CDQ6LJRVWGGFJLBN4VYIWTT56W267O5C4RVIRIUWRRB6SD6HD2IOIHAH`,
  breaker `CCTZ5EUWWROUQIYHVRHGQKU4SA5LTZZFDI46NWH3R7CRP6XGQJKULQYE`,
  multisig `CBH2X55EWCPXSALTSF54TKN2FPMDM46J7ITIHKYXME5GRXNVS6HYURXL`,
  payments `CDYM4UQCPM64PVXBK7AWQWZEQPTCK6VHHYZBHCS5ELRTVVNP23QBQDNG`.
  Verified live: core initialised through a multisig quorum proposal,
  pause/unpause proposals exercised, and the payments policy machinery
  (spending policy + allowlist) exercised. A live policy-routed `pay`
  still needs the deployer funded with testnet USDC (Circle faucet);
  until then the pay is proven only up to the token transfer by
  simulation.
- **Ad-hoc — 2026-11-23 (superseded).** Wave-readiness deployment, not a
  tagged release. Core
  `CBRDRP6TZROQVHSXCWFLRTJWISAME7EWG2KJLXCK7Z4LVXTJIMETEJJN`, policy
  `CDDFXOXJCGJSQYY4HWSV2FQLNUP6HDIWQDIAX3CWWCOPUWMGLLGEK4KI`, breaker
  `CBNXE2446LQR6AZUQGFPFYEUV6FZIGB5WWYUKWVDDOMNBSF2HJ7WLQK7`,
  multisig `CCDPBWVHDJNJHXBUHTV6GXS7YFTF3ZFWMY366PIS6GBAXEY5XFW7DLPJ`,
  payments `CC2NJPL3ZFCJ3XYGE3YC4M5PSNUBAII5ZTZELGGMWNOGV65X6KRPKTXY`.
  Superseded by v0.1.0; do not point new integrations at these IDs.

## 10. Redeploying and rotating

- **New code:** rebuild (`make optimize`), deploy fresh contracts, and
  follow the steps above again. There is no in-place upgrade — see the
  upgrade guide issue (#15) for the migration story.
- **New admin:** the admin of core is the multisig; rotate the owner set
  with a quorum proposal (`set_owners` targeting the multisig itself), not
  by re-deploying.
- **New deployer key:** generate and fund a new identity as in step 2. The
  old identity's secret is not needed to keep the deployed contracts alive.

## Troubleshooting

- **`reference-types not enabled` on upload.** The release wasm must be
  built for `wasm32v1-none` (`make optimize` does this). A
  `wasm32-unknown-unknown` build from a current stable Rust enables
  reference types, and the Soroban VM rejects the module at upload time
  with `HostError: Error(WasmVm, InvalidAction)`. This repo used to build
  for `wasm32-unknown-unknown`; it was switched when the first testnet
  deploy hit exactly this error.
- **`client error (Connect)` from the Stellar CLI.** The CLI's HTTP client
  does not honour the `HTTPS_PROXY` / `https_proxy` environment variables.
  On a host whose only egress is a proxy, the CLI cannot reach the RPC
  even though `curl` can. Workarounds: run the CLI where it has direct
  egress, or drive the deployment with `@stellar/stellar-sdk` under Node
  with `NODE_USE_ENV_PROXY=1` (that is how the 2026-11-23 deployment was
  actually performed — the deploy scripts are kept outside the repo).
- **Payments constructor rejects the `network` argument.** A
  `#[contracttype]` unit-variant enum encodes as a one-element vec holding
  the variant symbol, not a bare symbol. The CLI accepts the variant name
  (`--network Testnet`); with the JS SDK the argument is
  `ScVal.scvVec([ScVal.scvSymbol('Testnet')])`. A bare symbol fails the
  create with `HostError: Error(Context, InvalidAction)`.
- **Deploy fails on `--optimize`.** `stellar contract deploy` optimises the
  wasm by default, which needs the CLI's `additional-libs` feature. If your
  CLI build lacks it, pass `--optimize=false` — `make optimize` already
  produced a release build.
