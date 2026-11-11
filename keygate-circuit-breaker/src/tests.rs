//! Unit tests for keygate-circuit-breaker. Run with `cargo test -p keygate-circuit-breaker`.
//!
//! Mocked auths are still recorded, so the `*_requires_*` tests at the bottom
//! assert on `env.auths()` to pin down who has to sign and for what.
//!
//! The last section goes further and puts a throwaway guarded caller in front
//! of a real registry and a real breaker, because the rule that a breach must
//! be *skipped* rather than returned as an error is a rule about the
//! transaction boundary and cannot be shown by calling `enforce` directly.

// The crate is `no_std`, but the test harness is not, and `env.auths()` hands
// back a `std::vec::Vec`.
extern crate std;

use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype,
    testutils::{Address as _, AuthorizedFunction, AuthorizedInvocation, Ledger as _},
    Address, Env, IntoVal, InvokeError, String, Symbol, Val, Vec,
};

use keygate_core::{KeygateCore, KeygateCoreClient};

use super::{Error, KeygateBreaker, KeygateBreakerClient, Outcome};

const RESOURCE: &str = "vault:main";
const WINDOW: u64 = 3_600;
const LIMIT: i128 = 1_000;

struct Fixture {
    env: Env,
    contract: Address,
    admin: Address,
}

impl Fixture {
    /// Deploys a breaker with `vault:main` limited to 1000 per hour.
    fn new(now: u64) -> Fixture {
        let env = Env::default();
        env.ledger().set_timestamp(now);
        let contract = env.register(KeygateBreaker, ());
        let admin = Address::generate(&env);
        env.mock_all_auths();
        let client = KeygateBreakerClient::new(&env, &contract);
        client.initialize(&admin);
        client.configure(&String::from_str(&env, RESOURCE), &LIMIT, &WINDOW);
        Fixture {
            env,
            contract,
            admin,
        }
    }

    fn client(&self) -> KeygateBreakerClient<'_> {
        KeygateBreakerClient::new(&self.env, &self.contract)
    }

    fn resource(&self) -> String {
        String::from_str(&self.env, RESOURCE)
    }

    fn advance(&self, seconds: u64) {
        let now = self.env.ledger().timestamp();
        self.env.ledger().set_timestamp(now + seconds);
    }
}

/// Asserts the last invocation required exactly `expected` to sign, in the
/// order the `require_auth` calls were made, and that each of them signed for
/// `func` on `contract` with `args`.
///
/// Signature checks belong to the host, so `env.auths()` is the only way a unit
/// test can see them. Asserting here is what turns a dropped `require_auth`
/// into a failing test rather than a silent widening of who may call in.
#[track_caller]
fn assert_signed_by(
    env: &Env,
    contract: &Address,
    func: Symbol,
    args: Vec<Val>,
    expected: &[&Address],
) {
    let mut want = std::vec::Vec::new();
    for &address in expected {
        want.push((
            address.clone(),
            AuthorizedInvocation {
                function: AuthorizedFunction::Contract((
                    contract.clone(),
                    func.clone(),
                    args.clone(),
                )),
                sub_invocations: std::vec::Vec::new(),
            },
        ));
    }
    assert_eq!(env.auths(), want);
}

#[test]
fn charges_reduce_the_remaining_budget() {
    let f = Fixture::new(1_000);
    assert_eq!(f.client().enforce(&f.resource(), &400), Outcome::Charged);
    assert_eq!(f.client().remaining(&f.resource()), 600);

    f.client().enforce(&f.resource(), &250);
    assert_eq!(f.client().remaining(&f.resource()), 350);

    let state = f.client().state(&f.resource()).unwrap();
    assert_eq!(state.used, 650);
    assert!(!state.tripped);
}

#[test]
fn spending_exactly_the_limit_is_allowed() {
    let f = Fixture::new(1_000);
    f.client().enforce(&f.resource(), &LIMIT);
    assert_eq!(f.client().remaining(&f.resource()), 0);
    assert!(!f.client().is_tripped(&f.resource()));
}

#[test]
fn crossing_the_limit_reports_breached_and_trips() {
    let f = Fixture::new(1_000);
    f.client().enforce(&f.resource(), &900);

    // The breaching call *succeeds*. A contract that returns an error has its
    // own writes discarded, so an error here would also discard the latch it
    // had just recorded — and a breaker that forgets it tripped is only a
    // comparison. The breach is a value the caller has to act on.
    assert_eq!(f.client().enforce(&f.resource(), &200), Outcome::Breached);
    assert!(f.client().is_tripped(&f.resource()));
    assert_eq!(f.client().remaining(&f.resource()), 0);

    // Once tripped, even a trivial amount is refused, and there the refusal can
    // be an error because there is nothing left to record. This is the point:
    // the resource stops bleeding instead of allowing the next small withdrawal.
    let after = f.client().try_enforce(&f.resource(), &1);
    assert_eq!(after, Err(Ok(Error::Tripped)));
}

#[test]
fn a_breach_is_recorded_with_the_offending_total() {
    let f = Fixture::new(1_000);
    f.client().enforce(&f.resource(), &900);
    f.client().enforce(&f.resource(), &200);

    let state = f.client().state(&f.resource()).unwrap();
    assert_eq!(state.used, 1_100);
    assert!(state.tripped);
}

#[test]
fn the_trip_survives_the_window_rolling_over() {
    let f = Fixture::new(1_000);
    f.client().enforce(&f.resource(), &2_000);
    f.advance(WINDOW + 1);

    // Time passing must not silently re-open a latched resource.
    let still = f.client().try_enforce(&f.resource(), &1);
    assert_eq!(still, Err(Ok(Error::Tripped)));
}

#[test]
fn an_untripped_window_resets_after_it_elapses() {
    let f = Fixture::new(1_000);
    f.client().enforce(&f.resource(), &900);
    assert_eq!(f.client().remaining(&f.resource()), 100);

    f.advance(WINDOW);
    assert_eq!(f.client().remaining(&f.resource()), LIMIT);
    f.client().enforce(&f.resource(), &900);
    assert_eq!(f.client().remaining(&f.resource()), 100);
}

#[test]
fn a_new_window_anchors_on_the_first_charge_after_expiry() {
    let f = Fixture::new(1_000);
    f.client().enforce(&f.resource(), &500);

    f.advance(WINDOW - 10);
    // Still inside the original window.
    assert_eq!(f.client().remaining(&f.resource()), 500);

    f.advance(20);
    assert_eq!(f.client().remaining(&f.resource()), LIMIT);
    // A view must not persist the roll, so storage still holds the old window.
    let stale = f.client().state(&f.resource()).unwrap();
    assert_eq!(stale.window_start, 1_000);
    assert_eq!(stale.used, 500);

    f.client().enforce(&f.resource(), &600);
    let rolled = f.client().state(&f.resource()).unwrap();
    assert_eq!(rolled.window_start, 4_610);
    assert_eq!(rolled.used, 600);
}

#[test]
fn reset_clears_the_latch_and_starts_a_fresh_window() {
    let f = Fixture::new(1_000);
    f.client().enforce(&f.resource(), &2_000);
    assert!(f.client().is_tripped(&f.resource()));

    f.client().reset(&f.resource());
    assert!(!f.client().is_tripped(&f.resource()));
    assert_eq!(f.client().remaining(&f.resource()), LIMIT);
    f.client().enforce(&f.resource(), &10);
}

#[test]
fn trip_closes_a_resource_before_any_breach() {
    let f = Fixture::new(1_000);
    f.client().trip(&f.resource());
    let blocked = f.client().try_enforce(&f.resource(), &1);
    assert_eq!(blocked, Err(Ok(Error::Tripped)));

    f.client().reset(&f.resource());
    f.client().enforce(&f.resource(), &1);
}

#[test]
fn non_positive_amounts_are_refused_without_charging() {
    let f = Fixture::new(1_000);
    assert_eq!(
        f.client().try_enforce(&f.resource(), &0),
        Err(Ok(Error::InvalidAmount))
    );
    assert_eq!(
        f.client().try_enforce(&f.resource(), &-5),
        Err(Ok(Error::InvalidAmount))
    );
    let state = f.client().state(&f.resource());
    assert!(state.is_none());
}

#[test]
fn an_unknown_resource_is_not_configured() {
    let f = Fixture::new(1_000);
    let other = String::from_str(&f.env, "vault:other");
    assert_eq!(
        f.client().try_enforce(&other, &1),
        Err(Ok(Error::NotConfigured))
    );
    assert!(f.client().config(&other).is_none());
    assert!(!f.client().is_tripped(&other));
}

#[test]
fn invalid_limits_and_windows_are_rejected() {
    let f = Fixture::new(1_000);
    // `configure` succeeds with a `Config`, which carries no `PartialEq`, so the
    // rejections are matched by arm rather than compared as whole values.
    assert!(matches!(
        f.client().try_configure(&f.resource(), &0, &WINDOW),
        Err(Ok(Error::InvalidLimit))
    ));
    assert!(matches!(
        f.client().try_configure(&f.resource(), &-1, &WINDOW),
        Err(Ok(Error::InvalidLimit))
    ));
    assert!(matches!(
        f.client().try_configure(&f.resource(), &LIMIT, &0),
        Err(Ok(Error::InvalidWindow))
    ));
    // The original configuration survived the rejected writes.
    let config = f.client().config(&f.resource()).unwrap();
    assert_eq!(config.limit, LIMIT);
    assert_eq!(config.window, WINDOW);
}

#[test]
fn reconfiguring_does_not_duplicate_the_resource_list() {
    let f = Fixture::new(1_000);
    f.client().configure(&f.resource(), &2_000, &WINDOW);
    assert_eq!(f.client().resources().len(), 1);

    let second = String::from_str(&f.env, "bridge:out");
    f.client().configure(&second, &500, &WINDOW);
    assert_eq!(f.client().resources().len(), 2);
}

#[test]
fn replacing_a_limit_does_not_forgive_a_trip() {
    let f = Fixture::new(1_000);
    f.client().enforce(&f.resource(), &2_000);

    f.client().configure(&f.resource(), &10_000, &WINDOW);
    let blocked = f.client().try_enforce(&f.resource(), &1);
    assert_eq!(blocked, Err(Ok(Error::Tripped)));
}

#[test]
fn initialize_rejects_a_second_call() {
    let f = Fixture::new(1_000);
    let stranger = Address::generate(&f.env);
    assert_eq!(
        f.client().try_initialize(&stranger),
        Err(Ok(Error::AlreadyInitialized))
    );
    assert_eq!(f.client().admin(), Some(f.admin.clone()));
}

// --- who has to sign -------------------------------------------------------
//
// One test per entry point that calls `require_auth`, each asserting the exact
// authorisation the host recorded: address, contract function and arguments.
// The state-transition tests above cannot catch a dropped `require_auth`.
//
// `enforce`, `remaining` and the views are deliberately unauthorised: the
// breaker is a budget guard for callers the registry has already authenticated.

#[test]
fn initialize_requires_the_admin() {
    let env = Env::default();
    let contract = env.register(KeygateBreaker, ());
    let admin = Address::generate(&env);
    env.mock_all_auths();

    KeygateBreakerClient::new(&env, &contract).initialize(&admin);

    assert_signed_by(
        &env,
        &contract,
        Symbol::new(&env, "initialize"),
        (admin.clone(),).into_val(&env),
        &[&admin],
    );
}

#[test]
fn configure_requires_the_admin() {
    let f = Fixture::new(1_000);

    f.client().configure(&f.resource(), &2_000, &WINDOW);

    assert_signed_by(
        &f.env,
        &f.contract,
        Symbol::new(&f.env, "configure"),
        (f.resource(), 2_000_i128, WINDOW).into_val(&f.env),
        &[&f.admin],
    );
}

#[test]
fn trip_requires_the_admin() {
    let f = Fixture::new(1_000);

    f.client().trip(&f.resource());

    assert_signed_by(
        &f.env,
        &f.contract,
        Symbol::new(&f.env, "trip"),
        (f.resource(),).into_val(&f.env),
        &[&f.admin],
    );
}

#[test]
fn reset_requires_the_admin() {
    let f = Fixture::new(1_000);
    f.client().trip(&f.resource());

    f.client().reset(&f.resource());

    // Re-opening a latched resource hands the budget back, so it is an admin
    // action even when the latch came from an automatic breach.
    assert_signed_by(
        &f.env,
        &f.contract,
        Symbol::new(&f.env, "reset"),
        (f.resource(),).into_val(&f.env),
        &[&f.admin],
    );
}

// --- a guarded caller, end to end -------------------------------------------
//
// The module docs, the README and SECURITY.md all say the same thing: a caller
// must skip the guarded action on `Outcome::Breached` instead of returning an
// error, because a revert would discard the latch that `enforce` just wrote.
// That is a claim about the transaction boundary, so no amount of calling
// `enforce` directly can support it — the tests above only prove the latch is
// set inside a frame that succeeds.
//
// What follows is the smallest honest version of an integrator: a throwaway
// contract that authenticates through a real keygate-core registry, charges a
// real breaker, and only then performs a recorded action.

/// The guarded caller's own bookkeeping. Written only when the action runs, so
/// a test can see whether the action happened, how often and for how much.
#[contracttype]
#[derive(Clone)]
enum CallerKey {
    Count,
    Total,
}

/// Why the guarded call refused. Test-only, so it is deliberately blunt: a
/// real integrator would carry its own domain errors here.
#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum CallerError {
    /// `keygate-core` said no. The breaker must not have been charged.
    Unauthorised = 1,
    /// The breaker refused outright (tripped, unconfigured, bad amount).
    BreakerRefused = 2,
    /// The deliberately-failing caller acting after the charge it was given.
    RefusedAfterCharging = 3,
}

/// Stands in for an integrator's business contract.
#[contract]
pub struct GuardedCaller;

#[contractimpl]
impl GuardedCaller {
    /// Authenticate, charge, then act — but only on `Charged`. Answering a
    /// breach with `Ok(Outcome::Breached)` is what keeps the latch under it.
    pub fn guarded(
        env: Env,
        core: Address,
        breaker: Address,
        caller: Address,
        resource: String,
        action: String,
        amount: i128,
    ) -> Result<Outcome, CallerError> {
        let outcome =
            authorise_then_charge(&env, &core, &breaker, &caller, &resource, &action, amount)?;
        if outcome == Outcome::Charged {
            record(&env, amount);
        }
        Ok(outcome)
    }

    /// The same call written the way the docs warn against: it acts and then
    /// returns an error, whatever `enforce` answered. Everything it wrote — and
    /// everything the breaker wrote on its behalf — should go with the frame.
    pub fn guarded_then_fail(
        env: Env,
        core: Address,
        breaker: Address,
        caller: Address,
        resource: String,
        action: String,
        amount: i128,
    ) -> Result<Outcome, CallerError> {
        authorise_then_charge(&env, &core, &breaker, &caller, &resource, &action, amount)?;
        record(&env, amount);
        Err(CallerError::RefusedAfterCharging)
    }

    /// How many guarded actions ran.
    pub fn count(env: Env) -> u32 {
        env.storage()
            .persistent()
            .get(&CallerKey::Count)
            .unwrap_or(0)
    }

    /// The total those actions carried.
    pub fn total(env: Env) -> i128 {
        env.storage()
            .persistent()
            .get(&CallerKey::Total)
            .unwrap_or(0)
    }
}

/// The composition the README spells out: the registry authenticates the
/// caller, and only then is the breaker asked for budget.
fn authorise_then_charge(
    env: &Env,
    core: &Address,
    breaker: &Address,
    caller: &Address,
    resource: &String,
    action: &String,
    amount: i128,
) -> Result<Outcome, CallerError> {
    match KeygateCoreClient::new(env, core).try_require_access(caller, resource, action) {
        Ok(Ok(())) => {}
        // A missing grant, an expired one, a refused signature — all the same
        // here: stop before a single unit of budget is spent.
        _ => return Err(CallerError::Unauthorised),
    }
    match KeygateBreakerClient::new(env, breaker).try_enforce(resource, &amount) {
        Ok(Ok(outcome)) => Ok(outcome),
        _ => Err(CallerError::BreakerRefused),
    }
}

fn record(env: &Env, amount: i128) {
    let count: u32 = env
        .storage()
        .persistent()
        .get(&CallerKey::Count)
        .unwrap_or(0);
    let total: i128 = env
        .storage()
        .persistent()
        .get(&CallerKey::Total)
        .unwrap_or(0);
    env.storage()
        .persistent()
        .set(&CallerKey::Count, &(count + 1));
    env.storage()
        .persistent()
        .set(&CallerKey::Total, &(total + amount));
}

const ACTION: &str = "withdraw";
const TTL: u64 = 3_600;

/// Collapses a `try_` answer to the contract's own result, so an expected
/// refusal stays inspectable as a value and a broken invocation is a test bug
/// rather than a quietly unwrapped `None`.
fn land<T, E>(
    answer: Result<Result<T, impl core::fmt::Debug>, Result<E, InvokeError>>,
) -> Result<T, E> {
    match answer {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(conversion)) => panic!("result did not convert: {:?}", conversion),
        Err(Ok(refusal)) => Err(refusal),
        Err(Err(invoke)) => panic!("invocation failed: {:?}", invoke),
    }
}

/// A registry, a breaker and a guarded caller, wired the way a deployment is.
struct Stack {
    env: Env,
    core: Address,
    breaker: Address,
    guarded: Address,
    caller: Address,
    resource: String,
}

impl Stack {
    /// `caller` holds `withdraw` on `vault:main`, and the breaker allows `limit`
    /// per hour.
    fn new(limit: i128) -> Stack {
        let env = Env::default();
        env.ledger().set_timestamp(1_000);
        // The guarded caller sits between the test and both contracts, so the
        // caller's signature is checked from a *non-root* frame. Plain
        // `mock_all_auths` only covers root auth and would abort instead.
        env.mock_all_auths_allowing_non_root_auth();
        let core = env.register(KeygateCore, ());
        let breaker = env.register(KeygateBreaker, ());
        let guarded = env.register(GuardedCaller, ());
        let admin = Address::generate(&env);
        let caller = Address::generate(&env);
        let resource = String::from_str(&env, RESOURCE);
        let action = String::from_str(&env, ACTION);

        let registry = KeygateCoreClient::new(&env, &core);
        registry.initialize(&admin);
        registry.issue(&resource, &action, &caller, &TTL);

        let guard = KeygateBreakerClient::new(&env, &breaker);
        guard.initialize(&admin);
        guard.configure(&resource, &limit, &WINDOW);

        Stack {
            env,
            core,
            breaker,
            guarded,
            caller,
            resource,
        }
    }

    fn core_client(&self) -> KeygateCoreClient<'_> {
        KeygateCoreClient::new(&self.env, &self.core)
    }

    fn breaker_client(&self) -> KeygateBreakerClient<'_> {
        KeygateBreakerClient::new(&self.env, &self.breaker)
    }

    fn guard_client(&self) -> GuardedCallerClient<'_> {
        GuardedCallerClient::new(&self.env, &self.guarded)
    }

    fn action(&self) -> String {
        String::from_str(&self.env, ACTION)
    }

    fn remaining(&self) -> i128 {
        self.breaker_client().remaining(&self.resource)
    }

    fn call(&self, amount: i128) -> Result<Outcome, CallerError> {
        land(self.guard_client().try_guarded(
            &self.core,
            &self.breaker,
            &self.caller,
            &self.resource,
            &self.action(),
            &amount,
        ))
    }

    fn call_then_fail(&self, amount: i128) -> Result<Outcome, CallerError> {
        land(self.guard_client().try_guarded_then_fail(
            &self.core,
            &self.breaker,
            &self.caller,
            &self.resource,
            &self.action(),
            &amount,
        ))
    }
}

#[test]
fn an_authorised_charge_runs_the_guarded_action() {
    let s = Stack::new(LIMIT);

    assert_eq!(s.call(400), Ok(Outcome::Charged));

    assert_eq!(s.guard_client().count(), 1);
    assert_eq!(s.guard_client().total(), 400);
    assert_eq!(s.remaining(), 600);
}

#[test]
fn a_breach_skips_the_action_and_leaves_the_latch_closed() {
    let s = Stack::new(LIMIT);
    assert_eq!(s.call(900), Ok(Outcome::Charged));

    // `enforce` reported a breach and the caller answered with success, so the
    // latch written inside the call still stands when the call returns.
    assert_eq!(s.call(200), Ok(Outcome::Breached));
    assert_eq!(s.guard_client().count(), 1);
    assert_eq!(s.guard_client().total(), 900);
    assert!(s.breaker_client().is_tripped(&s.resource));

    // And the next attempt meets a closed resource, not a fresh window.
    assert_eq!(
        s.breaker_client().try_enforce(&s.resource, &1),
        Err(Ok(Error::Tripped))
    );
}

#[test]
fn a_failing_caller_rolls_back_the_charge_and_the_latch() {
    let s = Stack::new(LIMIT);
    assert_eq!(s.call(400), Ok(Outcome::Charged));

    // Charged, acted, then refused: the failed frame's usage is undone, and the
    // 400 from the call that completed is not touched.
    assert_eq!(
        s.call_then_fail(300),
        Err(CallerError::RefusedAfterCharging)
    );
    assert_eq!(s.guard_client().count(), 1);
    assert_eq!(s.guard_client().total(), 400);
    assert_eq!(s.remaining(), 600);

    // The same when the failure lands on top of a breach: the latch was written
    // by `enforce`, and it goes down with the caller's frame.
    assert_eq!(
        s.call_then_fail(900),
        Err(CallerError::RefusedAfterCharging)
    );
    assert!(!s.breaker_client().is_tripped(&s.resource));
    assert_eq!(s.remaining(), 600);

    // Proof the rollback was complete rather than merely hidden: the whole 600
    // is still spendable, and charging it trips nothing.
    assert_eq!(s.call(600), Ok(Outcome::Charged));
    assert_eq!(s.remaining(), 0);
    assert!(!s.breaker_client().is_tripped(&s.resource));
}

#[test]
fn a_caller_without_the_capability_spends_no_budget() {
    let s = Stack::new(LIMIT);
    s.core_client().revoke(&s.resource, &s.action(), &s.caller);

    assert_eq!(s.call(1), Err(CallerError::Unauthorised));

    // Not "used zero": never touched. No state record means `enforce` was never
    // reached, so a refused authorisation cannot be used to spend budget or to
    // trip a shared resource out of spite.
    assert!(s.breaker_client().state(&s.resource).is_none());
    assert_eq!(s.remaining(), LIMIT);
    assert_eq!(s.guard_client().count(), 0);
}
