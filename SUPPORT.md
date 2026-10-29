# Support

There is no support contract here. This is an open-source project maintained by
one person in their own time, and that is worth saying plainly before anyone
builds a dependency on it.

## Where to ask

| I want to… | Go to |
| --- | --- |
| ask how to use it | open a [discussion-style issue](https://github.com/TeethakingBuilds/keygate-contracts/issues) in the repository that owns the code |
| report a bug | an issue, with the version and a reproduction |
| propose a feature | an issue first; a pull request is welcome once the shape is agreed |
| report a vulnerability | privately, see [SECURITY.md](./SECURITY.md) — never a public issue |

Questions asked in public are answered in public, which is the only way the
answers become useful to the next person.

## Which repository

- [keygate-contracts](https://github.com/TeethakingBuilds/keygate-contracts) — the
  Soroban contracts. Anything about on-chain behaviour, storage, or an error code.
- [keygate-server](https://github.com/TeethakingBuilds/keygate-server) — the
  authorisation service. Anything about the ledger mirror, staleness, `/v1/decide`.
- [keygate-client](https://github.com/TeethakingBuilds/keygate-client) — the
  TypeScript SDK. Anything about building, signing or submitting a transaction,
  or verifying a decision.

## What to expect

Issues get read. That is the honest commitment: a volunteer project that
promises four-hour response times is promising something it cannot deliver.
A bug report with a reproduction and a version number is far more likely to get
fixed than a description of a symptom, and a pull request that follows
[CONTRIBUTING.md](./CONTRIBUTING.md) — same checks, same style, one behaviour per
change — is very likely to be merged.

## If you need more than that

The code is Apache-2.0: run it yourself, fork it, change it. Nothing in this
repository requires permission or a paid tier, and there is no hosted service to
sell. If keygate is holding real value, the budget you have for support is better
spent on an independent audit of the contracts than on any answer given here —
see [SECURITY.md](./SECURITY.md).
