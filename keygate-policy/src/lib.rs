//! keygate-policy: an allow/deny layer in front of a keygate-core registry.
//!
//! Separating these two concerns answers two different questions. The registry
//! asks "does this holder have the capability?" and the policy asks "do we want
//! this capability to be usable at all right now?". A deny rule takes an action
//! out of service immediately without having to revoke hundreds of grants, and
//! deleting the rule hands the capability back to whatever the registry says.
//!
//! Evaluation order:
//! 1. A rule matching `(resource, action)` wins outright. A `Deny` short-circuits
//!    without calling the registry at all.
//! 2. Otherwise the default effect applies. It is `Deny` unless an admin flips
//!    it, so an unconfigured policy is closed.
//! 3. An `Allow` (from a rule or the default) still requires a live capability,
//!    which is checked by calling into the registry.

#![no_std]

use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, symbol_short, Address, Env, IntoVal,
    String, Val, Vec,
};

#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Effect {
    Allow,
    Deny,
}

/// Identifies a rule. Used as a storage key so lookup is a map read.
#[contracttype]
#[derive(Clone)]
pub struct RuleKey {
    pub resource: String,
    pub action: String,
}

#[contracttype]
#[derive(Clone)]
pub struct Rule {
    pub id: u64,
    pub resource: String,
    pub action: String,
    pub effect: Effect,
}

#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    Admin,
    Registry,
    DefaultEffect,
    NextId,
    LiveRuleIds,
    Rule(u64),
    RuleLookup(RuleKey),
}

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum Error {
    AlreadyInitialized = 1,
    NotInitialized = 2,
    RuleNotFound = 3,
    EmptyName = 4,
}

#[contract]
pub struct KeygatePolicy;

#[contractimpl]
impl KeygatePolicy {
    /// `registry` is the keygate-core contract consulted for capabilities.
    pub fn initialize(env: Env, admin: Address, registry: Address) -> Result<(), Error> {
        if env.storage().persistent().has(&DataKey::Admin) {
            return Err(Error::AlreadyInitialized);
        }
        admin.require_auth();
        env.storage().persistent().set(&DataKey::Admin, &admin);
        env.storage().persistent().set(&DataKey::Registry, &registry);
        // Closed by default: an action nobody wrote a rule for is denied.
        env.storage()
            .persistent()
            .set(&DataKey::DefaultEffect, &Effect::Deny);
        env.events()
            .publish((symbol_short!("pol_init"),), registry);
        Ok(())
    }

    /// Creates or overwrites the rule for `(resource, action)`.
    pub fn set_rule(
        env: Env,
        resource: String,
        action: String,
        effect: Effect,
    ) -> Result<Rule, Error> {
        let admin = Self::require_admin(&env)?;
        if resource.is_empty() || action.is_empty() {
            return Err(Error::EmptyName);
        }

        let key = RuleKey {
            resource: resource.clone(),
            action: action.clone(),
        };
        let existing: Option<u64> = env.storage().persistent().get(&DataKey::RuleLookup(key.clone()));
        let id: u64 = match existing {
            Some(id) => id,
            None => {
                let next: u64 = env
                    .storage()
                    .persistent()
                    .get(&DataKey::NextId)
                    .unwrap_or(1);
                env.storage().persistent().set(&DataKey::NextId, &(next + 1));
                let mut ids: Vec<u64> = env
                    .storage()
                    .persistent()
                    .get(&DataKey::LiveRuleIds)
                    .unwrap_or(Vec::new(&env));
                ids.push_back(next);
                env.storage().persistent().set(&DataKey::LiveRuleIds, &ids);
                next
            }
        };

        let rule = Rule {
            id,
            resource,
            action,
            effect,
        };
        env.storage().persistent().set(&DataKey::Rule(id), &rule);
        env.storage().persistent().set(&DataKey::RuleLookup(key), &id);
        // The payload carries a plain bool rather than the `Effect` enum: a
        // C-like contracttype enum is encoded as a u32 discriminant, and an
        // indexer that has to guess that mapping is an indexer that gets
        // allow/deny inverted one refactor later.
        env.events().publish(
            (symbol_short!("rule_set"), admin),
            (rule.resource.clone(), rule.action.clone(), rule.effect == Effect::Allow),
        );
        Ok(rule)
    }

    pub fn remove_rule(env: Env, resource: String, action: String) -> Result<u64, Error> {
        let admin = Self::require_admin(&env)?;
        let key = RuleKey {
            resource,
            action,
        };
        let id: u64 = env
            .storage()
            .persistent()
            .get(&DataKey::RuleLookup(key.clone()))
            .ok_or(Error::RuleNotFound)?;
        let rule: Rule = env
            .storage()
            .persistent()
            .get(&DataKey::Rule(id))
            .ok_or(Error::RuleNotFound)?;
        env.storage().persistent().remove(&DataKey::Rule(id));
        env.storage().persistent().remove(&DataKey::RuleLookup(key));

        let mut kept = Vec::new(&env);
        let ids: Vec<u64> = env
            .storage()
            .persistent()
            .get(&DataKey::LiveRuleIds)
            .unwrap_or(Vec::new(&env));
        for candidate in ids.iter() {
            if candidate != id {
                kept.push_back(candidate);
            }
        }
        env.storage().persistent().set(&DataKey::LiveRuleIds, &kept);

        env.events().publish(
            (symbol_short!("rule_del"), admin),
            (rule.resource, rule.action),
        );
        Ok(id)
    }

    pub fn rule(env: Env, resource: String, action: String) -> Option<Rule> {
        let key = RuleKey {
            resource,
            action,
        };
        let id: u64 = env.storage().persistent().get(&DataKey::RuleLookup(key))?;
        env.storage().persistent().get(&DataKey::Rule(id))
    }

    /// Every configured rule, in insertion order.
    pub fn rules(env: Env) -> Vec<Rule> {
        let mut out = Vec::new(&env);
        let ids: Vec<u64> = env
            .storage()
            .persistent()
            .get(&DataKey::LiveRuleIds)
            .unwrap_or(Vec::new(&env));
        for id in ids.iter() {
            if let Some(rule) = env.storage().persistent().get(&DataKey::Rule(id)) {
                out.push_back(rule);
            }
        }
        out
    }

    pub fn default_effect(env: Env) -> Effect {
        let effect: Option<Effect> = env.storage().persistent().get(&DataKey::DefaultEffect);
        effect.unwrap_or(Effect::Deny)
    }

    /// Flips the answer for everything without a matching rule. Opening this is
    /// a real privilege change, so it is emitted as an event for the indexer.
    pub fn set_default_effect(env: Env, effect: Effect) -> Result<(), Error> {
        let admin = Self::require_admin(&env)?;
        env.storage().persistent().set(&DataKey::DefaultEffect, &effect);
        env.events().publish(
            (symbol_short!("default"), admin),
            effect == Effect::Allow,
        );
        Ok(())
    }

    pub fn registry(env: Env) -> Option<Address> {
        env.storage().persistent().get(&DataKey::Registry)
    }

    /// Repoints the policy at a different registry, e.g. after an upgrade.
    pub fn set_registry(env: Env, registry: Address) -> Result<(), Error> {
        let admin = Self::require_admin(&env)?;
        env.storage().persistent().set(&DataKey::Registry, &registry);
        env.events().publish((symbol_short!("pol_reg"), admin), registry);
        Ok(())
    }

    /// The answer callers should act on: is `holder` allowed to perform
    /// `action` on `resource` right now?
    pub fn evaluate(env: Env, resource: String, action: String, holder: Address) -> bool {
        let effect = match Self::rule_ref(&env, &resource, &action) {
            Some(rule) => rule.effect,
            None => Self::default_effect_ref(&env),
        };
        if effect == Effect::Deny {
            return false;
        }

        let registry: Address = match env.storage().persistent().get(&DataKey::Registry) {
            Some(address) => address,
            // No registry to consult means no capability can be proven.
            None => return false,
        };

        let mut args: Vec<Val> = Vec::new(&env);
        args.push_back(resource.into_val(&env));
        args.push_back(action.into_val(&env));
        args.push_back(holder.into_val(&env));
        env.invoke_contract::<bool>(&registry, &symbol_short!("check"), args)
    }

    pub fn admin(env: Env) -> Option<Address> {
        env.storage().persistent().get(&DataKey::Admin)
    }

    /// Immediate handover. Both the outgoing and incoming admin must sign, so
    /// a single compromised key cannot move control on its own.
    pub fn transfer_admin(env: Env, next: Address) -> Result<(), Error> {
        let admin = Self::require_admin(&env)?;
        next.require_auth();
        env.storage().persistent().set(&DataKey::Admin, &next);
        env.events()
            .publish((symbol_short!("pol_adm"), admin), next);
        Ok(())
    }

    fn rule_ref(env: &Env, resource: &String, action: &String) -> Option<Rule> {
        let key = RuleKey {
            resource: resource.clone(),
            action: action.clone(),
        };
        let id: u64 = env.storage().persistent().get(&DataKey::RuleLookup(key))?;
        env.storage().persistent().get(&DataKey::Rule(id))
    }

    fn default_effect_ref(env: &Env) -> Effect {
        let effect: Option<Effect> = env.storage().persistent().get(&DataKey::DefaultEffect);
        effect.unwrap_or(Effect::Deny)
    }

    fn require_admin(env: &Env) -> Result<Address, Error> {
        let admin: Address = env
            .storage()
            .persistent()
            .get(&DataKey::Admin)
            .ok_or(Error::NotInitialized)?;
        admin.require_auth();
        Ok(admin)
    }
}

#[cfg(test)]
mod tests;
