# Security policy

## Reporting a vulnerability

Use **Security → Report a vulnerability** on this repository. The advisory is
private: it is visible only to the reporter and the maintainer. Do not open a
public issue, and do not file a pull request that shows the exploit.

Expect an acknowledgement within a week. If a fix is possible, one ships before
the advisory becomes public, and the release notes credit the reporter unless
they ask not to be.

## Supported versions

| Version | Supported |
| --- | --- |
| 0.1.x | yes |
| anything older | no — deploy the current release |

keygate is versioned 0.x. The API is stable enough to build against; the
security posture is not yet settled enough to trust without reading this page.

## Before you put real value behind it

**This code is unaudited.** It is a careful implementation of a model its author
understands, tested against the cases that occurred to them, and nothing more. An
audit is a purchase, not a property of this repository.

Specifically, these are the ways keygate can be made to do something unexpected,
in rough order of how much they have shaped the design:

1. **The admin key is the whole system.** `issue`, `revoke`, `set_rule`,
   `configure` and `reset` are admin-only, and an admin can grant anything to
   anyone. `keygate-multisig` exists so that key is held by a threshold of people
   instead of one person — an admin that is not a multisig contract address is a
   single point of failure, and this library cannot detect it for you.
2. **`keygate-multisig` relays arbitrary calls.** A proposal carries a target
   contract, a function name and encoded arguments. Quorum approval is therefore
   approval of whatever that call does; the arguments are not human-readable and
   keygate cannot make them so. Approvers should read the proposal's encoded args
   with `keygate-client` before signing.
3. **`keygate-policy` trusts the registry it points at.** Repointing the registry
   changes which grants the policy evaluates. That is an admin action and it
   emits an event, so an indexer can watch for it, but the contracts themselves
   do not refuse a repoint.
4. **A breaker is a budget, not a balance.** `enforce` charges against a rolling
   window it maintains itself. It does not know the asset's real supply, so a
   resource that never calls `enforce` is unlimited.
5. **A breach is reported, not enforced.** A Soroban call that returns an error
   has its ledger writes discarded, and the latch is a write — so `enforce`
   answers `Outcome::Breached` as a value and keeps the resource closed. The
   price of that is that the *caller* has to honour the answer: a guarded call
   which ignores it moves value once past the limit, and only the next attempt
   meets `Error::Tripped`. Nothing in this repository can close that window;
   the composition example in README.md is the contract.
6. **Expiry is lazy.** An expired capability stops answering `true` immediately,
   but it stays in storage until something touches it. This affects contract size
   and `live_ids` cost over a long deployment, not whether a check is correct.
7. **Auth is the host's job.** `require_auth` is enforced by Soroban, and the unit
   tests mock all auths. They prove state transitions are right; they do not prove
   that a caller who cannot sign is stopped, because in the tests nobody ever
   fails to sign.

Cross-contract invariants are the integrator's responsibility. keygate answers
"may this holder do this action"; it does not know whether the caller then
actually charged the breaker for the amount it moved. The composition order in
[README.md](./README.md#composition-inside-another-contract) is the recommended
one: `require_access` first, then `enforce`, then the value transfer — and the
transfer only if `enforce` did not answer `Breached`.

## What is in scope

- Reading or writing another account's capability through these contracts.
- Bypassing a policy rule, a paused registry, or a tripped breaker.
- A `require_access` returning success for a holder with no live grant.
- Storage growth or an arithmetic path that makes a check disagree with itself.
- Anything that makes a signed decision from `keygate-server` verify for a
  different request than the one it was issued for.

## What is out of scope

- Costs you bear from using it (a ledger that grows, a call that fails closed and
  blocks your users). Those are documented limitations, not vulnerabilities.
- Attacks on the RPC, ledger or Stellar network itself.
- A deployment that ignored this page: an admin key held by one person, a policy
  pointed at a registry it does not control, or `enforce` skipped.
- Versions older than the table above.

## Threat model in one paragraph

keygate is a decision-maker, not a vault. Its state is the authority on who may do
what, so the two things worth protecting are the admin set and the integrity of
`check`/`require_access`. Every grant, rule, budget and pause is emitted as an
on-chain event, which means the off-chain mirror in `keygate-server` can be
reconstructed by anyone who reads the ledger — the mirror is not a secret, and
neither is the model that an attacker who can read the chain can reason about.
What an attacker cannot do without the admin set is mint a capability.
