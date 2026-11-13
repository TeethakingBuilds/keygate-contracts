import * as freighter from '@stellar/freighter-api';
import { Contract, StrKey, TransactionBuilder, Networks, BASE_FEE, Address, nativeToScVal, scValToNative, rpc, Keypair } from '@stellar/stellar-sdk';
import { TESTNET_RPC, TESTNET_PASSPHRASE, TESTNET_USDC_SAC, TESTNET_USDC_ISSUER, parseAmount } from './web-core.js';

const server = new rpc.Server(TESTNET_RPC, { timeout: 15000 });

export function transactionHex(transaction) {
  return Array.from(transaction.hash(), (byte) => byte.toString(16).padStart(2, '0')).join('');
}

function checkResult(result) {
  if (result.error) throw new Error(result.error.message || 'Freighter declined this request.');
  return result;
}

export async function connect() {
  const connected = checkResult(await freighter.isConnected());
  if (!connected.isConnected) throw new Error('Freighter was not detected. Install or unlock Freighter in your normal browser, then reconnect.');
  const { address } = checkResult(await freighter.requestAccess());
  return readWalletDetails(address);
}

// Read existing access only; permission requests remain exclusive to connect().
export async function restore() {
  const connected = checkResult(await freighter.isConnected());
  if (!connected.isConnected) throw new Error('Freighter was not detected. Install or unlock Freighter in your normal browser, then reconnect.');
  const { address } = checkResult(await freighter.getAddress());
  return readWalletDetails(address);
}

async function readWalletDetails(address) {
  if (typeof address !== 'string' || !StrKey.isValidEd25519PublicKey(address)) throw new Error('Freighter did not return a valid Stellar account.');
  const network = checkResult(await freighter.getNetworkDetails());
  if (typeof network.network !== 'string' || !network.network || typeof network.networkPassphrase !== 'string' || !network.networkPassphrase) throw new Error('Freighter did not return valid network details. Reconnect before proceeding.');
  return { address, network: network.network, passphrase: network.networkPassphrase };
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
  return { xdr: ready.toXDR(), hash: transactionHex(ready), address, recipient, amount, fee: ready.fee, expiresAt: Number(ready.timeBounds.maxTime), token: TESTNET_USDC_SAC, issuer: TESTNET_USDC_ISSUER };
}

export async function signPrepared(prepared) {
  if (Date.now() / 1000 >= prepared.expiresAt - 10) throw new Error('The review expired. Simulate the payment again.');
  const current = await connect();
  if (current.address !== prepared.address) throw new Error('Wallet account changed. Prepare a new payment.');
  if (current.passphrase !== TESTNET_PASSPHRASE) throw new Error('Freighter must be on Stellar Testnet.');
  const signed = checkResult(await freighter.signTransaction(prepared.xdr, { address: prepared.address, networkPassphrase: TESTNET_PASSPHRASE }));
  return verifySigned(prepared, signed);
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
