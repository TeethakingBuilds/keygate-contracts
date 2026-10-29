## What this changes

One behaviour. Describe the outcome, not the diff — the diff is already here.

## Does it move the wire format?

Check every box that applies, and say what you changed in the other repositories
in the same pull request (or link the pull request that does):

- [ ] No public function changed (name, arguments, or return type)
- [ ] No `#[contracterror]` variant added, removed, or renumbered → `CONTRACT_ERRORS` in [keygate-client](https://github.com/TeethakingBuilds/keygate-client/blob/main/src/errors.ts)
- [ ] No event emitted differently (name or argument order) → the decode table in [keygate-server](https://github.com/TeethakingBuilds/keygate-server/blob/main/src/ledger/decoder.ts)
- [ ] No `#[contracttype]` struct field added, removed, or reordered
- [ ] No `Effect` discriminant touched → `EFFECT_DISCRIMINANT` in keygate-client

A checked box with no matching change elsewhere is the failure mode this project
is most likely to hit: the contracts still work, and the SDK or the indexer quietly
stops describing them correctly.

## Tests

- [ ] There is a test that fails without this change
- [ ] The failure path is tested by error variant, not just "it errored"
- [ ] The boundary is tested (expiry exactly at `now`, budget exactly at its limit, threshold exactly met)

`make check` runs fmt, clippy with `-D warnings`, and the test suite — the same
three things CI does.

## Notes for the reviewer

Anything a reviewer should know that the diff does not show: a decision you made
between two workable designs and why, a case you deliberately did not handle, or
a place where you are about to disagree with the docs.
