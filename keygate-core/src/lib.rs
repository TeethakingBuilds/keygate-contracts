//! keygate-core: a capability registry for Soroban contracts.
//!
//! A *capability* is a triple `(resource, action, holder)` with an expiry.
//! Callers ask the registry whether a holder may perform an action on a
//! resource instead of hard-coding role checks inside every business contract.
//!
//! Storage layout:
//! - `DataKey::Admin` / `PendingAdmin` — two-step handover of ownership.
//! - `DataKey::Paused` — freeze switch for issuance and revocation.
//! - `DataKey::NextId` — monotonic grant id source.
//! - `DataKey::LiveIds` — every live grant id, for enumeration and cleanup.
//! - `DataKey::Grant(u64)` — the grant record itself.
//! - `DataKey::Lookup(GrantKey)` — id of the live grant for a triple, which is
//!   what makes an access check a single storage read.

#![no_std]

use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, symbol_short, Address, Env, String, Vec,
};

/// The address-space identity of a live grant. Used as a storage key so that
/// lookup is a hash-map read rather than a scan.
#[contracttype]
#[derive(Clone)]
pub struct GrantKey {
    pub resource: String,
    pub action: String,
    pub holder: Address,
}

/// A recorded capability. Revoked and expired grants are removed rather than
/// flagged, so the absence of a record is the only denial state to reason about.
#[contracttype]
#[derive(Clone)]
pub struct Grant {
    pub id: u64,
    pub resource: String,
    pub action: String,
    pub holder: Address,
    pub issuer: Address,
    /// Ledger timestamp after which the grant stops being valid.
    pub expires_at: u64,
}

#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    Admin,
    PendingAdmin,
    Paused,
    NextId,
    LiveIds,
    Grant(u64),
    Lookup(GrantKey),
}

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum Error {
    AlreadyInitialized = 1,
    NotInitialized = 2,
    Paused = 3,
    GrantNotFound = 4,
    ZeroTtl = 5,
    NoPendingAdmin = 6,
}

#[contract]
pub struct KeygateCore;

#[contractimpl]
impl KeygateCore {
    /// Records the initial admin. Only callable once, and only with the
    /// admin's own authorisation.
    pub fn initialize(env: Env, admin: Address) -> Result<(), Error> {
        if env.storage().persistent().has(&DataKey::Admin) {
            return Err(Error::AlreadyInitialized);
        }
        admin.require_auth();
        env.storage().persistent().set(&DataKey::Admin, &admin);
        env.events().publish((symbol_short!("init"),), admin);
        Ok(())
    }

    /// Issues `(resource, action) -> holder` for `ttl` seconds from now.
    ///
    /// Re-issuing a live triple keeps the grant id and overwrites the expiry,
    /// so issuing doubles as the renewal path.
    pub fn issue(
        env: Env,
        resource: String,
        action: String,
        holder: Address,
        ttl: u64,
    ) -> Result<Grant, Error> {
        let admin = Self::require_admin(&env)?;
        Self::require_unpaused(&env)?;
        if ttl == 0 {
            return Err(Error::ZeroTtl);
        }

        let key = GrantKey {
            resource: resource.clone(),
            action: action.clone(),
            holder: holder.clone(),
        };
        let existing: Option<u64> = env.storage().persistent().get(&DataKey::Lookup(key.clone()));
        let is_new = existing.is_none();

        let id: u64 = match existing {
            Some(id) => id,
            None => {
                let next: u64 = env
                    .storage()
                    .persistent()
                    .get(&DataKey::NextId)
                    .unwrap_or(1);
                env.storage().persistent().set(&DataKey::NextId, &(next + 1));
                next
            }
        };

        let grant = Grant {
            id,
            resource,
            action,
            holder: holder.clone(),
            issuer: admin,
            expires_at: env.ledger().timestamp() + ttl,
        };
        env.storage().persistent().set(&DataKey::Grant(id), &grant);
        env.storage().persistent().set(&DataKey::Lookup(key), &id);

        if is_new {
            let mut ids: Vec<u64> = env
                .storage()
                .persistent()
                .get(&DataKey::LiveIds)
                .unwrap_or(Vec::new(&env));
            ids.push_back(id);
            env.storage().persistent().set(&DataKey::LiveIds, &ids);
            env.events()
                .publish((symbol_short!("issued"), holder), grant.clone());
        } else {
            env.events()
                .publish((symbol_short!("renewed"), holder), grant.clone());
        }
        Ok(grant)
    }

    /// Removes a live grant and returns the id that was dropped.
    pub fn revoke(
        env: Env,
        resource: String,
        action: String,
        holder: Address,
    ) -> Result<u64, Error> {
        Self::require_admin(&env)?;
        Self::require_unpaused(&env)?;
        let key = GrantKey {
            resource,
            action,
            holder: holder.clone(),
        };
        let id: u64 = env
            .storage()
            .persistent()
            .get(&DataKey::Lookup(key.clone()))
            .ok_or(Error::GrantNotFound)?;
        // Carry the whole grant, not just its id: an indexer removing a
        // capability needs the triple, and should not have to remember it from
        // the issue event.
        let grant: Grant = env
            .storage()
            .persistent()
            .get(&DataKey::Grant(id))
            .ok_or(Error::GrantNotFound)?;
        Self::drop_grant(&env, id, &key);
        env.events()
            .publish((symbol_short!("revoked"), holder), grant.clone());
        Ok(id)
    }

    /// True when `holder` currently holds a live, unexpired grant.
    ///
    /// An expiry that has passed is treated as absent and the record is dropped
    /// there and then, so stale grants cannot accumulate in the live set.
    pub fn check(env: Env, resource: String, action: String, holder: Address) -> bool {
        let key = GrantKey {
            resource,
            action,
            holder,
        };
        let id: u64 = match env.storage().persistent().get(&DataKey::Lookup(key.clone())) {
            Some(id) => id,
            None => return false,
        };
        let grant: Grant = match env.storage().persistent().get(&DataKey::Grant(id)) {
            Some(grant) => grant,
            None => return false,
        };
        if grant.expires_at <= env.ledger().timestamp() {
            Self::drop_grant(&env, id, &key);
            env.events()
                .publish((symbol_short!("expired"), grant.holder.clone()), grant.clone());
            false
        } else {
            true
        }
    }

    /// Authorising entry point for other contracts: authenticates `caller` and
    /// fails closed when the capability is missing.
    pub fn require_access(
        env: Env,
        caller: Address,
        resource: String,
        action: String,
    ) -> Result<(), Error> {
        caller.require_auth();
        if !Self::check(env.clone(), resource, action, caller) {
            return Err(Error::GrantNotFound);
        }
        Ok(())
    }

    /// The live grant for a triple, if any.
    pub fn grant(env: Env, resource: String, action: String, holder: Address) -> Option<Grant> {
        let key = GrantKey {
            resource,
            action,
            holder,
        };
        Self::live_grant(&env, &key)
    }

    /// Every live grant id. Intended for administrative tooling and for the
    /// off-chain indexer, which reconciles against this list.
    pub fn live_ids(env: Env) -> Vec<u64> {
        Self::live_ids_ref(&env)
    }

    /// Grants held by `holder`, in issue order.
    pub fn grants_of(env: Env, holder: Address) -> Vec<Grant> {
        let mut out = Vec::new(&env);
        let now = env.ledger().timestamp();
        for id in Self::live_ids_ref(&env).iter() {
            let maybe: Option<Grant> = env.storage().persistent().get(&DataKey::Grant(id));
            if let Some(grant) = maybe {
                if grant.holder == holder && grant.expires_at > now {
                    out.push_back(grant);
                }
            }
        }
        out
    }

    pub fn admin(env: Env) -> Option<Address> {
        env.storage().persistent().get(&DataKey::Admin)
    }

    /// Stage a successor admin. The current admin keeps power until the nominee
    /// accepts, so a mistyped address cannot strand the registry.
    pub fn propose_admin(env: Env, next: Address) -> Result<(), Error> {
        let admin = Self::require_admin(&env)?;
        env.storage().persistent().set(&DataKey::PendingAdmin, &next);
        env.events().publish((symbol_short!("prop_adm"), admin), next);
        Ok(())
    }

    /// Complete a staged handover. Callable only by the nominee.
    pub fn accept_admin(env: Env) -> Result<Address, Error> {
        let pending: Address = env
            .storage()
            .persistent()
            .get(&DataKey::PendingAdmin)
            .ok_or(Error::NoPendingAdmin)?;
        pending.require_auth();
        env.storage().persistent().set(&DataKey::Admin, &pending);
        env.storage().persistent().remove(&DataKey::PendingAdmin);
        env.events().publish((symbol_short!("admin"),), pending.clone());
        Ok(pending)
    }

    pub fn cancel_admin(env: Env) -> Result<(), Error> {
        Self::require_admin(&env)?;
        env.storage().persistent().remove(&DataKey::PendingAdmin);
        Ok(())
    }

    /// Freezes issuance, renewal and revocation. Checks keep working: a frozen
    /// registry still answers questions, it just stops accepting changes.
    pub fn pause(env: Env) -> Result<(), Error> {
        Self::require_admin(&env)?;
        env.storage().persistent().set(&DataKey::Paused, &true);
        env.events().publish((symbol_short!("paused"),), ());
        Ok(())
    }

    pub fn unpause(env: Env) -> Result<(), Error> {
        Self::require_admin(&env)?;
        env.storage().persistent().set(&DataKey::Paused, &false);
        env.events().publish((symbol_short!("unpause"),), ());
        Ok(())
    }

    pub fn paused(env: Env) -> bool {
        Self::is_paused(&env)
    }

    fn is_paused(env: &Env) -> bool {
        let flag: Option<bool> = env.storage().persistent().get(&DataKey::Paused);
        matches!(flag, Some(true))
    }

    fn require_unpaused(env: &Env) -> Result<(), Error> {
        if Self::is_paused(env) {
            return Err(Error::Paused);
        }
        Ok(())
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

    fn live_ids_ref(env: &Env) -> Vec<u64> {
        let ids: Option<Vec<u64>> = env.storage().persistent().get(&DataKey::LiveIds);
        ids.unwrap_or(Vec::new(env))
    }

    fn live_grant(env: &Env, key: &GrantKey) -> Option<Grant> {
        let id: u64 = env.storage().persistent().get(&DataKey::Lookup(key.clone()))?;
        let grant: Grant = env.storage().persistent().get(&DataKey::Grant(id))?;
        if grant.expires_at > env.ledger().timestamp() {
            Some(grant)
        } else {
            None
        }
    }

    fn drop_grant(env: &Env, id: u64, key: &GrantKey) {
        env.storage().persistent().remove(&DataKey::Grant(id));
        env.storage().persistent().remove(&DataKey::Lookup(key.clone()));
        let mut kept = Vec::new(env);
        for candidate in Self::live_ids_ref(env).iter() {
            if candidate != id {
                kept.push_back(candidate);
            }
        }
        env.storage().persistent().set(&DataKey::LiveIds, &kept);
    }
}

#[cfg(test)]
mod tests;
