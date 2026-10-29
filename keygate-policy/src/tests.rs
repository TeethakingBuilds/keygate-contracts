//! Unit tests for keygate-policy. Run with `cargo test -p keygate-policy`.
//!
//! These tests deploy the real keygate-core registry so the cross-contract
//! capability check is covered rather than mocked away.

use soroban_sdk::{
    testutils::Address as _, testutils::Ledger as _, Address, Env, String,
};

use keygate_core::{KeygateCore, KeygateCoreClient};

use super::{Effect, Error, KeygatePolicy, KeygatePolicyClient, Rule};

const RESOURCE: &str = "vault:main";
const ACTION: &str = "withdraw";

struct Fixture {
    env: Env,
    policy: Address,
    registry: Address,
    admin: Address,
    holder: Address,
    other: Address,
}

impl Fixture {
    fn new(now: u64) -> Fixture {
        let env = Env::default();
        env.ledger().set_timestamp(now);

        let registry = env.register(KeygateCore, ());
        let policy = env.register(KeygatePolicy, ());
        let admin = Address::generate(&env);
        let holder = Address::generate(&env);
        let other = Address::generate(&env);
        env.mock_all_auths();

        KeygateCoreClient::new(&env, &registry).initialize(&admin);
        KeygatePolicyClient::new(&env, &policy).initialize(&admin, &registry);

        Fixture {
            env,
            policy,
            registry,
            admin,
            holder,
            other,
        }
    }

    fn policy_client(&self) -> KeygatePolicyClient<'_> {
        KeygatePolicyClient::new(&self.env, &self.policy)
    }

    fn registry_client(&self) -> KeygateCoreClient<'_> {
        KeygateCoreClient::new(&self.env, &self.registry)
    }

    fn resource(&self) -> String {
        String::from_str(&self.env, RESOURCE)
    }

    fn action(&self) -> String {
        String::from_str(&self.env, ACTION)
    }

    /// Grants the holder a live capability on the canonical triple.
    fn grant(&self) {
        self.registry_client()
            .issue(&self.resource(), &self.action(), &self.holder, &3_600);
    }

    fn evaluate(&self, holder: &Address) -> bool {
        self.policy_client()
            .evaluate(&self.resource(), &self.action(), holder)
    }
}

#[test]
fn an_unconfigured_policy_denies_even_with_a_live_grant() {
    let f = Fixture::new(1_000);
    f.grant();

    // The registry says yes; the policy has not been told to allow anything.
    assert!(f
        .registry_client()
        .check(&f.resource(), &f.action(), &f.holder));
    assert!(!f.evaluate(&f.holder));
    assert_eq!(f.policy_client().default_effect(), Effect::Deny);
}

#[test]
fn an_allow_rule_defers_to_the_registry() {
    let f = Fixture::new(1_000);
    f.policy_client()
        .set_rule(&f.resource(), &f.action(), &Effect::Allow);

    // Allowed by policy but no capability yet.
    assert!(!f.evaluate(&f.holder));

    f.grant();
    assert!(f.evaluate(&f.holder));
    // Policy cannot invent a capability for someone who has none.
    assert!(!f.evaluate(&f.other));
}

#[test]
fn a_deny_rule_overrides_a_live_grant() {
    let f = Fixture::new(1_000);
    f.grant();
    f.policy_client()
        .set_rule(&f.resource(), &f.action(), &Effect::Allow);
    assert!(f.evaluate(&f.holder));

    f.policy_client()
        .set_rule(&f.resource(), &f.action(), &Effect::Deny);
    assert!(!f.evaluate(&f.holder));

    // Removing the rule restores registry-only behaviour.
    let removed = f.policy_client().remove_rule(&f.resource(), &f.action());
    assert_eq!(removed, 1);
    assert_eq!(f.policy_client().default_effect(), Effect::Deny);
    assert!(!f.evaluate(&f.holder));
}

#[test]
fn an_allow_default_effect_opens_everything_the_registry_allows() {
    let f = Fixture::new(1_000);
    f.grant();
    f.policy_client().set_default_effect(&Effect::Allow);
    assert!(f.evaluate(&f.holder));
    assert!(!f.evaluate(&f.other));
}

#[test]
fn deny_short_circuits_without_consulting_the_registry() {
    let f = Fixture::new(1_000);
    f.policy_client()
        .set_rule(&f.resource(), &f.action(), &Effect::Deny);
    // The registry is untouched, so there is nothing to prune and no read made.
    assert_eq!(f.registry_client().live_ids().len(), 0);
    assert!(!f.evaluate(&f.holder));
}

#[test]
fn set_rule_overwrites_in_place() {
    let f = Fixture::new(1_000);
    let first: Rule = f
        .policy_client()
        .set_rule(&f.resource(), &f.action(), &Effect::Allow);
    let second = f
        .policy_client()
        .set_rule(&f.resource(), &f.action(), &Effect::Deny);

    assert_eq!(first.id, second.id);
    assert_eq!(second.effect, Effect::Deny);
    assert_eq!(f.policy_client().rules().len(), 1);
}

#[test]
fn rules_are_listed_in_insertion_order() {
    let f = Fixture::new(1_000);
    let read = String::from_str(&f.env, "read");
    f.policy_client().set_rule(&f.resource(), &read, &Effect::Allow);
    f.policy_client()
        .set_rule(&f.resource(), &f.action(), &Effect::Deny);

    let listed = f.policy_client().rules();
    assert_eq!(listed.len(), 2);
    // Insertion order is what the indexer replays, so ids must ascend.
    let first = listed.get(0).unwrap();
    let second = listed.get(1).unwrap();
    assert_eq!(first.id, 1);
    assert_eq!(second.id, 2);
    assert_eq!(first.effect, Effect::Allow);
    assert_eq!(second.effect, Effect::Deny);

    // The first rule was keyed on `read`, so the withdraw triple is still
    // covered only by the second one.
    assert!(!f.evaluate(&f.holder));
}

#[test]
fn empty_rule_names_are_rejected() {
    let f = Fixture::new(1_000);
    let empty = String::from_str(&f.env, "");
    let bad = f.policy_client().try_set_rule(&empty, &f.action(), &Effect::Allow);
    assert_eq!(bad, Err(Ok(Error::EmptyName)));
}

#[test]
fn removing_an_absent_rule_fails() {
    let f = Fixture::new(1_000);
    let missing = f.policy_client().try_remove_rule(&f.resource(), &f.action());
    assert_eq!(missing, Err(Ok(Error::RuleNotFound)));
}

#[test]
fn repointing_the_registry_changes_which_capabilities_count() {
    let f = Fixture::new(1_000);
    f.policy_client()
        .set_rule(&f.resource(), &f.action(), &Effect::Allow);

    let replacement = f.env.register(KeygateCore, ());
    let outsider = Address::generate(&f.env);
    KeygateCoreClient::new(&f.env, &replacement).initialize(&f.admin);
    KeygateCoreClient::new(&f.env, &replacement).issue(
        &f.resource(),
        &f.action(),
        &outsider,
        &3_600,
    );

    f.policy_client().set_registry(&replacement);
    assert_eq!(f.policy_client().registry(), Some(replacement.clone()));

    assert!(!f.evaluate(&f.holder));
    assert!(f.evaluate(&outsider));
}

#[test]
fn initialize_rejects_a_second_call() {
    let f = Fixture::new(1_000);
    let other = Address::generate(&f.env);
    let bad = f.policy_client().try_initialize(&other, &f.registry);
    assert_eq!(bad, Err(Ok(Error::AlreadyInitialized)));
}

#[test]
fn admin_transfer_moves_control() {
    let f = Fixture::new(1_000);
    let successor = Address::generate(&f.env);

    assert_eq!(f.policy_client().admin(), Some(f.admin.clone()));
    f.policy_client().transfer_admin(&successor);
    assert_eq!(f.policy_client().admin(), Some(successor.clone()));

    // Everything the successor writes lands under the new admin, and rule
    // authoring keeps working from there.
    let rule = f
        .policy_client()
        .set_rule(&f.resource(), &f.action(), &Effect::Allow);
    assert_eq!(rule.id, 1);
}
