# Contributing to keygate-contracts

## Setup

A current stable Rust toolchain and `rustup` are all you need. `rust-toolchain.toml`
pins `rustfmt`, `clippy` and `wasm32-unknown-unknown`, so the first build
provisions them:

```bash
cargo build --workspace     # host build; this is what tests run against
make check                  # fmt-check + clippy -D warnings + cargo test
make optimize               # release wasm, the artifact you would deploy
```

Tests use the Soroban SDK's own `Env`, so there is no node, network or fixture to
start. `cargo test -p keygate-core` is the usual loop when you are inside one crate.

`Cargo.lock` is committed and CI builds with `--locked`, because `soroban-env-host`
22.1.3 (which `soroban-sdk 22.0.11` pins exactly) declares `ed25519-dalek >=2.0.0`
with no upper bound: an unpinned resolve picks dalek 3.x and env-host then fails to
compile against `rand_chacha 0.3`. So a dependency change is not complete until the
lockfile matches it — commit the regenerated `Cargo.lock`, or run the
`Reset lockfile` workflow, which re-resolves, reapplies the dalek pin, proves the
workspace compiles, and only then commits.

## Where the code lives

One crate per contract, and each keeps the same shape:

```
keygate-<name>/src/lib.rs     storage types, the #[contractimpl], the errors
keygate-<name>/src/tests.rs   unit tests, one `Fixture` per file
```

There is deliberately no shared `keygate-common` crate. The four contracts publish
independent wasm, and a common crate would mean every redeploy carries whatever
else happened to be in it.

## How this repository relates to the other two

The contracts are the source of truth for three names that other repositories
copy. Change one and you change the others in the same pull request, or the
deployment stops matching its own SDK:

| In this repository | Mirrored in | As |
| --- | --- | --- |
| `#[contracterror]` codes | [keygate-client/src/errors.ts](https://github.com/TeethakingBuilds/keygate-client/blob/main/src/errors.ts) | `CONTRACT_ERRORS`, per contract role |
| event names and their argument order | [keygate-server/src/ledger/decoder.ts](https://github.com/TeethakingBuilds/keygate-server/blob/main/src/ledger/decoder.ts) | the decode table for the ledger mirror |
| function names and argument types | [keygate-client/src/calls.ts](https://github.com/TeethakingBuilds/keygate-client/blob/main/src/calls.ts) | one builder per public function |

`CONTRACT_ERRORS` is keyed by role because the same number means different things
in different contracts — code `3` is `Paused` in the registry and `NotConfigured`
in the breaker. A code that is missing from the client table still reaches the
caller, as `UnknownError(n)`, so forgetting it is a naming bug rather than a
silent one.

## Code style

`cargo fmt` decides, and CI refuses anything it would rewrite. If a branch is red
only for formatting, the `Fix formatting` workflow runs `cargo fmt` on it and
commits the result — use it rather than hand-wrapping calls. Beyond that:

- **Storage keys are an enum per contract** (`DataKey`), with variants named for
  what they hold rather than where they sit. A new variant is a schema change:
  existing deployments keep whatever was stored, so a key's meaning cannot be
  reassigned, only added to.
- **Every panic has to be an `Error` variant.** A `#[contracterror]` code is part
  of the ABI: it is what `keygate-client` names the failure with. Adding a
  `panic!` in a public path is a bug on its own.
- **Events are the public record.** The off-chain mirror reconstructs state from
  them, so a state change that does not emit is a state change no indexer can
  see. Emit when you write, and keep the argument order stable — it is positional
  in the decoder.
- **Auth at the top of the function.** `admin.require_auth()` before doing
  anything, including before reading, so a refusal costs no storage and no
  events.

## Tests

Sixty tests across the four crates, all against `Env`. The pattern in every one of
them:

```rust
let fixture = Fixture::new(now);        // deploy, initialise, set the clock
fixture.env.mock_all_auths();           // auth is the host's job, not ours
// … act, then assert on what the contract now says
```

Move the clock with `env.ledger().set_timestamp(…)` rather than waiting; advance
a ledger sequence with `env.ledger().set_sequence_number(…)`. Two things a new
test should cover, because they are the ones this project got wrong while
writing it: the failure path (which error code, not just "it failed") and the
boundary (an expiry exactly at `now`, a budget exactly at its limit).

## Pull requests

- One behaviour per pull request, with a test that fails without it.
- `make check` green before you push; CI runs the same commands.
- If you changed a public function, an error code, or an event, say so in the
  description and link the matching change in `keygate-client` or
  `keygate-server`.
- By contributing you confirm you may license the change under Apache-2.0, and
  this project follows the [Code of Conduct](./CODE_OF_CONDUCT.md).

## Adding a contract

A fifth crate is a real cost: it joins the workspace, gets its own error table in
the client, its own decode table in the server, and its own deployment. Most
things that look like a new contract are a new `resource` string in the existing
ones — that is the point of keeping `resource` and `action` free-form.

If it is genuinely a new contract: copy `keygate-policy` as the smallest crate,
keep the file layout, register it in the workspace `members` and in the `CONTRACTS`
list in [Makefile](./Makefile), and add it to the table in
[README.md](./README.md).
