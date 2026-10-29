//! Unit tests for keygate-multisig. Run with `cargo test -p keygate-multisig`.
//!
//! A throwaway `Recorder` contract stands in for the guarded target so the
//! relayed call and its side effect are both observable.

use soroban_sdk::{
    contract, contractimpl, contracttype, symbol_short, testutils::Address as _,
    testutils::Ledger as _, Address, Env, IntoVal, String, Symbol, Val, Vec,
};

use super::{Error, KeygateMultisig, KeygateMultisigClient};

#[contracttype]
#[derive(Clone)]
enum RecKey {
    Last,
    Count,
}

/// Minimal target: stores the last value it was called with.
#[contract]
pub struct Recorder;

#[contractimpl]
impl Recorder {
    pub fn record(env: Env, value: i128) {
        let count: u32 = env.storage().persistent().get(&RecKey::Count).unwrap_or(0);
        env.storage().persistent().set(&RecKey::Last, &value);
        env.storage().persistent().set(&RecKey::Count, &(count + 1));
    }

    pub fn last(env: Env) -> Option<i128> {
        env.storage().persistent().get(&RecKey::Last)
    }

    pub fn count(env: Env) -> u32 {
        let count: Option<u32> = env.storage().persistent().get(&RecKey::Count);
        count.unwrap_or(0)
    }
}

/// Minimal target for a `Deny`-style call with no arguments.
#[contract]
pub struct Named;

#[contractimpl]
impl Named {
    pub fn tag(env: Env, label: String) {
        env.storage()
            .persistent()
            .set(&symbol_short!("label"), &label);
    }
}

struct Fixture {
    env: Env,
    multisig: Address,
    recorder: Address,
    owners: Vec<Address>,
    a: Address,
    b: Address,
    c: Address,
    outsider: Address,
}

impl Fixture {
    /// Deploys an n-of-3 multisig against a fresh `Recorder`.
    fn new(threshold: u32) -> Fixture {
        let env = Env::default();
        env.ledger().set_timestamp(1_000);
        let recorder = env.register(Recorder, ());
        let multisig = env.register(KeygateMultisig, ());
        let a = Address::generate(&env);
        let b = Address::generate(&env);
        let c = Address::generate(&env);
        let outsider = Address::generate(&env);
        let mut owners = Vec::new(&env);
        owners.push_back(a.clone());
        owners.push_back(b.clone());
        owners.push_back(c.clone());
        env.mock_all_auths();
        KeygateMultisigClient::new(&env, &multisig).initialize(&owners, &threshold);
        Fixture {
            env,
            multisig,
            recorder,
            owners,
            a,
            b,
            c,
            outsider,
        }
    }

    fn client(&self) -> KeygateMultisigClient<'_> {
        KeygateMultisigClient::new(&self.env, &self.multisig)
    }

    fn recorder(&self) -> RecorderClient<'_> {
        RecorderClient::new(&self.env, &self.recorder)
    }

    /// A proposal calling `record(42)`.
    fn record_args(&self, value: i128) -> Vec<Val> {
        let mut args = Vec::new(&self.env);
        args.push_back(value.into_val(&self.env));
        args
    }

    fn submit_record(&self, caller: &Address, value: i128) -> u64 {
        self.client().submit(
            caller,
            &self.recorder,
            &symbol_short!("record"),
            &self.record_args(value),
        )
    }
}

#[test]
fn initialize_stores_owners_and_threshold() {
    let f = Fixture::new(2);
    assert_eq!(f.client().threshold(), 2);
    assert_eq!(f.client().owners(), f.owners);
    assert!(f.client().is_owner(&f.a));
    assert!(!f.client().is_owner(&f.outsider));
}

#[test]
fn initialize_rejects_bad_owner_sets() {
    let env = Env::default();
    let multisig = env.register(KeygateMultisig, ());
    let client = KeygateMultisigClient::new(&env, &multisig);
    env.mock_all_auths();

    let empty: Vec<Address> = Vec::new(&env);
    assert_eq!(
        client.try_initialize(&empty, &1),
        Err(Ok(Error::EmptyOwners))
    );

    let one = Address::generate(&env);
    let mut owners = Vec::new(&env);
    owners.push_back(one.clone());
    assert_eq!(
        client.try_initialize(&owners, &0),
        Err(Ok(Error::InvalidThreshold))
    );
    assert_eq!(
        client.try_initialize(&owners, &2),
        Err(Ok(Error::InvalidThreshold))
    );

    assert_eq!(client.try_initialize(&owners, &1), Ok(Ok(())));
    assert_eq!(
        client.try_initialize(&owners, &1),
        Err(Ok(Error::AlreadyInitialized))
    );
}

#[test]
fn initialize_rejects_duplicate_owners() {
    let env = Env::default();
    let multisig = env.register(KeygateMultisig, ());
    let client = KeygateMultisigClient::new(&env, &multisig);
    env.mock_all_auths();

    let a = Address::generate(&env);
    let mut owners = Vec::new(&env);
    owners.push_back(a.clone());
    owners.push_back(a);
    assert_eq!(
        client.try_initialize(&owners, &2),
        Err(Ok(Error::DuplicateOwner))
    );
}

#[test]
fn submit_auto_approves_for_the_proposer() {
    let f = Fixture::new(2);
    let id = f.submit_record(&f.a, 42);
    assert_eq!(id, 1);

    let proposal = f.client().proposal(&id).unwrap();
    assert_eq!(proposal.target, f.recorder);
    assert!(!proposal.executed);
    assert_eq!(proposal.approvals.len(), 1);
    assert_eq!(f.client().live_ids().len(), 1);
    // Nothing has run yet.
    assert_eq!(f.recorder().count(), 0);
}

#[test]
fn execute_waits_for_quorum_then_relays_the_call() {
    let f = Fixture::new(2);
    let id = f.submit_record(&f.a, 42);

    assert_eq!(
        f.client().try_execute(&f.a, &id),
        Err(Ok(Error::QuorumNotMet))
    );
    assert_eq!(f.recorder().count(), 0);

    assert_eq!(f.client().approve(&f.b, &id), 2);
    f.client().execute(&f.a, &id);

    assert_eq!(f.recorder().last(), Some(42));
    assert_eq!(f.recorder().count(), 1);
    assert!(f.client().proposal(&id).unwrap().executed);
    // Executed work leaves the pending set for the indexer's cursor.
    assert_eq!(f.client().live_ids().len(), 0);
}

#[test]
fn a_third_approval_is_not_required() {
    let f = Fixture::new(2);
    let id = f.submit_record(&f.a, 7);
    f.client().approve(&f.b, &id);
    f.client().execute(&f.c, &id);
    assert_eq!(f.recorder().last(), Some(7));
}

#[test]
fn executing_twice_is_refused() {
    let f = Fixture::new(1);
    let id = f.submit_record(&f.a, 1);
    f.client().execute(&f.a, &id);

    assert_eq!(
        f.client().try_execute(&f.a, &id),
        Err(Ok(Error::AlreadyExecuted))
    );
    assert_eq!(f.recorder().count(), 1);
}

#[test]
fn approving_twice_is_refused() {
    let f = Fixture::new(2);
    let id = f.submit_record(&f.a, 1);
    assert_eq!(
        f.client().try_approve(&f.a, &id),
        Err(Ok(Error::DuplicateApproval))
    );
    assert_eq!(f.client().approve(&f.b, &id), 2);
}

#[test]
fn unapprove_withdraws_support_and_can_block_execution() {
    let f = Fixture::new(2);
    let id = f.submit_record(&f.a, 1);
    f.client().approve(&f.b, &id);

    assert_eq!(f.client().unapprove(&f.b, &id), 1);
    assert_eq!(
        f.client().try_execute(&f.a, &id),
        Err(Ok(Error::QuorumNotMet))
    );

    f.client().approve(&f.b, &id);
    f.client().execute(&f.a, &id);
    assert_eq!(f.recorder().count(), 1);
}

#[test]
fn unapprove_by_someone_who_never_approved_is_refused() {
    let f = Fixture::new(2);
    let id = f.submit_record(&f.a, 1);
    assert_eq!(
        f.client().try_unapprove(&f.b, &id),
        Err(Ok(Error::DuplicateApproval))
    );
}

#[test]
fn cancel_closes_an_open_proposal() {
    let f = Fixture::new(2);
    let id = f.submit_record(&f.a, 1);
    f.client().cancel(&f.a, &id);

    assert!(f.client().proposal(&id).is_none());
    assert_eq!(f.client().live_ids().len(), 0);
    assert_eq!(
        f.client().try_execute(&f.a, &id),
        Err(Ok(Error::ProposalNotFound))
    );
}

#[test]
fn cannot_cancel_a_proposal_that_already_ran() {
    let f = Fixture::new(1);
    let id = f.submit_record(&f.a, 1);
    f.client().execute(&f.a, &id);
    assert_eq!(
        f.client().try_cancel(&f.a, &id),
        Err(Ok(Error::AlreadyExecuted))
    );
}

#[test]
fn non_owners_cannot_touch_proposals() {
    let f = Fixture::new(2);
    let id = f.submit_record(&f.a, 1);
    let outsider_args = f.record_args(9);

    assert_eq!(
        f.client().try_submit(
            &f.outsider,
            &f.recorder,
            &symbol_short!("record"),
            &outsider_args
        ),
        Err(Ok(Error::NotAnOwner))
    );
    assert_eq!(
        f.client().try_approve(&f.outsider, &id),
        Err(Ok(Error::NotAnOwner))
    );
    assert_eq!(
        f.client().try_execute(&f.outsider, &id),
        Err(Ok(Error::NotAnOwner))
    );
    assert_eq!(f.recorder().count(), 0);
}

#[test]
fn unknown_proposal_ids_are_not_found() {
    let f = Fixture::new(2);
    assert!(f.client().proposal(&99).is_none());
    assert_eq!(f.client().approvals(&99).len(), 0);
    assert_eq!(
        f.client().try_approve(&f.a, &99),
        Err(Ok(Error::ProposalNotFound))
    );
}

#[test]
fn proposals_relay_arguments_of_several_types() {
    let f = Fixture::new(1);
    let named = f.env.register(Named, ());
    let label = String::from_str(&f.env, "cold-storage");

    let mut args = Vec::new(&f.env);
    args.push_back(label.clone().into_val(&f.env));
    let id = f
        .client()
        .submit(&f.a, &named, &symbol_short!("tag"), &args);
    f.client().execute(&f.a, &id);

    // The recorder was never involved, so its counter proves the relay targeted
    // `named` and deserialised the String argument correctly.
    assert_eq!(f.recorder().count(), 0);
    assert_eq!(f.client().proposal(&id).unwrap().func, symbol_short!("tag"));
}

#[test]
fn owner_set_can_be_rotated_through_a_quorum_proposal() {
    let f = Fixture::new(2);
    let next_owners = {
        let mut list = Vec::new(&f.env);
        list.push_back(f.b.clone());
        list.push_back(f.c.clone());
        list
    };

    let mut args = Vec::new(&f.env);
    args.push_back(next_owners.clone().into_val(&f.env));
    args.push_back(2u32.into_val(&f.env));
    let id = f
        .client()
        .submit(&f.a, &f.multisig, &Symbol::new(&f.env, "set_owners"), &args);
    f.client().approve(&f.c, &id);
    f.client().execute(&f.a, &id);

    assert_eq!(f.client().owners().len(), 2);
    assert_eq!(f.client().threshold(), 2);
    assert!(!f.client().is_owner(&f.a));
    assert!(f.client().is_owner(&f.b));

    // The removed owner can no longer propose; the new ones can.
    let stale_args = f.record_args(3);
    assert_eq!(
        f.client()
            .try_submit(&f.a, &f.recorder, &symbol_short!("record"), &stale_args),
        Err(Ok(Error::NotAnOwner))
    );
    let follow_up = f.submit_record(&f.b, 3);
    assert_eq!(f.client().proposal(&follow_up).unwrap().approvals.len(), 1);
}

#[test]
fn a_rejected_rotation_leaves_owners_intact() {
    let f = Fixture::new(2);
    let empty: Vec<Address> = Vec::new(&f.env);
    let mut args = Vec::new(&f.env);
    args.push_back(empty.into_val(&f.env));
    args.push_back(1u32.into_val(&f.env));

    let id = f
        .client()
        .submit(&f.a, &f.multisig, &Symbol::new(&f.env, "set_owners"), &args);
    f.client().approve(&f.b, &id);
    // The inner call reverts, so execute must surface that rather than mark it done.
    assert!(f.client().try_execute(&f.a, &id).is_err());
    assert_eq!(f.client().owners().len(), 3);
    assert!(!f.client().proposal(&id).unwrap().executed);
}

#[test]
fn proposal_ids_are_monotonic_across_owners() {
    let f = Fixture::new(2);
    assert_eq!(f.submit_record(&f.a, 1), 1);
    assert_eq!(f.submit_record(&f.b, 2), 2);
    assert_eq!(f.submit_record(&f.c, 3), 3);
    assert_eq!(f.client().live_ids().len(), 3);
    assert_eq!(f.client().proposal(&2).unwrap().proposed_at, 1_000);
}
