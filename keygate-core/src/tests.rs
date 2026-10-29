//! Unit tests for keygate-core. Run with `cargo test -p keygate-core`.
//!
//! Authorisation itself is enforced by the Soroban host rather than by our own
//! comparisons, so these tests mock all auths and focus on state transitions.

use soroban_sdk::{testutils::Address as _, testutils::Ledger as _, Address, Env, String};

use super::{Error, Grant, KeygateCore, KeygateCoreClient};

const RESOURCE: &str = "vault:main";
const ACTION: &str = "withdraw";

/// A deployed registry plus the three addresses every test needs.
struct Fixture {
    env: Env,
    contract: Address,
    admin: Address,
    holder: Address,
    other: Address,
}

impl Fixture {
    /// Deploys the contract, sets the clock to `now` and initialises the admin.
    fn new(now: u64) -> Fixture {
        let env = Env::default();
        env.ledger().set_timestamp(now);
        let contract = env.register(KeygateCore, ());
        let admin = Address::generate(&env);
        let holder = Address::generate(&env);
        let other = Address::generate(&env);
        env.mock_all_auths();
        KeygateCoreClient::new(&env, &contract).initialize(&admin);
        Fixture {
            env,
            contract,
            admin,
            holder,
            other,
        }
    }

    fn client(&self) -> KeygateCoreClient<'_> {
        KeygateCoreClient::new(&self.env, &self.contract)
    }

    fn resource(&self) -> String {
        String::from_str(&self.env, RESOURCE)
    }

    fn action(&self) -> String {
        String::from_str(&self.env, ACTION)
    }

    fn advance(&self, seconds: u64) {
        let now = self.env.ledger().timestamp();
        self.env.ledger().set_timestamp(now + seconds);
    }

    /// Issues the canonical capability for `holder` with a one hour ttl.
    fn issue(&self) -> Grant {
        self.client()
            .issue(&self.resource(), &self.action(), &self.holder, &3_600)
    }
}

#[test]
fn initialize_records_admin_and_rejects_a_second_call() {
    let f = Fixture::new(1_000);
    assert_eq!(f.client().admin(), Some(f.admin.clone()));

    let again = f.client().try_initialize(&f.other);
    assert_eq!(again, Err(Ok(Error::AlreadyInitialized)));
}

#[test]
fn issue_then_check_grants_access() {
    let f = Fixture::new(1_000);
    f.issue();

    assert!(f
        .client()
        .check(&f.resource(), &f.action(), &f.holder));
    assert!(!f.client().check(&f.resource(), &f.action(), &f.other));
}

#[test]
fn check_after_expiry_denies_and_prunes_the_grant() {
    let f = Fixture::new(1_000);
    f.issue();
    assert_eq!(f.client().live_ids().len(), 1);

    f.advance(3_601);

    assert!(!f
        .client()
        .check(&f.resource(), &f.action(), &f.holder));
    assert_eq!(f.client().live_ids().len(), 0);
    assert!(f.client().grant(&f.resource(), &f.action(), &f.holder).is_none());
}

#[test]
fn expiry_on_the_dot_is_denied() {
    let f = Fixture::new(1_000);
    f.issue();
    f.advance(3_600);
    assert!(!f
        .client()
        .check(&f.resource(), &f.action(), &f.holder));
}

#[test]
fn revoke_removes_access() {
    let f = Fixture::new(1_000);
    let grant = f.issue();

    let id = f.client().revoke(&f.resource(), &f.action(), &f.holder);
    assert_eq!(id, grant.id);
    assert!(!f
        .client()
        .check(&f.resource(), &f.action(), &f.holder));

    let missing = f.client().try_revoke(&f.resource(), &f.action(), &f.holder);
    assert_eq!(missing, Err(Ok(Error::GrantNotFound)));
}

#[test]
fn reissue_extends_expiry_but_keeps_the_same_id() {
    let f = Fixture::new(1_000);
    let first = f
        .client()
        .issue(&f.resource(), &f.action(), &f.holder, &60);
    let second = f
        .client()
        .issue(&f.resource(), &f.action(), &f.holder, &600);

    assert_eq!(first.id, second.id);
    assert_eq!(second.expires_at, 1_600);
    // A renewal must not leak a second entry into the live set.
    assert_eq!(f.client().live_ids().len(), 1);
}

#[test]
fn zero_ttl_is_rejected() {
    let f = Fixture::new(1_000);
    let bad = f
        .client()
        .try_issue(&f.resource(), &f.action(), &f.holder, &0);
    assert_eq!(bad, Err(Ok(Error::ZeroTtl)));
}

#[test]
fn pause_blocks_changes_but_not_checks() {
    let f = Fixture::new(1_000);
    f.issue();

    f.client().pause();
    assert!(f.client().paused());

    let blocked = f
        .client()
        .try_issue(&f.resource(), &f.action(), &f.other, &60);
    assert_eq!(blocked, Err(Ok(Error::Paused)));

    let blocked_revoke = f.client().try_revoke(&f.resource(), &f.action(), &f.holder);
    assert_eq!(blocked_revoke, Err(Ok(Error::Paused)));

    // Existing capabilities keep resolving while the registry is frozen.
    assert!(f
        .client()
        .check(&f.resource(), &f.action(), &f.holder));

    f.client().unpause();
    f.client().revoke(&f.resource(), &f.action(), &f.holder);
}

#[test]
fn admin_handover_is_two_step() {
    let f = Fixture::new(1_000);
    let successor = Address::generate(&f.env);

    f.client().propose_admin(&successor);
    // Nothing changes until the nominee accepts.
    assert_eq!(f.client().admin(), Some(f.admin.clone()));

    assert_eq!(f.client().accept_admin(), successor.clone());
    assert_eq!(f.client().admin(), Some(successor.clone()));

    // Control now sits with the successor.
    f.client()
        .issue(&f.resource(), &f.action(), &f.holder, &60);
}

#[test]
fn cancel_pending_handover_leaves_admin_untouched() {
    let f = Fixture::new(1_000);
    let successor = Address::generate(&f.env);
    f.client().propose_admin(&successor);
    f.client().cancel_admin();

    let missing = f.client().try_accept_admin();
    assert_eq!(missing, Err(Ok(Error::NoPendingAdmin)));
    assert_eq!(f.client().admin(), Some(f.admin.clone()));
}

#[test]
fn grants_of_returns_only_the_requested_holder() {
    let f = Fixture::new(1_000);
    let read = String::from_str(&f.env, "read");
    f.client()
        .issue(&f.resource(), &f.action(), &f.holder, &3_600);
    f.client()
        .issue(&f.resource(), &f.action(), &f.other, &3_600);
    f.client().issue(&f.resource(), &read, &f.holder, &3_600);

    let mine = f.client().grants_of(&f.holder);
    assert_eq!(mine.len(), 2);
    for grant in mine.iter() {
        assert_eq!(grant.holder, f.holder.clone());
    }
    assert_eq!(f.client().grants_of(&f.other).len(), 1);
}

#[test]
fn grants_of_skips_expired_entries() {
    let f = Fixture::new(1_000);
    f.issue();
    f.advance(3_601);
    assert_eq!(f.client().grants_of(&f.holder).len(), 0);
}

#[test]
fn require_access_accepts_the_holder() {
    let f = Fixture::new(1_000);
    f.issue();
    f.client()
        .require_access(&f.holder, &f.resource(), &f.action());
}

#[test]
fn require_access_denies_someone_without_the_capability() {
    let f = Fixture::new(1_000);
    f.issue();
    let denied = f
        .client()
        .try_require_access(&f.other, &f.resource(), &f.action());
    assert_eq!(denied, Err(Ok(Error::GrantNotFound)));
}

#[test]
fn distinct_actions_on_one_resource_are_independent() {
    let f = Fixture::new(1_000);
    let read = String::from_str(&f.env, "read");
    f.client().issue(&f.resource(), &read, &f.holder, &3_600);

    assert!(f.client().check(&f.resource(), &read, &f.holder));
    assert!(!f
        .client()
        .check(&f.resource(), &f.action(), &f.holder));
    assert_eq!(f.client().live_ids().len(), 1);
}
