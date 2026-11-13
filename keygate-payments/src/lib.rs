//! keygate-payments: non-custodial, policy-guarded USDC payments and burns.
//!
//! Funds never sit in this contract. A direct payment moves USDC from payer to
//! recipient through the official USDC Stellar Asset Contract (SAC). A bridge
//! payment invokes Circle's official CCTP V2 TokenMessengerMinter. In both
//! cases the payer signs the complete invocation and this contract atomically
//! checks and consumes the payer's spending policy first.
//!
//! Daily limits use UTC ledger-day buckets. Monthly limits use fixed 30-day UTC
//! ledger-time buckets because Soroban exposes epoch seconds, not a calendar.
//! These are intentionally not described as rolling windows: spending near a
//! bucket boundary can use capacity from each adjacent bucket.
//!
//! A classic account can still call the SAC directly. Keygate governs payments
//! routed through this contract; it cannot bind unrelated account activity.

#![no_std]

use soroban_sdk::{
    contract, contractclient, contracterror, contractimpl, contracttype, panic_with_error,
    symbol_short, token, Address, BytesN, Env, String, Vec,
};

pub const USDC_DECIMALS: u32 = 7;
pub const DAY_SECONDS: u64 = 86_400;
pub const MONTH_SECONDS: u64 = 30 * DAY_SECONDS;
pub const MAX_INTENT_LIFETIME: u64 = 7 * DAY_SECONDS;

const LEDGERS_PER_DAY: u32 = 17_280;
const LIVE_THRESHOLD: u32 = 30 * LEDGERS_PER_DAY;
const LIVE_BUMP: u32 = 365 * LEDGERS_PER_DAY;
const REPLAY_BUMP: u32 = 30 * LEDGERS_PER_DAY;

const ETHEREUM_DOMAIN: u32 = 0;
const SOLANA_DOMAIN: u32 = 5;
const BASE_DOMAIN: u32 = 6;
const STANDARD_FINALITY_THRESHOLD: u32 = 2_000;

const MAINNET_USDC: &str = "CCW67TSZV3SSS2HXMBQ5JFGCKJNXKZM7UQUWUZPUTHXSTZLEO7SJMI75";
const TESTNET_USDC: &str = "CBIELTK6YBZJU5UP2WWQEUCYKLPU6AUNZ2BQ4WWFEIE3USCIHMXQDAMA";
const MAINNET_CCTP: &str = "CAE2G5Z77UP7GYPYGFOWFGW7C7J6I4YP2AFGSADRKQY62SYUFLPNFTXL";
const TESTNET_CCTP: &str = "CDNG7HXAPBWICI2E3AUBP3YZWZELJLYSB6F5CC7WLDTLTHVM74SLRTHP";

#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StellarNetwork {
    Testnet,
    Mainnet,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SpendingPolicy {
    /// Maximum stroops (1e-7 USDC) in one UTC ledger-day bucket.
    pub daily_limit: i128,
    /// Maximum stroops in one fixed 30-day UTC ledger-time bucket.
    pub monthly_limit: i128,
    /// Payments strictly above this need `approval_threshold` co-signers.
    pub approval_above: i128,
    pub approvers: Vec<Address>,
    pub approval_threshold: u32,
    pub allowlist_only: bool,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Usage {
    pub day_bucket: u64,
    pub day_used: i128,
    pub month_bucket: u64,
    pub month_used: i128,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PaymentReceipt {
    pub reference: BytesN<32>,
    pub from: Address,
    pub to: Address,
    pub amount: i128,
    pub timestamp: u64,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BridgeIntent {
    pub destination_domain: u32,
    pub mint_recipient: BytesN<32>,
    pub destination_caller: BytesN<32>,
    pub amount: i128,
    pub max_fee: i128,
    pub min_finality_threshold: u32,
    pub reference: BytesN<32>,
    pub deadline: u64,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BridgeReceipt {
    pub reference: BytesN<32>,
    pub from: Address,
    pub destination_domain: u32,
    pub mint_recipient: BytesN<32>,
    pub amount: i128,
    pub max_fee: i128,
    pub timestamp: u64,
}

#[contracttype]
#[derive(Clone)]
pub struct AllowKey {
    pub account: Address,
    pub recipient: Address,
}

#[contracttype]
#[derive(Clone)]
pub struct BridgeKey {
    pub account: Address,
    pub destination_domain: u32,
}

#[contracttype]
#[derive(Clone)]
pub struct IntentKey {
    pub account: Address,
    pub reference: BytesN<32>,
}

#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    Admin,
    PendingAdmin,
    Network,
    Token,
    Cctp,
    Paused,
    Policy(Address),
    Usage(Address),
    Allowed(AllowKey),
    BridgeAllowed(BridgeKey),
    Payment(IntentKey),
    Bridge(IntentKey),
}

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum Error {
    Paused = 1,
    InvalidAmount = 2,
    InvalidPolicy = 3,
    NoPolicy = 4,
    RecipientNotAllowed = 5,
    BridgeRouteNotAllowed = 6,
    UnsupportedBridgeDomain = 7,
    DailyLimitExceeded = 8,
    MonthlyLimitExceeded = 9,
    DuplicateReference = 10,
    SelfPayment = 11,
    ApprovalUnavailable = 12,
    DuplicateApprover = 13,
    NotAnApprover = 14,
    InvalidToken = 15,
    InvalidCctp = 16,
    InvalidDeadline = 17,
    InvalidBridgeAmount = 18,
    InvalidDestination = 19,
    FastTransferUnsupported = 20,
    NoPendingAdmin = 21,
}

#[contractclient(name = "CctpClient")]
pub trait Cctp {
    #[allow(clippy::too_many_arguments)]
    fn deposit_for_burn(
        env: &Env,
        caller: Address,
        amount: i128,
        destination_domain: u32,
        mint_recipient: BytesN<32>,
        burn_token: Address,
        destination_caller: BytesN<32>,
        max_fee: i128,
        min_finality_threshold: u32,
    );
}

#[contract]
pub struct KeygatePayments;

#[contractimpl]
impl KeygatePayments {
    /// Atomically binds this deployment to the official network-specific USDC
    /// SAC and CCTP V2 TokenMessengerMinter; there is no uninitialized window.
    pub fn __constructor(
        env: Env,
        admin: Address,
        network: StellarNetwork,
        token: Address,
        cctp: Address,
    ) {
        admin.require_auth();
        if token != Self::official_usdc(&env, network) {
            panic_with_error!(&env, Error::InvalidToken);
        }
        if cctp != Self::official_cctp(&env, network) {
            panic_with_error!(&env, Error::InvalidCctp);
        }
        let token_client = token::TokenClient::new(&env, &token);
        if token_client.decimals() != USDC_DECIMALS
            || token_client.symbol() != String::from_str(&env, "USDC")
        {
            panic_with_error!(&env, Error::InvalidToken);
        }
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::Network, &network);
        env.storage().instance().set(&DataKey::Token, &token);
        env.storage().instance().set(&DataKey::Cctp, &cctp);
        Self::bump_instance(&env);
        env.events()
            .publish((symbol_short!("pay_init"), admin), (network, token, cctp));
    }

    /// Creates or replaces `account`'s policy. The first policy needs the new
    /// quorum to consent; later changes need the current quorum.
    pub fn set_policy(
        env: Env,
        account: Address,
        policy: SpendingPolicy,
        approvals: Vec<Address>,
    ) -> Result<(), Error> {
        account.require_auth();
        Self::validate_policy(&env, &account, &policy)?;
        match Self::policy_ref(&env, &account) {
            Some(current) => Self::require_quorum(&env, &current, &approvals)?,
            None => Self::require_quorum(&env, &policy, &approvals)?,
        }
        let key = DataKey::Policy(account.clone());
        env.storage().persistent().set(&key, &policy);
        Self::bump_live(&env, &key);
        Self::bump_instance(&env);
        env.events()
            .publish((symbol_short!("policy"), account), policy);
        Ok(())
    }

    pub fn policy(env: Env, account: Address) -> Option<SpendingPolicy> {
        Self::policy_ref(&env, &account)
    }

    /// Adding a recipient widens authority and therefore needs the current
    /// policy's quorum. Removing one only needs the account.
    pub fn allow_recipient(
        env: Env,
        account: Address,
        recipient: Address,
        approvals: Vec<Address>,
    ) -> Result<(), Error> {
        account.require_auth();
        let policy = Self::policy_ref(&env, &account).ok_or(Error::NoPolicy)?;
        Self::require_quorum(&env, &policy, &approvals)?;
        let key = DataKey::Allowed(AllowKey {
            account: account.clone(),
            recipient: recipient.clone(),
        });
        env.storage().persistent().set(&key, &true);
        Self::bump_live(&env, &key);
        Self::bump_instance(&env);
        env.events()
            .publish((symbol_short!("allow"), account), recipient);
        Ok(())
    }

    pub fn disallow_recipient(
        env: Env,
        account: Address,
        recipient: Address,
    ) -> Result<(), Error> {
        account.require_auth();
        env.storage()
            .persistent()
            .remove(&DataKey::Allowed(AllowKey {
                account: account.clone(),
                recipient: recipient.clone(),
            }));
        Self::bump_instance(&env);
        env.events()
            .publish((symbol_short!("disallow"), account), recipient);
        Ok(())
    }

    pub fn is_allowed(env: Env, account: Address, recipient: Address) -> bool {
        let key = DataKey::Allowed(AllowKey { account, recipient });
        if env.storage().persistent().has(&key) {
            Self::bump_live(&env, &key);
            true
        } else {
            false
        }
    }

    /// Enables one of Circle's verified destination domains for `account`.
    /// Stellar-origin CCTP is Standard Transfer only.
    pub fn set_bridge_route(
        env: Env,
        account: Address,
        destination_domain: u32,
        enabled: bool,
        approvals: Vec<Address>,
    ) -> Result<(), Error> {
        account.require_auth();
        Self::require_supported_domain(destination_domain)?;
        let policy = Self::policy_ref(&env, &account).ok_or(Error::NoPolicy)?;
        if enabled {
            Self::require_quorum(&env, &policy, &approvals)?;
        }
        let key = DataKey::BridgeAllowed(BridgeKey {
            account: account.clone(),
            destination_domain,
        });
        if enabled {
            env.storage().persistent().set(&key, &true);
            Self::bump_live(&env, &key);
        } else {
            env.storage().persistent().remove(&key);
        }
        Self::bump_instance(&env);
        env.events().publish(
            (symbol_short!("br_route"), account),
            (destination_domain, enabled),
        );
        Ok(())
    }

    pub fn bridge_route_allowed(env: Env, account: Address, destination_domain: u32) -> bool {
        let key = DataKey::BridgeAllowed(BridgeKey {
            account,
            destination_domain,
        });
        if env.storage().persistent().has(&key) {
            Self::bump_live(&env, &key);
            true
        } else {
            false
        }
    }

    /// Transfers USDC directly from `from` to `to` after atomically consuming
    /// the payer's limits. `reference` is unique per payer.
    pub fn pay(
        env: Env,
        from: Address,
        to: Address,
        amount: i128,
        reference: BytesN<32>,
        deadline: u64,
        approvals: Vec<Address>,
    ) -> Result<PaymentReceipt, Error> {
        Self::require_unpaused(&env)?;
        Self::validate_intent(&env, amount, deadline)?;
        if from == to {
            return Err(Error::SelfPayment);
        }
        from.require_auth();
        let intent = IntentKey {
            account: from.clone(),
            reference: reference.clone(),
        };
        let receipt_key = DataKey::Payment(intent);
        Self::require_unused(&env, &receipt_key)?;

        let policy = Self::policy_ref(&env, &from).ok_or(Error::NoPolicy)?;
        if policy.allowlist_only && !Self::is_allowed(env.clone(), from.clone(), to.clone()) {
            return Err(Error::RecipientNotAllowed);
        }
        Self::require_payment_approval(&env, &policy, amount, &approvals)?;
        Self::consume(&env, &from, &policy, amount)?;

        let receipt = PaymentReceipt {
            reference: reference.clone(),
            from: from.clone(),
            to: to.clone(),
            amount,
            timestamp: env.ledger().timestamp(),
        };
        env.storage().persistent().set(&receipt_key, &receipt);
        Self::bump_replay(&env, &receipt_key);
        Self::bump_instance(&env);

        token::TokenClient::new(&env, &Self::token_ref(&env)).transfer(&from, &to, &amount);
        env.events()
            .publish((symbol_short!("paid"), from, to), (reference, amount));
        Ok(receipt)
    }

    /// Burns USDC through Circle CCTP V2 after the same policy checks as `pay`.
    /// Amounts must be exact at six decimals; the seventh Stellar decimal is
    /// rejected rather than silently rounded down by CCTP.
    pub fn bridge(
        env: Env,
        from: Address,
        intent: BridgeIntent,
        approvals: Vec<Address>,
    ) -> Result<BridgeReceipt, Error> {
        Self::require_unpaused(&env)?;
        Self::validate_intent(&env, intent.amount, intent.deadline)?;
        Self::require_supported_domain(intent.destination_domain)?;
        if intent.amount % 10 != 0 || intent.max_fee < 0 || intent.max_fee >= intent.amount {
            return Err(Error::InvalidBridgeAmount);
        }
        if intent.min_finality_threshold < STANDARD_FINALITY_THRESHOLD {
            return Err(Error::FastTransferUnsupported);
        }
        let zero = BytesN::from_array(&env, &[0; 32]);
        if intent.mint_recipient == zero || intent.destination_caller == zero {
            return Err(Error::InvalidDestination);
        }
        from.require_auth();
        let intent_key = IntentKey {
            account: from.clone(),
            reference: intent.reference.clone(),
        };
        let receipt_key = DataKey::Bridge(intent_key);
        Self::require_unused(&env, &receipt_key)?;
        if !Self::bridge_route_allowed(env.clone(), from.clone(), intent.destination_domain) {
            return Err(Error::BridgeRouteNotAllowed);
        }

        let policy = Self::policy_ref(&env, &from).ok_or(Error::NoPolicy)?;
        Self::require_payment_approval(&env, &policy, intent.amount, &approvals)?;
        Self::consume(&env, &from, &policy, intent.amount)?;

        let receipt = BridgeReceipt {
            reference: intent.reference.clone(),
            from: from.clone(),
            destination_domain: intent.destination_domain,
            mint_recipient: intent.mint_recipient.clone(),
            amount: intent.amount,
            max_fee: intent.max_fee,
            timestamp: env.ledger().timestamp(),
        };
        env.storage().persistent().set(&receipt_key, &receipt);
        Self::bump_replay(&env, &receipt_key);
        Self::bump_instance(&env);

        CctpClient::new(&env, &Self::cctp_ref(&env)).deposit_for_burn(
            &from,
            &intent.amount,
            &intent.destination_domain,
            &intent.mint_recipient,
            &Self::token_ref(&env),
            &intent.destination_caller,
            &intent.max_fee,
            &intent.min_finality_threshold,
        );
        env.events().publish(
            (symbol_short!("bridged"), from),
            (
                intent.reference,
                intent.destination_domain,
                intent.mint_recipient,
                intent.amount,
                intent.max_fee,
            ),
        );
        Ok(receipt)
    }

    pub fn usage(env: Env, account: Address) -> Usage {
        Self::usage_at(&env, &account)
    }

    pub fn remaining(env: Env, account: Address) -> Result<(i128, i128), Error> {
        let policy = Self::policy_ref(&env, &account).ok_or(Error::NoPolicy)?;
        let usage = Self::usage_at(&env, &account);
        Ok((
            (policy.daily_limit - usage.day_used).max(0),
            (policy.monthly_limit - usage.month_used).max(0),
        ))
    }

    pub fn payment_receipt(
        env: Env,
        account: Address,
        reference: BytesN<32>,
    ) -> Option<PaymentReceipt> {
        env.storage()
            .persistent()
            .get(&DataKey::Payment(IntentKey { account, reference }))
    }

    pub fn bridge_receipt(
        env: Env,
        account: Address,
        reference: BytesN<32>,
    ) -> Option<BridgeReceipt> {
        env.storage()
            .persistent()
            .get(&DataKey::Bridge(IntentKey { account, reference }))
    }

    pub fn network(env: Env) -> StellarNetwork {
        env.storage().instance().get(&DataKey::Network).unwrap()
    }

    pub fn token(env: Env) -> Address {
        Self::token_ref(&env)
    }

    pub fn cctp(env: Env) -> Address {
        Self::cctp_ref(&env)
    }

    pub fn admin(env: Env) -> Address {
        Self::admin_ref(&env)
    }

    pub fn pause(env: Env) {
        let admin = Self::admin_ref(&env);
        admin.require_auth();
        env.storage().instance().set(&DataKey::Paused, &true);
        Self::bump_instance(&env);
        env.events().publish((symbol_short!("paused"), admin), ());
    }

    pub fn unpause(env: Env) {
        let admin = Self::admin_ref(&env);
        admin.require_auth();
        env.storage().instance().set(&DataKey::Paused, &false);
        Self::bump_instance(&env);
        env.events().publish((symbol_short!("unpause"), admin), ());
    }

    pub fn paused(env: Env) -> bool {
        Self::is_paused(&env)
    }

    pub fn propose_admin(env: Env, next: Address) {
        let admin = Self::admin_ref(&env);
        admin.require_auth();
        env.storage().instance().set(&DataKey::PendingAdmin, &next);
        Self::bump_instance(&env);
        env.events()
            .publish((symbol_short!("prop_adm"), admin), next);
    }

    pub fn accept_admin(env: Env) -> Result<Address, Error> {
        let pending: Address = env
            .storage()
            .instance()
            .get(&DataKey::PendingAdmin)
            .ok_or(Error::NoPendingAdmin)?;
        pending.require_auth();
        env.storage().instance().set(&DataKey::Admin, &pending);
        env.storage().instance().remove(&DataKey::PendingAdmin);
        Self::bump_instance(&env);
        env.events()
            .publish((symbol_short!("admin"),), pending.clone());
        Ok(pending)
    }

    fn validate_policy(
        env: &Env,
        account: &Address,
        policy: &SpendingPolicy,
    ) -> Result<(), Error> {
        if policy.daily_limit <= 0
            || policy.monthly_limit < policy.daily_limit
            || policy.approval_above < 0
            || policy.approval_threshold > policy.approvers.len()
        {
            return Err(Error::InvalidPolicy);
        }
        if policy.approval_threshold == 0 {
            if !policy.approvers.is_empty() || policy.approval_above < policy.daily_limit {
                return Err(Error::InvalidPolicy);
            }
            return Ok(());
        }
        let mut unique = Vec::new(env);
        for approver in policy.approvers.iter() {
            if approver == *account || unique.contains(&approver) {
                return Err(Error::DuplicateApprover);
            }
            unique.push_back(approver);
        }
        Ok(())
    }

    fn require_quorum(
        env: &Env,
        policy: &SpendingPolicy,
        approvals: &Vec<Address>,
    ) -> Result<(), Error> {
        if policy.approval_threshold == 0 {
            if approvals.is_empty() {
                return Ok(());
            }
            return Err(Error::NotAnApprover);
        }
        let mut unique = Vec::new(env);
        for approval in approvals.iter() {
            if !policy.approvers.contains(&approval) {
                return Err(Error::NotAnApprover);
            }
            if unique.contains(&approval) {
                return Err(Error::DuplicateApprover);
            }
            approval.require_auth();
            unique.push_back(approval);
        }
        if unique.len() < policy.approval_threshold {
            return Err(Error::ApprovalUnavailable);
        }
        Ok(())
    }

    fn require_payment_approval(
        env: &Env,
        policy: &SpendingPolicy,
        amount: i128,
        approvals: &Vec<Address>,
    ) -> Result<(), Error> {
        if amount > policy.approval_above {
            Self::require_quorum(env, policy, approvals)
        } else if approvals.is_empty() {
            Ok(())
        } else {
            Err(Error::NotAnApprover)
        }
    }

    fn validate_intent(env: &Env, amount: i128, deadline: u64) -> Result<(), Error> {
        if amount <= 0 {
            return Err(Error::InvalidAmount);
        }
        let now = env.ledger().timestamp();
        if deadline < now || deadline > now.saturating_add(MAX_INTENT_LIFETIME) {
            return Err(Error::InvalidDeadline);
        }
        Ok(())
    }

    fn require_supported_domain(domain: u32) -> Result<(), Error> {
        if matches!(domain, ETHEREUM_DOMAIN | SOLANA_DOMAIN | BASE_DOMAIN) {
            Ok(())
        } else {
            Err(Error::UnsupportedBridgeDomain)
        }
    }

    fn require_unused(env: &Env, key: &DataKey) -> Result<(), Error> {
        if env.storage().persistent().has(key) {
            Err(Error::DuplicateReference)
        } else {
            Ok(())
        }
    }

    fn consume(
        env: &Env,
        account: &Address,
        policy: &SpendingPolicy,
        amount: i128,
    ) -> Result<(), Error> {
        let mut usage = Self::usage_at(env, account);
        usage.day_used = usage
            .day_used
            .checked_add(amount)
            .ok_or(Error::DailyLimitExceeded)?;
        if usage.day_used > policy.daily_limit {
            return Err(Error::DailyLimitExceeded);
        }
        usage.month_used = usage
            .month_used
            .checked_add(amount)
            .ok_or(Error::MonthlyLimitExceeded)?;
        if usage.month_used > policy.monthly_limit {
            return Err(Error::MonthlyLimitExceeded);
        }
        let key = DataKey::Usage(account.clone());
        env.storage().persistent().set(&key, &usage);
        Self::bump_live(env, &key);
        Ok(())
    }

    fn usage_at(env: &Env, account: &Address) -> Usage {
        let now = env.ledger().timestamp();
        let day_bucket = now / DAY_SECONDS;
        let month_bucket = now / MONTH_SECONDS;
        let key = DataKey::Usage(account.clone());
        let stored: Option<Usage> = env.storage().persistent().get(&key);
        if stored.is_some() {
            Self::bump_live(env, &key);
        }
        let mut usage = stored.unwrap_or(Usage {
            day_bucket,
            day_used: 0,
            month_bucket,
            month_used: 0,
        });
        if usage.day_bucket != day_bucket {
            usage.day_bucket = day_bucket;
            usage.day_used = 0;
        }
        if usage.month_bucket != month_bucket {
            usage.month_bucket = month_bucket;
            usage.month_used = 0;
        }
        usage
    }

    fn policy_ref(env: &Env, account: &Address) -> Option<SpendingPolicy> {
        let key = DataKey::Policy(account.clone());
        let value = env.storage().persistent().get(&key);
        if value.is_some() {
            Self::bump_live(env, &key);
        }
        value
    }

    fn require_unpaused(env: &Env) -> Result<(), Error> {
        if Self::is_paused(env) {
            Err(Error::Paused)
        } else {
            Ok(())
        }
    }

    fn is_paused(env: &Env) -> bool {
        let flag: Option<bool> = env.storage().instance().get(&DataKey::Paused);
        matches!(flag, Some(true))
    }

    fn admin_ref(env: &Env) -> Address {
        env.storage().instance().get(&DataKey::Admin).unwrap()
    }

    fn token_ref(env: &Env) -> Address {
        env.storage().instance().get(&DataKey::Token).unwrap()
    }

    fn cctp_ref(env: &Env) -> Address {
        env.storage().instance().get(&DataKey::Cctp).unwrap()
    }

    fn official_usdc(env: &Env, network: StellarNetwork) -> Address {
        match network {
            StellarNetwork::Testnet => Address::from_str(env, TESTNET_USDC),
            StellarNetwork::Mainnet => Address::from_str(env, MAINNET_USDC),
        }
    }

    fn official_cctp(env: &Env, network: StellarNetwork) -> Address {
        match network {
            StellarNetwork::Testnet => Address::from_str(env, TESTNET_CCTP),
            StellarNetwork::Mainnet => Address::from_str(env, MAINNET_CCTP),
        }
    }

    fn bump_live(env: &Env, key: &DataKey) {
        env.storage()
            .persistent()
            .extend_ttl(key, LIVE_THRESHOLD, LIVE_BUMP);
    }

    fn bump_replay(env: &Env, key: &DataKey) {
        env.storage()
            .persistent()
            .extend_ttl(key, LIVE_THRESHOLD, REPLAY_BUMP);
    }

    fn bump_instance(env: &Env) {
        env.storage().instance().extend_ttl(LIVE_THRESHOLD, LIVE_BUMP);
    }
}

#[cfg(test)]
mod tests;
