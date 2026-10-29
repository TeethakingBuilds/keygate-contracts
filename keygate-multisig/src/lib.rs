//! keygate-multisig: threshold administration for the rest of keygate.
//!
//! The registry, policy and breaker all have a single admin address. That is
//! fine for a user-facing capability, but it means one key can freeze issuance,
//! flip a policy default to `Allow`, or repoint a policy at a hostile registry.
//! This contract puts those actions behind m-of-n approval by holding the admin
//! address itself: propose the call, gather approvals, execute it, and the
//! target sees the multisig as the caller.
//!
//! Proposals carry an already-encoded argument list (`Vec<Val>`), so the
//! multisig can relay any call without keygate needing to know every target's
//! signature at compile time.

#![no_std]

use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, symbol_short, Address, Env, Symbol, Val,
    Vec,
};

#[contracttype]
#[derive(Clone)]
pub struct Proposal {
    pub id: u64,
    pub target: Address,
    pub func: Symbol,
    pub args: Vec<Val>,
    /// Owners who have approved. The proposer is included at submission.
    pub approvals: Vec<Address>,
    pub executed: bool,
    pub proposed_at: u64,
}

#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    Owners,
    Threshold,
    NextId,
    LiveIds,
    Proposal(u64),
}

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum Error {
    AlreadyInitialized = 1,
    EmptyOwners = 2,
    DuplicateOwner = 3,
    InvalidThreshold = 4,
    NotAnOwner = 5,
    ProposalNotFound = 6,
    AlreadyExecuted = 7,
    DuplicateApproval = 8,
    QuorumNotMet = 9,
}

#[contract]
pub struct KeygateMultisig;

#[contractimpl]
impl KeygateMultisig {
    /// `threshold` of `owners` must approve a proposal before it can execute.
    pub fn initialize(env: Env, owners: Vec<Address>, threshold: u32) -> Result<(), Error> {
        if env.storage().persistent().has(&DataKey::Owners) {
            return Err(Error::AlreadyInitialized);
        }
        if owners.is_empty() {
            return Err(Error::EmptyOwners);
        }
        if threshold == 0 || threshold > owners.len() {
            return Err(Error::InvalidThreshold);
        }
        let mut unique = Vec::new(&env);
        for owner in owners.iter() {
            if unique.contains(&owner) {
                return Err(Error::DuplicateOwner);
            }
            unique.push_back(owner);
        }
        env.storage().persistent().set(&DataKey::Owners, &unique);
        env.storage()
            .persistent()
            .set(&DataKey::Threshold, &threshold);
        env.events()
            .publish((symbol_short!("ms_init"),), (unique, threshold));
        Ok(())
    }

    /// Records a call to be made once quorum approves it.
    pub fn submit(
        env: Env,
        caller: Address,
        target: Address,
        func: Symbol,
        args: Vec<Val>,
    ) -> Result<u64, Error> {
        Self::require_owner(&env, &caller)?;
        let id: u64 = env
            .storage()
            .persistent()
            .get(&DataKey::NextId)
            .unwrap_or(1);
        env.storage().persistent().set(&DataKey::NextId, &(id + 1));

        let mut approvals = Vec::new(&env);
        approvals.push_back(caller.clone());
        let proposal = Proposal {
            id,
            target: target.clone(),
            func: func.clone(),
            args: args.clone(),
            approvals,
            executed: false,
            proposed_at: env.ledger().timestamp(),
        };
        env.storage()
            .persistent()
            .set(&DataKey::Proposal(id), &proposal);

        let mut ids: Vec<u64> = env
            .storage()
            .persistent()
            .get(&DataKey::LiveIds)
            .unwrap_or(Vec::new(&env));
        ids.push_back(id);
        env.storage().persistent().set(&DataKey::LiveIds, &ids);

        env.events()
            .publish((symbol_short!("submitted"), caller), proposal);
        Ok(id)
    }

    /// Adds `caller` to the proposal's approvals.
    pub fn approve(env: Env, caller: Address, id: u64) -> Result<u32, Error> {
        Self::require_owner(&env, &caller)?;
        let mut proposal = Self::load(&env, id)?;
        if proposal.executed {
            return Err(Error::AlreadyExecuted);
        }
        if proposal.approvals.contains(&caller) {
            return Err(Error::DuplicateApproval);
        }
        proposal.approvals.push_back(caller.clone());
        let count = proposal.approvals.len();
        env.storage()
            .persistent()
            .set(&DataKey::Proposal(id), &proposal);
        env.events()
            .publish((symbol_short!("approved"), caller), id);
        Ok(count)
    }

    /// Withdraws `caller`'s approval, for when a proposal is revised or an
    /// owner approves by mistake.
    pub fn unapprove(env: Env, caller: Address, id: u64) -> Result<u32, Error> {
        Self::require_owner(&env, &caller)?;
        let mut proposal = Self::load(&env, id)?;
        if proposal.executed {
            return Err(Error::AlreadyExecuted);
        }
        if !proposal.approvals.contains(&caller) {
            return Err(Error::DuplicateApproval);
        }
        let mut kept = Vec::new(&env);
        for approver in proposal.approvals.iter() {
            if approver != caller {
                kept.push_back(approver);
            }
        }
        proposal.approvals = kept;
        let count = proposal.approvals.len();
        env.storage()
            .persistent()
            .set(&DataKey::Proposal(id), &proposal);
        env.events()
            .publish((symbol_short!("unapprove"), caller), id);
        Ok(count)
    }

    /// Drops a proposal that has not run.
    pub fn cancel(env: Env, caller: Address, id: u64) -> Result<(), Error> {
        Self::require_owner(&env, &caller)?;
        let proposal = Self::load(&env, id)?;
        if proposal.executed {
            return Err(Error::AlreadyExecuted);
        }
        env.storage().persistent().remove(&DataKey::Proposal(id));
        let mut kept = Vec::new(&env);
        let ids: Vec<u64> = env
            .storage()
            .persistent()
            .get(&DataKey::LiveIds)
            .unwrap_or(Vec::new(&env));
        for candidate in ids.iter() {
            if candidate != id {
                kept.push_back(candidate);
            }
        }
        env.storage().persistent().set(&DataKey::LiveIds, &kept);
        env.events()
            .publish((symbol_short!("cancelled"), caller), id);
        Ok(())
    }

    /// Makes the recorded call. The target authorises this by treating the
    /// multisig's own address as the caller, so keygate contracts should be
    /// administered through here rather than by handing a key to one person.
    pub fn execute(env: Env, caller: Address, id: u64) -> Result<(), Error> {
        Self::require_owner(&env, &caller)?;
        let proposal = Self::load(&env, id)?;
        if proposal.executed {
            return Err(Error::AlreadyExecuted);
        }
        let threshold: u32 = env
            .storage()
            .persistent()
            .get(&DataKey::Threshold)
            .ok_or(Error::EmptyOwners)?;
        if proposal.approvals.len() < threshold {
            return Err(Error::QuorumNotMet);
        }

        env.invoke_contract::<()>(&proposal.target, &proposal.func, proposal.args.clone());

        let mut done = proposal.clone();
        done.executed = true;
        env.storage()
            .persistent()
            .set(&DataKey::Proposal(id), &done);

        // Executed proposals stay readable by id but leave the pending set, so
        // an indexer's cursor over `live_ids` only ever holds open work.
        let mut kept = Vec::new(&env);
        let ids: Vec<u64> = env
            .storage()
            .persistent()
            .get(&DataKey::LiveIds)
            .unwrap_or(Vec::new(&env));
        for candidate in ids.iter() {
            if candidate != id {
                kept.push_back(candidate);
            }
        }
        env.storage().persistent().set(&DataKey::LiveIds, &kept);

        env.events()
            .publish((symbol_short!("executed"), caller), id);
        Ok(())
    }

    /// Rotates the owner set. Reachable only through a proposal that this
    /// contract executes, which is what makes the guard a quorum check rather
    /// than a signature check.
    pub fn set_owners(env: Env, owners: Vec<Address>, threshold: u32) -> Result<(), Error> {
        let myself = env.current_contract_address();
        myself.require_auth();
        if owners.is_empty() {
            return Err(Error::EmptyOwners);
        }
        if threshold == 0 || threshold > owners.len() {
            return Err(Error::InvalidThreshold);
        }
        let mut unique = Vec::new(&env);
        for owner in owners.iter() {
            if unique.contains(&owner) {
                return Err(Error::DuplicateOwner);
            }
            unique.push_back(owner);
        }
        env.storage().persistent().set(&DataKey::Owners, &unique);
        env.storage()
            .persistent()
            .set(&DataKey::Threshold, &threshold);
        env.events()
            .publish((symbol_short!("ms_owners"),), (unique, threshold));
        Ok(())
    }

    pub fn proposal(env: Env, id: u64) -> Option<Proposal> {
        env.storage().persistent().get(&DataKey::Proposal(id))
    }

    /// Open proposal ids, in submission order.
    pub fn live_ids(env: Env) -> Vec<u64> {
        let ids: Option<Vec<u64>> = env.storage().persistent().get(&DataKey::LiveIds);
        ids.unwrap_or(Vec::new(&env))
    }

    pub fn approvals(env: Env, id: u64) -> Vec<Address> {
        match env.storage().persistent().get(&DataKey::Proposal(id)) {
            Some(proposal) => proposal.approvals,
            None => Vec::new(&env),
        }
    }

    pub fn owners(env: Env) -> Vec<Address> {
        let owners: Option<Vec<Address>> = env.storage().persistent().get(&DataKey::Owners);
        owners.unwrap_or(Vec::new(&env))
    }

    pub fn threshold(env: Env) -> u32 {
        let value: Option<u32> = env.storage().persistent().get(&DataKey::Threshold);
        value.unwrap_or(0)
    }

    pub fn is_owner(env: Env, address: Address) -> bool {
        Self::owners(&env).contains(&address)
    }

    fn load(env: &Env, id: u64) -> Result<Proposal, Error> {
        env.storage()
            .persistent()
            .get(&DataKey::Proposal(id))
            .ok_or(Error::ProposalNotFound)
    }

    /// Owner-set membership is checked here; the signature itself is enforced
    /// by the host when `caller.require_auth()` runs.
    fn require_owner(env: &Env, caller: &Address) -> Result<(), Error> {
        caller.require_auth();
        if !Self::owners(env).contains(caller) {
            return Err(Error::NotAnOwner);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
