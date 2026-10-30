//! keygate-circuit-breaker: rolling-window limits that trip themselves.
//!
//! A guard answers one question: has this resource spent more than it should
//! inside the current window? It is deliberately not authorised on its own.
//! A caller that has already been authenticated by keygate-core composes the
//! two checks as `core.require_access(...)` then `breaker.enforce(...)`, and the
//! breaker stays usable from plain contracts too.
//!
//! Two properties are worth stating explicitly because they are the reason this
//! exists rather than a `require(amount <= limit)` line in a business contract:
//!
//! - **A breach trips.** Exceeding the limit does not merely reject that one
//!   call, it closes the resource until an admin resets it. A single exploit
//!   cannot keep draining in small steps.
//! - **Windows roll forward lazily.** There is no cron job: the window start is
//!   rewritten on first use after expiry, so an idle resource costs nothing.
//!
//! The first property shapes the API. A Soroban call that returns an error has
//! its own ledger writes discarded, so a breaker that answered a breach with
//! `Err` could record the latch and the rejection, but never both. `enforce`
//! therefore reports the breach as an [`Outcome`] value and keeps the latch,
//! and the caller decides what to do with it. A caller that reverts on
//! `Outcome::Breached` reverts the latch too — that is the network's rule, not
//! this contract's, and it is why the guarded call should be skipped rather
//! than failed. Every attempt *after* the latch is a real error
//! (`Error::Tripped`), because by then the rejection has nothing to record.

#![no_std]

use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, symbol_short, Address, Env, String, Vec,
};

/// The configured limit for a resource.
#[contracttype]
#[derive(Clone)]
pub struct Config {
    /// Maximum total amount admitted per window.
    pub limit: i128,
    /// Window length in ledger seconds.
    pub window: u64,
}

/// What happened to a charge. A unit enum so the caller can compare it against
/// `Outcome::Breached` without decoding anything.
#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Outcome {
    /// `amount` was admitted and the window total moved.
    Charged,
    /// `amount` would have crossed the limit. The resource is now latched and
    /// the caller must not carry out the guarded action.
    Breached,
}

/// Observed usage, plus the latch set when the limit is exceeded.
#[contracttype]
#[derive(Clone)]
pub struct State {
    pub window_start: u64,
    pub used: i128,
    pub tripped: bool,
}

#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    Admin,
    Config(String),
    State(String),
    Resources,
}

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum Error {
    AlreadyInitialized = 1,
    NotInitialized = 2,
    NotConfigured = 3,
    Tripped = 4,
    InvalidLimit = 5,
    InvalidWindow = 6,
    InvalidAmount = 7,
}

#[contract]
pub struct KeygateBreaker;

#[contractimpl]
impl KeygateBreaker {
    pub fn initialize(env: Env, admin: Address) -> Result<(), Error> {
        if env.storage().persistent().has(&DataKey::Admin) {
            return Err(Error::AlreadyInitialized);
        }
        admin.require_auth();
        env.storage().persistent().set(&DataKey::Admin, &admin);
        env.events().publish((symbol_short!("init"),), admin);
        Ok(())
    }

    /// Creates or replaces the limit for `resource`. Replacing a limit does not
    /// forgive a trip: the latch is only cleared by an explicit `reset`.
    pub fn configure(
        env: Env,
        resource: String,
        limit: i128,
        window: u64,
    ) -> Result<Config, Error> {
        let admin = Self::require_admin(&env)?;
        if limit <= 0 {
            return Err(Error::InvalidLimit);
        }
        if window == 0 {
            return Err(Error::InvalidWindow);
        }

        let config = Config { limit, window };
        let existed: bool = env
            .storage()
            .persistent()
            .has(&DataKey::Config(resource.clone()));
        env.storage()
            .persistent()
            .set(&DataKey::Config(resource.clone()), &config);
        if !existed {
            let mut resources: Vec<String> = env
                .storage()
                .persistent()
                .get(&DataKey::Resources)
                .unwrap_or(Vec::new(&env));
            resources.push_back(resource.clone());
            env.storage()
                .persistent()
                .set(&DataKey::Resources, &resources);
        }
        env.events().publish(
            (symbol_short!("cb_cfg"), admin),
            (resource, config.limit, config.window),
        );
        Ok(config)
    }

    /// Charges `amount` against `resource`'s current window.
    ///
    /// Fails closed on everything that is the caller's fault to fix alone: a
    /// missing config, a tripped latch, a non-positive amount. A limit that the
    /// amount would cross is not an error — it is `Outcome::Breached`, which
    /// latches the resource and leaves the caller to skip the guarded action.
    pub fn enforce(env: Env, resource: String, amount: i128) -> Result<Outcome, Error> {
        if amount <= 0 {
            return Err(Error::InvalidAmount);
        }
        let config: Config = env
            .storage()
            .persistent()
            .get(&DataKey::Config(resource.clone()))
            .ok_or(Error::NotConfigured)?;
        let mut state = Self::state_at(&env, &resource, &config);
        if state.tripped {
            return Err(Error::Tripped);
        }

        let projected = state.used + amount;
        if projected > config.limit {
            state.tripped = true;
            // Record the offending total so a reset reviewer can see how far
            // past the limit usage ran.
            state.used = projected;
            env.storage()
                .persistent()
                .set(&DataKey::State(resource.clone()), &state);
            // `cb_trip` is only ever an automatic breach; an operator-initiated
            // shutdown uses `cb_force`. Keeping them on separate topics means an
            // indexer never has to infer who caused the latch from the numbers.
            env.events().publish(
                (symbol_short!("cb_trip"),),
                (resource, config.limit, projected),
            );
            return Ok(Outcome::Breached);
        }

        state.used = projected;
        env.storage()
            .persistent()
            .set(&DataKey::State(resource.clone()), &state);
        // The successful charge is published too, not just the breach. Without
        // it an off-chain mirror could see limits and trips but never usage, so
        // `remaining` would be unknowable until something failed.
        env.events().publish(
            (symbol_short!("cb_use"),),
            (resource.clone(), state.window_start, state.used),
        );
        Ok(Outcome::Charged)
    }

    /// How much of the window is still available. Read-only, and rolls the
    /// window forward in the same way `enforce` does without persisting it.
    pub fn remaining(env: Env, resource: String) -> Result<i128, Error> {
        let config: Config = env
            .storage()
            .persistent()
            .get(&DataKey::Config(resource.clone()))
            .ok_or(Error::NotConfigured)?;
        let state = Self::state_at(&env, &resource, &config);
        if state.tripped {
            return Ok(0);
        }
        Ok(config.limit - state.used)
    }

    pub fn config(env: Env, resource: String) -> Option<Config> {
        env.storage().persistent().get(&DataKey::Config(resource))
    }

    /// The persisted state, without rolling the window forward.
    pub fn state(env: Env, resource: String) -> Option<State> {
        env.storage().persistent().get(&DataKey::State(resource))
    }

    pub fn is_tripped(env: Env, resource: String) -> bool {
        let state: Option<State> = env.storage().persistent().get(&DataKey::State(resource));
        matches!(state, Some(State { tripped: true, .. }))
    }

    /// Closes the latch without a breach. Use it when off-chain signals (an
    /// oracle, an incident, a withdrawn audit) say to stop before limits are hit.
    pub fn trip(env: Env, resource: String) -> Result<(), Error> {
        let admin = Self::require_admin(&env)?;
        let config: Config = env
            .storage()
            .persistent()
            .get(&DataKey::Config(resource.clone()))
            .ok_or(Error::NotConfigured)?;
        let mut state = Self::state_at(&env, &resource, &config);
        state.tripped = true;
        env.storage()
            .persistent()
            .set(&DataKey::State(resource.clone()), &state);
        env.events()
            .publish((symbol_short!("cb_force"), admin), resource);
        Ok(())
    }

    /// Clears the latch and starts a fresh window from now.
    pub fn reset(env: Env, resource: String) -> Result<(), Error> {
        let admin = Self::require_admin(&env)?;
        let state = State {
            window_start: env.ledger().timestamp(),
            used: 0,
            tripped: false,
        };
        env.storage()
            .persistent()
            .set(&DataKey::State(resource.clone()), &state);
        env.events()
            .publish((symbol_short!("cb_reset"), admin), resource);
        Ok(())
    }

    pub fn resources(env: Env) -> Vec<String> {
        let list: Option<Vec<String>> = env.storage().persistent().get(&DataKey::Resources);
        list.unwrap_or(Vec::new(&env))
    }

    pub fn admin(env: Env) -> Option<Address> {
        env.storage().persistent().get(&DataKey::Admin)
    }

    /// Effective state at the current timestamp, starting a new window when the
    /// stored one has run out. Not persisted, so views stay side-effect free.
    fn state_at(env: &Env, resource: &String, config: &Config) -> State {
        let now = env.ledger().timestamp();
        let stored: Option<State> = env
            .storage()
            .persistent()
            .get(&DataKey::State(resource.clone()));
        match stored {
            None => State {
                window_start: now,
                used: 0,
                tripped: false,
            },
            Some(state) => {
                // A tripped latch survives window rollover; only `reset` clears it.
                if !state.tripped && now >= state.window_start + config.window {
                    State {
                        window_start: now,
                        used: 0,
                        tripped: false,
                    }
                } else {
                    state
                }
            }
        }
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
