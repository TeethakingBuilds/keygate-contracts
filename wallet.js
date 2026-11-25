import { Contract, StrKey, TransactionBuilder, Networks, BASE_FEE, Address, nativeToScVal, scValToNative, rpc, Keypair, xdr } from '@stellar/stellar-sdk';
import { TESTNET_RPC, TESTNET_PASSPHRASE, TESTNET_USDC_SAC, TESTNET_USDC_ISSUER, TESTNET_KEYGATE_PAYMENTS, parseAmount, formatUnits } from './web-core.js';

const server = new rpc.Server(TESTNET_RPC, { timeout: 15000 });

export function transactionHex(transaction) {
  return Array.from(transaction.hash(), (byte) => byte.toString(16).padStart(2, '0')).join('');
}

/**
 * The wallets the demo offers, in picker order. The ids are the kit's
 * product ids (Freighter is listed as Varial, its new name). WalletConnect,
 * Ledger and Trezor are left out on purpose: WalletConnect needs a Reown
 * Cloud project ID the project does not have, Ledger needs a Buffer
 * polyfill in the bundle and Trezor needs a manifest.
 *
 * The kit is loaded with dynamic imports, not a static import: the kit's
 * Varial module imports named exports from @stellar/freighter-api, whose
 * UMD bundle Node cannot load as ESM, so a static import would break
 * `pnpm test:web` (the tests never call loadKit, so the kit never loads
 * under Node). In the browser esbuild inlines the dynamic imports into
 * wallet.bundle.js — code splitting needs --outdir, and this build uses
 * a single --outfile, so there are no separate chunk files to deploy.
 *
 * The build also passes `--define:global=window` to esbuild. A kit
 * dependency (randombytes, via the HOT Wallet SDK) reads bare `global`
 * at module scope, and browsers have no `global` — without the define
 * the bundle throws "global is not defined" the moment it loads
 * (issue #23). The define rewrites those references to `window`,
 * which is the same object in a browser. Do not remove it without
 * re-checking the built bundle for bare `global` identifiers.
 */
const WALLET_MODULES = [
  { id: 'varial', load: () => import('@creit.tech/stellar-wallets-kit/modules/varial').then((module) => module.VarialModule) },
  { id: 'xbull', load: () => import('@creit.tech/stellar-wallets-kit/modules/xbull').then((module) => module.xBullModule) },
  { id: 'albedo', load: () => import('@creit.tech/stellar-wallets-kit/modules/albedo').then((module) => module.AlbedoModule) },
  { id: 'rabet', load: () => import('@creit.tech/stellar-wallets-kit/modules/rabet').then((module) => module.RabetModule) },
  { id: 'lobstr', load: () => import('@creit.tech/stellar-wallets-kit/modules/lobstr').then((module) => module.LobstrModule) },
  { id: 'hana', load: () => import('@creit.tech/stellar-wallets-kit/modules/hana').then((module) => module.HanaModule) },
  { id: 'hot-wallet', load: () => import('@creit.tech/stellar-wallets-kit/modules/hotwallet').then((module) => module.HotWalletModule) },
  { id: 'klever', load: () => import('@creit.tech/stellar-wallets-kit/modules/klever').then((module) => module.KleverModule) },
];

/** The wallet ids the picker offers, in picker order. */
export function supportedWalletIds() {
  return WALLET_MODULES.map((entry) => entry.id);
}

let kitPromise = null;

/**
 * Initialise the kit once, then reuse it. The kit keeps the connected
 * address and the selected module in localStorage, so restore-on-reload
 * works without opening the picker again.
 */
async function loadKit() {
  if (kitPromise) return kitPromise;
  kitPromise = (async () => {
    const [{ StellarWalletsKit }, { Networks: KitNetworks }, ...moduleClasses] = await Promise.all([
      import('@creit.tech/stellar-wallets-kit/sdk'),
      import('@creit.tech/stellar-wallets-kit/types'),
      ...WALLET_MODULES.map((entry) => entry.load()),
    ]);
    const modules = moduleClasses.map((Module) => new Module());
    StellarWalletsKit.init({ network: KitNetworks.TESTNET, modules });
    return StellarWalletsKit;
  })();
  return kitPromise;
}

/**
 * The kit throws plain { code, message } objects as well as Errors; both
 * carry the message the user should see.
 */
function kitMessage(error) {
  if (error instanceof Error) return error.message;
  if (error && typeof error.message === 'string' && error.message) return error.message;
  return 'The wallet request failed.';
}

/**
 * A connection is a signing wallet on the right network; nothing more is
 * stored. The kit keeps its own connected state, and every signing call
 * re-confirms the connected account before a signature is requested.
 * A kit can be injected for tests; without one the real kit is loaded.
 */
export async function connect(kit) {
  const activeKit = kit ?? (await loadKit());
  let address;
  try {
    ({ address } = await activeKit.authModal());
  } catch (error) {
    throw new Error(kitMessage(error));
  }
  return readWalletDetails(activeKit, address);
}

// Read existing access only; the picker opens only from connect().
export async function restore(kit) {
  const activeKit = kit ?? (await loadKit());
  let address;
  try {
    ({ address } = await activeKit.getAddress());
  } catch (error) {
    throw new Error(kitMessage(error));
  }
  return readWalletDetails(activeKit, address);
}

async function readWalletDetails(kit, address) {
  if (typeof address !== 'string' || !StrKey.isValidEd25519PublicKey(address)) throw new Error('The connected wallet did not return a valid Stellar account.');
  let network;
  try {
    network = await kit.getNetwork();
  } catch (error) {
    throw new Error(kitMessage(error));
  }
  if (typeof network.network !== 'string' || !network.network || typeof network.networkPassphrase !== 'string' || !network.networkPassphrase) throw new Error('The connected wallet did not return valid network details. Reconnect before proceeding.');
  let walletName = 'Wallet';
  try {
    walletName = kit.selectedModule?.productName ?? 'Wallet';
  } catch {
    walletName = 'Wallet';
  }
  return { address, walletName, network: network.network, passphrase: network.networkPassphrase };
}

export function validStellarAddress(value) {
  if (!StrKey.isValidEd25519PublicKey(value)) throw new Error('Enter a Stellar G-address with a valid checksum.');
}

export async function usdcBalance(address) {
  const account = await server.getAccount(address);
  const probe = new TransactionBuilder(account, { fee: BASE_FEE, networkPassphrase: Networks.TESTNET })
    .addOperation(new Contract(TESTNET_USDC_SAC).call('balance', new Address(address).toScVal()))
    .setTimeout(60).build();
  const simulation = await server.simulateTransaction(probe);
  if (rpc.Api.isSimulationError(simulation)) throw new Error(simulation.error);
  if (rpc.Api.isSimulationRestore(simulation)) throw new Error('Archived account data must be restored.');
  if (!simulation.result?.retval) throw new Error('USDC SAC returned no balance.');
  const value = simulation.result.retval;
  if (value.type !== 'scvI128') throw new Error('Unexpected USDC balance response.');
  return scValToNative(value);
}

export function buildDirect(account, { address, recipient, amount }) {
  validStellarAddress(address);
  validStellarAddress(recipient);
  if (account.accountId() !== address) throw new Error('Transaction source does not match the payer.');
  if (address === recipient) throw new Error('Source and recipient must be different accounts.');
  return new TransactionBuilder(account, { fee: BASE_FEE, networkPassphrase: TESTNET_PASSPHRASE })
    .addOperation(new Contract(TESTNET_USDC_SAC).call('transfer', new Address(address).toScVal(), new Address(recipient).toScVal(), nativeToScVal(parseAmount(amount), { type: 'i128' })))
    .setTimeout(180).build();
}

export async function prepareDirect({ address, recipient, amount }) {
  validStellarAddress(address);
  validStellarAddress(recipient);
  if (address === recipient) throw new Error('Source and recipient must be different accounts.');
  const account = await server.getAccount(address);
  const transaction = buildDirect(account, { address, recipient, amount });
  const simulation = await server.simulateTransaction(transaction);
  if (rpc.Api.isSimulationError(simulation)) throw new Error(`Simulation refused: ${simulation.error}`);
  if (rpc.Api.isSimulationRestore(simulation)) throw new Error('Restore archived ledger data before sending.');
  if (!simulation.result) throw new Error('Simulation returned no execution result.');
  for (const entry of simulation.result.auth ?? []) {
    if (entry.credentials.type !== 'sorobanCredentialsSourceAccount') {
      throw new Error('This account requires additional authorization. Multisig/smart-account signing is not enabled in the direct wallet flow.');
    }
  }
  const ready = rpc.assembleTransaction(transaction, simulation).build();
  return { xdr: ready.toXDR(), hash: transactionHex(ready), address, recipient, amount, fee: ready.fee, expiresAt: Number(ready.timeBounds.maxTime), token: TESTNET_USDC_SAC, issuer: TESTNET_USDC_ISSUER, route: 'direct' };
}

/**
 * Soroban encodes `Option<T>::None` as an `scvLedgerKeyContractInstance` — a
 * value no contract returns deliberately. Treat that tag as "absent".
 */
const NONE_TAG = 'scvLedgerKeyContractInstance';

/** Reads one contract struct as a name → field map, rejecting anything else. */
function mapFields(scval, context) {
  if (scval.type !== 'scvMap') throw new Error(`${context}: expected a map, got ${scval.type}`);
  const fields = new Map();
  for (const entry of scval.value) fields.set(String(entry.key.value), entry.val);
  return fields;
}

function i128Field(fields, name, context) {
  const value = fields.get(name);
  if (value === undefined) throw new Error(`${context}: missing field '${name}'`);
  if (value.type !== 'scvI128') throw new Error(`${context}.${name}: expected i128, got ${value.type}`);
  return value.value.hi * (1n << 64n) + value.value.lo;
}

function u32Field(fields, name, context) {
  const value = fields.get(name);
  if (value === undefined) throw new Error(`${context}: missing field '${name}'`);
  if (value.type !== 'scvU32') throw new Error(`${context}.${name}: expected u32, got ${value.type}`);
  return Number(value.value);
}

function boolField(fields, name, context) {
  const value = fields.get(name);
  if (value === undefined) throw new Error(`${context}: missing field '${name}'`);
  if (value.type !== 'scvBool') throw new Error(`${context}.${name}: expected bool, got ${value.type}`);
  return Boolean(value.value);
}

/** `SpendingPolicy` in `keygate-payments`, or `null` when no policy is set. */
export function decodeSpendingPolicy(retval) {
  if (retval.type === NONE_TAG) return null;
  const fields = mapFields(retval, 'policy');
  return {
    dailyLimit: i128Field(fields, 'daily_limit', 'policy'),
    monthlyLimit: i128Field(fields, 'monthly_limit', 'policy'),
    approvalAbove: i128Field(fields, 'approval_above', 'policy'),
    approvers: (() => {
      const value = fields.get('approvers');
      if (value === undefined) throw new Error(`policy: missing field 'approvers'`);
      if (value.type !== 'scvVec') throw new Error(`policy.approvers: expected vec, got ${value.type}`);
      return value.value.map((entry) => Address.fromScAddress(entry.value).toString());
    })(),
    approvalThreshold: u32Field(fields, 'approval_threshold', 'policy'),
    allowlistOnly: boolField(fields, 'allowlist_only', 'policy'),
  };
}

/** `remaining` in `keygate-payments`: the `(daily, monthly)` headroom pair. */
export function decodeRemaining(retval) {
  if (retval.type !== 'scvVec' || retval.value.length !== 2) throw new Error('remaining: expected a (daily, monthly) pair');
  const [daily, monthly] = retval.value;
  if (daily.type !== 'scvI128' || monthly.type !== 'scvI128') throw new Error('remaining: expected i128 values');
  return {
    daily: daily.value.hi * (1n << 64n) + daily.value.lo,
    monthly: monthly.value.hi * (1n << 64n) + monthly.value.lo,
  };
}

/** `is_allowed` in `keygate-payments`. */
export function decodeAllowed(retval) {
  if (retval.type !== 'scvBool') throw new Error(`is_allowed: expected bool, got ${retval.type}`);
  return Boolean(retval.value);
}

/** Simulates one read-only contract call and returns its return value. */
async function simulateRead(account, contract, func, args) {
  const probe = new TransactionBuilder(account, { fee: BASE_FEE, networkPassphrase: Networks.TESTNET })
    .addOperation(new Contract(contract).call(func, ...args))
    .setTimeout(60).build();
  const simulation = await server.simulateTransaction(probe);
  if (rpc.Api.isSimulationError(simulation)) throw new Error(`Policy read refused: ${simulation.error}`);
  if (rpc.Api.isSimulationRestore(simulation)) throw new Error('Restore archived ledger data before sending.');
  if (!simulation.result?.retval) throw new Error(`Policy read '${func}' returned no value.`);
  return simulation.result.retval;
}

export function newPaymentReference() {
  const reference = new Uint8Array(32);
  globalThis.crypto.getRandomValues(reference);
  return reference;
}

export function buildPolicy(account, { address, recipient, amount, reference, deadline }) {
  validStellarAddress(address);
  validStellarAddress(recipient);
  if (account.accountId() !== address) throw new Error('Transaction source does not match the payer.');
  if (address === recipient) throw new Error('Source and recipient must be different accounts.');
  if (!(reference instanceof Uint8Array) || reference.length !== 32) throw new Error('Payment reference must be exactly 32 bytes.');
  if (!Number.isInteger(deadline) || deadline <= 0) throw new Error('Payment deadline must be unix seconds in the future.');
  if (deadline > Math.floor(Date.now() / 1000) + 7 * 24 * 3600) throw new Error('Payment deadline must be within 7 days — the contract refuses longer intents.');
  return new TransactionBuilder(account, { fee: BASE_FEE, networkPassphrase: TESTNET_PASSPHRASE })
    .addOperation(new Contract(TESTNET_KEYGATE_PAYMENTS).call(
      'pay',
      new Address(address).toScVal(),
      new Address(recipient).toScVal(),
      nativeToScVal(parseAmount(amount), { type: 'i128' }),
      xdr.ScVal.scvBytes(reference),
      nativeToScVal(BigInt(deadline), { type: 'u64' }),
      xdr.ScVal.scvVec([]),
    ))
    .setTimeout(180).build();
}

export async function preparePolicy({ address, recipient, amount }) {
  validStellarAddress(address);
  validStellarAddress(recipient);
  if (address === recipient) throw new Error('Source and recipient must be different accounts.');
  const account = await server.getAccount(address);
  const units = parseAmount(amount);
  const policy = decodeSpendingPolicy(await simulateRead(account, TESTNET_KEYGATE_PAYMENTS, 'policy', [new Address(address).toScVal()]));
  if (policy === null) throw new Error('No spending policy is set for this account on the Keygate payments contract. Set one before sending through the policy path.');
  const allowed = decodeAllowed(await simulateRead(account, TESTNET_KEYGATE_PAYMENTS, 'is_allowed', [new Address(address).toScVal(), new Address(recipient).toScVal()]));
  if (!allowed) throw new Error('The policy does not allow this account to pay that recipient.');
  const remaining = decodeRemaining(await simulateRead(account, TESTNET_KEYGATE_PAYMENTS, 'remaining', [new Address(address).toScVal()]));
  if (units > remaining.daily) throw new Error(`Amount exceeds the daily headroom (${formatUnits(remaining.daily)} USDC left today).`);
  if (units > remaining.monthly) throw new Error(`Amount exceeds the monthly headroom (${formatUnits(remaining.monthly)} USDC left this month).`);
  if (units > policy.approvalAbove && policy.approvalThreshold > 0) throw new Error('This amount is above the policy approval threshold, so co-signers must approve it — the wallet flow cannot attach their signatures. Send a smaller amount or arrange approvals first.');
  const reference = newPaymentReference();
  const deadline = Math.floor(Date.now() / 1000) + 3600;
  const transaction = buildPolicy(account, { address, recipient, amount, reference, deadline });
  const simulation = await server.simulateTransaction(transaction);
  if (rpc.Api.isSimulationError(simulation)) throw new Error(`Simulation refused: ${simulation.error}`);
  if (rpc.Api.isSimulationRestore(simulation)) throw new Error('Restore archived ledger data before sending.');
  if (!simulation.result) throw new Error('Simulation returned no execution result.');
  for (const entry of simulation.result.auth ?? []) {
    if (entry.credentials.type !== 'sorobanCredentialsSourceAccount') {
      throw new Error('This account requires additional authorization. Multisig/smart-account signing is not enabled in the policy wallet flow.');
    }
  }
  const ready = rpc.assembleTransaction(transaction, simulation).build();
  return { xdr: ready.toXDR(), hash: transactionHex(ready), address, recipient, amount, fee: ready.fee, expiresAt: Number(ready.timeBounds.maxTime), route: 'policy', reference: Array.from(reference, (byte) => byte.toString(16).padStart(2, '0')).join(''), deadline, payments: TESTNET_KEYGATE_PAYMENTS };
}

export async function signPrepared(prepared, kit) {
  if (Date.now() / 1000 >= prepared.expiresAt - 10) throw new Error('The review expired. Simulate the payment again.');
  const activeKit = kit ?? (await loadKit());
  const current = await restore(activeKit);
  if (current.address !== prepared.address) throw new Error('Wallet account changed. Prepare a new payment.');
  if (current.passphrase !== TESTNET_PASSPHRASE) throw new Error('The connected wallet must be on Stellar Testnet.');
  let signed;
  try {
    signed = await activeKit.signTransaction(prepared.xdr, { address: prepared.address, networkPassphrase: TESTNET_PASSPHRASE });
  } catch (error) {
    throw new Error(kitMessage(error));
  }
  return verifySigned(prepared, signed);
}

/** Clears the kit's stored connection; local state is cleared by the caller. */
export async function disconnect(kit) {
  const activeKit = kit ?? (await loadKit());
  try {
    await activeKit.disconnect();
  } catch {
    // The wallet is already disconnected.
  }
}

export function verifySigned(prepared, signed) {
  if (signed.signerAddress !== prepared.address) throw new Error('Unexpected wallet signer.');
  const original = TransactionBuilder.fromXDR(prepared.xdr, TESTNET_PASSPHRASE);
  const transaction = TransactionBuilder.fromXDR(signed.signedTxXdr, TESTNET_PASSPHRASE);
  const expected = original.signatureBase();
  const actual = transaction.signatureBase();
  if (actual.length !== expected.length || !actual.every((byte, index) => byte === expected[index])) throw new Error('Signed transaction differs from the reviewed transaction.');
  if (transaction.source !== prepared.address || transactionHex(transaction) !== prepared.hash) throw new Error('Reviewed payer or transaction hash changed.');
  const key = Keypair.fromPublicKey(prepared.address);
  if (!transaction.signatures.some((signature) => key.verify(transaction.hash(), signature.signature))) throw new Error('Wallet returned no valid account signature.');
  if (Date.now() / 1000 >= prepared.expiresAt) throw new Error('The transaction expired while awaiting wallet approval. Prepare it again.');
  return transaction;
}

export async function submitSigned(transaction) {
  const response = await server.sendTransaction(transaction);
  if (response.hash !== transactionHex(transaction)) throw new Error('RPC returned an unexpected transaction hash.');
  if (response.status === 'ERROR') throw new Error('Ledger submission was rejected. Check the transaction status before retrying.');
  if (!['PENDING', 'DUPLICATE', 'TRY_AGAIN_LATER'].includes(response.status)) throw new Error('Unrecognized RPC submission status.');
  return response;
}

export async function getTransaction(hash) {
  const response = await server.getTransaction(hash);
  return { status: response.status, ledger: response.ledger ?? null, latestLedger: response.latestLedger };
}
