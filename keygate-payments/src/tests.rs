// Circle's fixed burn ABI also gives generated SDK helpers eight arguments.
#![allow(clippy::too_many_arguments)]

extern crate std;

use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, panic_with_error,
    testutils::{Address as _, Ledger as _},
    Address, BytesN, Env, String, Vec,
};

use super::{
    BridgeIntent, BridgeReceipt, Error, KeygatePayments, KeygatePaymentsClient, PaymentReceipt,
    SpendingPolicy, StellarNetwork, DAY_SECONDS, MONTH_SECONDS,
};

const TESTNET_USDC: &str = "CBIELTK6YBZJU5UP2WWQEUCYKLPU6AUNZ2BQ4WWFEIE3USCIHMXQDAMA";
const TESTNET_CCTP: &str = "CDNG7HXAPBWICI2E3AUBP3YZWZELJLYSB6F5CC7WLDTLTHVM74SLRTHP";

#[contracttype]
#[derive(Clone)]
enum TokenKey {
    Balance(Address),
}

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
enum TokenError {
    InsufficientBalance = 1,
}

#[contract]
struct MockUsdc;

#[contractimpl]
impl MockUsdc {
    pub fn decimals(_env: Env) -> u32 {
        7
    }

    pub fn name(env: Env) -> String {
        String::from_str(&env, "USDC:test")
    }

    pub fn symbol(env: Env) -> String {
        String::from_str(&env, "USDC")
    }

    pub fn mint(env: Env, to: Address, amount: i128) {
        let key = TokenKey::Balance(to);
        let balance: i128 = env.storage().persistent().get(&key).unwrap_or(0);
        env.storage().persistent().set(&key, &(balance + amount));
    }

    pub fn balance(env: Env, account: Address) -> i128 {
        env.storage()
            .persistent()
            .get(&TokenKey::Balance(account))
            .unwrap_or(0)
    }

    pub fn transfer(env: Env, from: Address, to: Address, amount: i128) {
        from.require_auth();
        let from_key = TokenKey::Balance(from);
        let to_key = TokenKey::Balance(to);
        let from_balance: i128 = env.storage().persistent().get(&from_key).unwrap_or(0);
        if amount < 0 || from_balance < amount {
            panic_with_error!(&env, TokenError::InsufficientBalance);
        }
        let to_balance: i128 = env.storage().persistent().get(&to_key).unwrap_or(0);
        env.storage()
            .persistent()
            .set(&from_key, &(from_balance - amount));
        env.storage()
            .persistent()
            .set(&to_key, &(to_balance + amount));
    }
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
struct BurnCall {
    caller: Address,
    amount: i128,
    destination_domain: u32,
    mint_recipient: BytesN<32>,
    burn_token: Address,
    destination_caller: BytesN<32>,
    max_fee: i128,
    min_finality_threshold: u32,
}

#[contract]
struct MockCctp;

#[contractimpl]
impl MockCctp {
    pub fn deposit_for_burn(
        env: Env,
        caller: Address,
        amount: i128,
        destination_domain: u32,
        mint_recipient: BytesN<32>,
        burn_token: Address,
        destination_caller: BytesN<32>,
        max_fee: i128,
        min_finality_threshold: u32,
    ) {
        caller.require_auth();
        env.storage().instance().set(
            &0_u32,
            &BurnCall {
                caller,
                amount,
                destination_domain,
                mint_recipient,
                burn_token,
                destination_caller,
                max_fee,
                min_finality_threshold,
            },
        );
    }

    pub fn last(env: Env) -> Option<BurnCall> {
        env.storage().instance().get(&0_u32)
    }
}

struct Fixture {
    env: Env,
    contract: Address,
    token: Address,
    cctp: Address,
    admin: Address,
    payer: Address,
    recipient: Address,
    approver_a: Address,
    approver_b: Address,
}

impl Fixture {
    fn new(now: u64) -> Self {
        let env = Env::default();
        env.ledger().set_timestamp(now);
        env.mock_all_auths();

        let token = Address::from_str(&env, TESTNET_USDC);
        env.register_at(&token, MockUsdc, ());
        let cctp = Address::from_str(&env, TESTNET_CCTP);
        env.register_at(&cctp, MockCctp, ());

        let admin = Address::generate(&env);
        let contract = env.register(
            KeygatePayments,
            (&admin, StellarNetwork::Testnet, &token, &cctp),
        );
        let payer = Address::generate(&env);
        let recipient = Address::generate(&env);
        let approver_a = Address::generate(&env);
        let approver_b = Address::generate(&env);
        Self {
            env,
            contract,
            token,
            cctp,
            admin,
            payer,
            recipient,
            approver_a,
            approver_b,
        }
    }

    fn client(&self) -> KeygatePaymentsClient<'_> {
        KeygatePaymentsClient::new(&self.env, &self.contract)
    }

    fn token(&self) -> MockUsdcClient<'_> {
        MockUsdcClient::new(&self.env, &self.token)
    }

    fn cctp(&self) -> MockCctpClient<'_> {
        MockCctpClient::new(&self.env, &self.cctp)
    }

    fn reference(&self, byte: u8) -> BytesN<32> {
        BytesN::from_array(&self.env, &[byte; 32])
    }

    fn no_approvals(&self) -> Vec<Address> {
        Vec::new(&self.env)
    }

    fn policy_without_approvals(&self, daily: i128, monthly: i128) -> SpendingPolicy {
        SpendingPolicy {
            daily_limit: daily,
            monthly_limit: monthly,
            approval_above: daily,
            approvers: Vec::new(&self.env),
            approval_threshold: 0,
            allowlist_only: false,
        }
    }

    fn threshold_policy(&self) -> SpendingPolicy {
        SpendingPolicy {
            daily_limit: 1_000,
            monthly_limit: 5_000,
            approval_above: 100,
            approvers: Vec::from_array(
                &self.env,
                [self.approver_a.clone(), self.approver_b.clone()],
            ),
            approval_threshold: 2,
            allowlist_only: false,
        }
    }

    fn bridge_intent(
        &self,
        destination_domain: u32,
        amount: i128,
        finality: u32,
        reference: u8,
    ) -> BridgeIntent {
        BridgeIntent {
            destination_domain,
            mint_recipient: BytesN::from_array(&self.env, &[1; 32]),
            destination_caller: BytesN::from_array(&self.env, &[2; 32]),
            amount,
            max_fee: 0,
            min_finality_threshold: finality,
            reference: self.reference(reference),
            deadline: 1_600,
        }
    }

    fn install(&self, policy: &SpendingPolicy, approvals: &Vec<Address>) {
        self.client().set_policy(&self.payer, policy, approvals);
    }

    fn mint(&self, amount: i128) {
        self.token().mint(&self.payer, &amount);
    }
}

#[test]
fn constructor_binds_official_addresses() {
    let f = Fixture::new(1_000);
    assert_eq!(f.client().network(), StellarNetwork::Testnet);
    assert_eq!(f.client().token(), f.token);
    assert_eq!(f.client().cctp(), f.cctp);
    assert_eq!(f.client().admin(), f.admin);
}

#[test]
#[should_panic]
fn constructor_refuses_non_usdc_address() {
    let env = Env::default();
    env.mock_all_auths();
    let fake = env.register(MockUsdc, ());
    let cctp = Address::from_str(&env, TESTNET_CCTP);
    env.register_at(&cctp, MockCctp, ());
    let admin = Address::generate(&env);
    env.register(
        KeygatePayments,
        (&admin, StellarNetwork::Testnet, &fake, &cctp),
    );
}

#[test]
fn direct_payment_moves_usdc_and_records_usage() {
    let f = Fixture::new(1_000);
    f.install(&f.policy_without_approvals(1_000, 5_000), &f.no_approvals());
    f.mint(800);

    let receipt = f.client().pay(
        &f.payer,
        &f.recipient,
        &300,
        &f.reference(1),
        &1_600,
        &f.no_approvals(),
    );

    assert_eq!(receipt.amount, 300);
    assert_eq!(f.token().balance(&f.payer), 500);
    assert_eq!(f.token().balance(&f.recipient), 300);
    assert_eq!(f.client().usage(&f.payer).day_used, 300);
    assert_eq!(
        f.client().payment_receipt(&f.payer, &f.reference(1)),
        Some(receipt)
    );
    assert_eq!(f.client().remaining(&f.payer), (700, 4_700));
}

#[test]
fn allowlist_is_enforced_before_transfer() {
    let f = Fixture::new(1_000);
    let mut policy = f.policy_without_approvals(1_000, 5_000);
    policy.allowlist_only = true;
    f.install(&policy, &f.no_approvals());
    f.mint(500);

    assert!(matches!(
        f.client().try_pay(
            &f.payer,
            &f.recipient,
            &100,
            &f.reference(1),
            &1_600,
            &f.no_approvals(),
        ),
        Err(Ok(Error::RecipientNotAllowed))
    ));

    f.client()
        .allow_recipient(&f.payer, &f.recipient, &f.no_approvals());
    f.client().pay(
        &f.payer,
        &f.recipient,
        &100,
        &f.reference(1),
        &1_600,
        &f.no_approvals(),
    );
    assert!(f.client().is_allowed(&f.payer, &f.recipient));

    f.client().disallow_recipient(&f.payer, &f.recipient);
    assert!(!f.client().is_allowed(&f.payer, &f.recipient));
}

#[test]
fn high_value_payment_requires_distinct_quorum() {
    let f = Fixture::new(1_000);
    let all = Vec::from_array(&f.env, [f.approver_a.clone(), f.approver_b.clone()]);
    f.install(&f.threshold_policy(), &all);
    f.mint(1_000);

    let one = Vec::from_array(&f.env, [f.approver_a.clone()]);
    assert!(matches!(
        f.client()
            .try_pay(&f.payer, &f.recipient, &200, &f.reference(1), &1_600, &one,),
        Err(Ok(Error::ApprovalUnavailable))
    ));

    let duplicate = Vec::from_array(&f.env, [f.approver_a.clone(), f.approver_a.clone()]);
    assert!(matches!(
        f.client().try_pay(
            &f.payer,
            &f.recipient,
            &200,
            &f.reference(1),
            &1_600,
            &duplicate,
        ),
        Err(Ok(Error::DuplicateApprover))
    ));

    f.client()
        .pay(&f.payer, &f.recipient, &200, &f.reference(1), &1_600, &all);
}

#[test]
fn initial_policy_requires_nominated_quorum() {
    let f = Fixture::new(1_000);
    let one = Vec::from_array(&f.env, [f.approver_a.clone()]);
    assert!(matches!(
        f.client()
            .try_set_policy(&f.payer, &f.threshold_policy(), &one),
        Err(Ok(Error::ApprovalUnavailable))
    ));
}

#[test]
fn limits_reset_only_at_documented_bucket_boundaries() {
    let f = Fixture::new(DAY_SECONDS - 10);
    f.install(&f.policy_without_approvals(100, 150), &f.no_approvals());
    f.mint(300);
    f.client().pay(
        &f.payer,
        &f.recipient,
        &100,
        &f.reference(1),
        &DAY_SECONDS,
        &f.no_approvals(),
    );

    assert!(matches!(
        f.client().try_pay(
            &f.payer,
            &f.recipient,
            &1,
            &f.reference(2),
            &DAY_SECONDS,
            &f.no_approvals(),
        ),
        Err(Ok(Error::DailyLimitExceeded))
    ));

    f.env.ledger().set_timestamp(DAY_SECONDS);
    f.client().pay(
        &f.payer,
        &f.recipient,
        &50,
        &f.reference(2),
        &(DAY_SECONDS + 100),
        &f.no_approvals(),
    );
    assert!(matches!(
        f.client().try_pay(
            &f.payer,
            &f.recipient,
            &1,
            &f.reference(3),
            &(DAY_SECONDS + 100),
            &f.no_approvals(),
        ),
        Err(Ok(Error::MonthlyLimitExceeded))
    ));

    f.env.ledger().set_timestamp(MONTH_SECONDS);
    assert_eq!(f.client().remaining(&f.payer), (100, 150));
}

#[test]
fn references_are_single_use_per_payer() {
    let f = Fixture::new(1_000);
    f.install(&f.policy_without_approvals(1_000, 5_000), &f.no_approvals());
    f.mint(500);
    let reference = f.reference(1);
    f.client().pay(
        &f.payer,
        &f.recipient,
        &100,
        &reference,
        &1_600,
        &f.no_approvals(),
    );
    assert!(matches!(
        f.client().try_pay(
            &f.payer,
            &f.recipient,
            &100,
            &reference,
            &1_600,
            &f.no_approvals(),
        ),
        Err(Ok(Error::DuplicateReference))
    ));

    let other = Address::generate(&f.env);
    f.client().set_policy(
        &other,
        &f.policy_without_approvals(1_000, 5_000),
        &f.no_approvals(),
    );
    f.token().mint(&other, &100);
    f.client().pay(
        &other,
        &f.recipient,
        &100,
        &reference,
        &1_600,
        &f.no_approvals(),
    );
}

#[test]
fn expired_and_excessive_deadlines_are_rejected() {
    let f = Fixture::new(1_000);
    f.install(&f.policy_without_approvals(1_000, 5_000), &f.no_approvals());
    assert!(matches!(
        f.client().try_pay(
            &f.payer,
            &f.recipient,
            &100,
            &f.reference(1),
            &999,
            &f.no_approvals(),
        ),
        Err(Ok(Error::InvalidDeadline))
    ));
    assert!(matches!(
        f.client().try_pay(
            &f.payer,
            &f.recipient,
            &100,
            &f.reference(1),
            &(1_000 + 8 * DAY_SECONDS),
            &f.no_approvals(),
        ),
        Err(Ok(Error::InvalidDeadline))
    ));
}

#[test]
fn nested_transfer_failure_rolls_back_usage_and_receipt() {
    let f = Fixture::new(1_000);
    f.install(&f.policy_without_approvals(1_000, 5_000), &f.no_approvals());
    let result = f.client().try_pay(
        &f.payer,
        &f.recipient,
        &100,
        &f.reference(1),
        &1_600,
        &f.no_approvals(),
    );
    assert!(result.is_err());
    assert_eq!(f.client().usage(&f.payer).day_used, 0);
    assert_eq!(f.client().payment_receipt(&f.payer, &f.reference(1)), None);
}

#[test]
fn pause_blocks_payments_without_blocking_policy_tightening() {
    let f = Fixture::new(1_000);
    f.install(&f.policy_without_approvals(1_000, 5_000), &f.no_approvals());
    f.mint(100);
    f.client().pause();
    assert!(f.client().paused());
    assert!(matches!(
        f.client().try_pay(
            &f.payer,
            &f.recipient,
            &100,
            &f.reference(1),
            &1_600,
            &f.no_approvals(),
        ),
        Err(Ok(Error::Paused))
    ));

    f.client().set_policy(
        &f.payer,
        &f.policy_without_approvals(500, 2_000),
        &f.no_approvals(),
    );
    f.client().unpause();
    assert!(!f.client().paused());
}

#[test]
fn bridge_requires_supported_enabled_standard_route_and_exact_precision() {
    let f = Fixture::new(1_000);
    f.install(
        &f.policy_without_approvals(10_000_000, 50_000_000),
        &f.no_approvals(),
    );

    assert!(matches!(
        f.client()
            .try_set_bridge_route(&f.payer, &27, &true, &f.no_approvals()),
        Err(Ok(Error::UnsupportedBridgeDomain))
    ));
    f.client()
        .set_bridge_route(&f.payer, &0, &true, &f.no_approvals());

    assert!(matches!(
        f.client().try_bridge(
            &f.payer,
            &f.bridge_intent(0, 1_000_001, 2_000, 1),
            &f.no_approvals(),
        ),
        Err(Ok(Error::InvalidBridgeAmount))
    ));
    assert!(matches!(
        f.client().try_bridge(
            &f.payer,
            &f.bridge_intent(0, 1_000_000, 1_000, 1),
            &f.no_approvals(),
        ),
        Err(Ok(Error::FastTransferUnsupported))
    ));

    let mut intent = f.bridge_intent(0, 1_000_000, 2_000, 1);
    intent.max_fee = 10_000;
    let receipt: BridgeReceipt = f.client().bridge(&f.payer, &intent, &f.no_approvals());
    assert_eq!(receipt.destination_domain, 0);
    let burn = f.cctp().last().unwrap();
    assert_eq!(burn.caller, f.payer);
    assert_eq!(burn.burn_token, f.token);
    assert_eq!(burn.amount, 1_000_000);
    assert_eq!(burn.max_fee, 10_000);
}

#[test]
fn bridge_route_must_be_enabled_for_payer() {
    let f = Fixture::new(1_000);
    f.install(
        &f.policy_without_approvals(10_000_000, 50_000_000),
        &f.no_approvals(),
    );
    assert!(matches!(
        f.client().try_bridge(
            &f.payer,
            &f.bridge_intent(6, 1_000_000, 2_000, 1),
            &f.no_approvals(),
        ),
        Err(Ok(Error::BridgeRouteNotAllowed))
    ));
}

#[test]
fn admin_handover_is_two_step() {
    let f = Fixture::new(1_000);
    let next = Address::generate(&f.env);
    f.client().propose_admin(&next);
    assert_eq!(f.client().accept_admin(), next);
    assert_eq!(f.client().admin(), next);
}

#[test]
fn payment_receipt_type_is_stable() {
    let f = Fixture::new(1_000);
    let receipt = PaymentReceipt {
        reference: f.reference(9),
        from: f.payer,
        to: f.recipient,
        amount: 1,
        timestamp: 1_000,
    };
    assert_eq!(receipt.amount, 1);
}
