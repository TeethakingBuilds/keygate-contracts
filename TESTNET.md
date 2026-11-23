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

- Rust with the `wasm32-unknown-unknown` target (the repo pins its toolchain
  in `rust-toolchain.toml`):
  ```sh
  rustup target add wasm32-unknown-unknown
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
| keygate-core | `target/wasm32-unknown-unknown/release/keygate_core.wasm` |
| keygate-policy | `target/wasm32-unknown-unknown/release/keygate_policy.wasm` |
| keygate-circuit-breaker | `target/wasm32-unknown-unknown/release/keygate_circuit_breaker.wasm` |
| keygate-multisig | `target/wasm32-unknown-unknown/release/keygate_multisig.wasm` |
| keygate-payments | `target/wasm32-unknown-unknown/release/keygate_payments.wasm` |

## 2. Create and fund the deployer identity

```sh
stellar keys generate --global keygate-deployer --network testnet
stellar keys fund keygate-deployer --network testnet
stellar keys address keygate-deployer
```

Record the address — it is the bootstrap address for the multisig and the
admin for the contracts that are not administered by the multisig. The secret
key lives in the CLI's key store (`~/.config/stellar/identity/`); never commit
it, never paste it anywhere, and never reuse this identity on mainnet.

For the multisig you need two more owner identities (threshold 2 of 3):

```sh
stellar keys generate --global keygate-owner-b --network testnet
stellar keys generate --global keygate-owner-c --network testnet
stellar keys fund keygate-owner-b --network testnet
stellar keys fund keygate-owner-c --network testnet
```

## 3. Deploy the multisig first

The multisig is deployed first so `keygate-core` can be administered by it
from the start — that is the setup the multisig's end-to-end test covers
(issue #3). Its constructor takes the bootstrap address, which is the
deployer key; the owner set is recorded afterwards with `initialize`.

```sh
DEPLOYER=$(stellar keys address keygate-deployer)

MULTISIG=$(stellar contract deploy \
  --wasm target/wasm32-unknown-unknown/release/keygate_multisig.wasm \
  --source keygate-deployer \
  --network testnet \
  -- --bootstrap "$DEPLOYER")
echo "multisig: $MULTISIG"
```

Initialise it with the three owners and threshold 2:

```sh
OWNER_B=$(stellar keys address keygate-owner-b)
OWNER_C=$(stellar keys address keygate-owner-c)

stellar contract invoke \
  --id "$MULTISIG" \
  --source keygate-deployer \
  --network testnet \
  -- initialize \
  --owners "[\"$DEPLOYER\", \"$OWNER_B\", \"$OWNER_C\"]" \
  --threshold 2
```

## 4. Deploy and initialise keygate-core

`keygate-core` is administered by the multisig, not by a single key:

```sh
CORE=$(stellar contract deploy \
  --wasm target/wasm32-unknown-unknown/release/keygate_core.wasm \
  --source keygate-deployer \
  --network testnet)
echo "core: $CORE"

stellar contract invoke \
  --id "$CORE" \
  --source keygate-deployer \
  --network testnet \
  -- initialize \
  --admin "$MULTISIG"
```

## 5. Deploy and initialise keygate-policy

The policy points at the core registry deployed in step 4:

```sh
POLICY=$(stellar contract deploy \
  --wasm target/wasm32-unknown-unknown/release/keygate_policy.wasm \
  --source keygate-deployer \
  --network testnet)
echo "policy: $POLICY"

stellar contract invoke \
  --id "$POLICY" \
  --source keygate-deployer \
  --network testnet \
  -- initialize \
  --admin "$DEPLOYER" \
  --registry "$CORE"
```

## 6. Deploy and initialise the circuit breaker

```sh
BREAKER=$(stellar contract deploy \
  --wasm target/wasm32-unknown-unknown/release/keygate_circuit_breaker.wasm \
  --source keygate-deployer \
  --network testnet)
echo "breaker: $BREAKER"

stellar contract invoke \
  --id "$BREAKER" \
  --source keygate-deployer \
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
  --wasm target/wasm32-unknown-unknown/release/keygate_payments.wasm \
  --source keygate-deployer \
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
stellar contract invoke --id "$CORE" --network testnet -- admin
stellar contract invoke --id "$POLICY" --network testnet -- registry
stellar contract invoke --id "$BREAKER" --network testnet -- admin
stellar contract invoke --id "$MULTISIG" --network testnet -- owners
stellar contract invoke --id "$MULTISIG" --network testnet -- threshold
stellar contract invoke --id "$PAYMENTS" --network testnet -- network
stellar contract invoke --id "$PAYMENTS" --network testnet -- token
```

Then prove the multisig actually administers core end to end: propose a
`pause` on core, approve it with the second owner, execute it, confirm core
is paused, and unpause it the same way so the deployment is left usable.
A proposal targets a contract by its ID, names the function, and carries the
arguments as a JSON array of encoded values:

```sh
# Submit the pause proposal as owner A (the deployer). It auto-approves for A.
PAUSE_ID=$(stellar contract invoke \
  --id "$MULTISIG" --source keygate-deployer --network testnet \
  -- submit \
  --caller "$DEPLOYER" \
  --target "$CORE" \
  --func pause \
  --args '[]')

# Second owner approves; quorum (2 of 3) is now met.
stellar contract invoke \
  --id "$MULTISIG" --source keygate-owner-b --network testnet \
  -- approve --caller "$OWNER_B" --id "$PAUSE_ID"

# Execute the proposal; core pauses.
stellar contract invoke \
  --id "$MULTISIG" --source keygate-deployer --network testnet \
  -- execute --caller "$DEPLOYER" --id "$PAUSE_ID"

stellar contract invoke --id "$CORE" --network testnet -- paused

# Leave the deployment usable: unpause through the same flow.
UNPAUSE_ID=$(stellar contract invoke \
  --id "$MULTISIG" --source keygate-deployer --network testnet \
  -- submit \
  --caller "$DEPLOYER" \
  --target "$CORE" \
  --func unpause \
  --args '[]')
stellar contract invoke \
  --id "$MULTISIG" --source keygate-owner-b --network testnet \
  -- approve --caller "$OWNER_B" --id "$UNPAUSE_ID"
stellar contract invoke \
  --id "$MULTISIG" --source keygate-deployer --network testnet \
  -- execute --caller "$DEPLOYER" --id "$UNPAUSE_ID"

stellar contract invoke --id "$CORE" --network testnet -- paused
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

## 10. Redeploying and rotating

- **New code:** rebuild (`make optimize`), deploy fresh contracts, and
  follow the steps above again. There is no in-place upgrade — see the
  upgrade guide issue (#15) for the migration story.
- **New admin:** the admin of core is the multisig; rotate the owner set
  with a quorum proposal (`set_owners` targeting the multisig itself), not
  by re-deploying.
- **New deployer key:** generate and fund a new identity as in step 2. The
  old identity's secret is not needed to keep the deployed contracts alive.
