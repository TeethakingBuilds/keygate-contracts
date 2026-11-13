import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { Account, Address, Asset, Keypair, scValToNative, TransactionBuilder } from '@stellar/stellar-sdk';
import { buildDirect, validStellarAddress, transactionHex, verifySigned } from './wallet.js';
import { TESTNET_USDC_ISSUER, TESTNET_USDC_SAC, TESTNET_PASSPHRASE, parseAmount, formatUnits, validateEvmAddress, decodeSolanaAddress, circleState, transactionHash, referenceList } from './web-core.js';

const payer = Keypair.random().publicKey();
const recipient = Keypair.random().publicKey();

test('amount parsing uses integer precision, never floating point', () => {
  assert.equal(parseAmount('10.1234567'), 101234567n);
  assert.equal(parseAmount('900719925.4740993'), 9007199254740993n);
  assert.equal(formatUnits(101234567n), '10.1234567');
});
test('CCTP accepts a trailing zero seventh decimal but rejects lost precision', () => {
  assert.equal(parseAmount('1.1234560', 6), 1123456n);
  assert.throws(() => parseAmount('1.1234567', 6));
});
test('zero, negative, exponent, malformed and excessive amounts are rejected', () => {
  for (const amount of ['0', '0.0000000', '-1', '1e3', '1,000', '01', '1.', '.2', 'NaN', '1.00000001', '170141183460469231731687303715884105728']) {
    assert.throws(() => parseAmount(amount), amount);
  }
});
test('amount is bounded by i128 in asset minor units', () => {
  const maximum = 2n ** 127n - 1n;
  assert.equal(parseAmount(formatUnits(maximum)), maximum);
  assert.throws(() => parseAmount(formatUnits(maximum + 1n)));
});
test('official USDC SAC derives from the official Testnet issuer', () => {
  assert.equal(new Asset('USDC', TESTNET_USDC_ISSUER).contractId(TESTNET_PASSPHRASE), TESTNET_USDC_SAC);
});
test('Stellar addresses use checksum validation', () => {
  assert.doesNotThrow(() => validStellarAddress(payer));
  assert.throws(() => validStellarAddress('G' + 'A'.repeat(55)));
  assert.throws(() => validStellarAddress(TESTNET_USDC_SAC));
});
test('EVM recipients must be non-zero and exactly 20 bytes', () => {
  assert.doesNotThrow(() => validateEvmAddress('0x' + 'a1'.repeat(20)));
  assert.throws(() => validateEvmAddress('0x' + '0'.repeat(40)));
  assert.throws(() => validateEvmAddress('0x1234'));
});
test('Solana validation checks decoded 32-byte length', () => {
  assert.equal(decodeSolanaAddress('So11111111111111111111111111111111111111112').length, 32);
  for (const value of ['1'.repeat(32), '1'.repeat(33), '0'.repeat(44), 'a'.repeat(44)]) assert.throws(() => decodeSolanaAddress(value));
});
test('attested does not mean destination settlement succeeded', () => {
  const state = circleState({ status: 'complete', message: '0x1234', attestation: '0xabcd' });
  assert.equal(state.attested, true);
  assert.match(state.label, /settlement not verified/);
});
test('missing or pending Circle proof never becomes attested', () => {
  assert.equal(circleState(null).attested, false);
  assert.equal(circleState({ status: 'complete', message: '0x12', attestation: 'PENDING' }).attested, false);
  assert.equal(circleState({ status: 'pending_confirmations', message: '0x12', attestation: '0xab' }).attested, false);
  assert.equal(circleState({ status: 'complete', message: '0x123', attestation: '0xab' }).attested, false);
  assert.equal(circleState({ status: 'complete', message: '0x12', attestation: '0xabc' }).attested, false);
});
test('Circle forwarding success is labeled provider-reported, not ledger-confirmed', () => {
  assert.match(circleState({ forwardState: 'COMPLETE' }).label, /Circle reports forwarded/);
  assert.match(circleState({ forwardState: 'FAILED' }).label, /recovery required/);
});
test('hash lookup only accepts transaction hashes', () => {
  assert.equal(transactionHash('A'.repeat(64)), 'a'.repeat(64));
  assert.throws(() => transactionHash('../accounts/x'));
  assert.throws(() => transactionHash('123'));
});
test('saved references discard untrusted local statuses', () => {
  assert.deepEqual(referenceList([{ account: payer, hash: 'a'.repeat(64), status: 'SUCCESS' }]), [{ account: payer, hash: 'a'.repeat(64) }]);
  assert.deepEqual(referenceList([{ account: '<script>', hash: 'a'.repeat(64) }]), []);
});
test('direct payment builds exactly the official SAC transfer invocation', () => {
  const transaction = buildDirect(new Account(payer, '12'), { address: payer, recipient, amount: '1.0000001' });
  assert.equal(transaction.source, payer);
  assert.equal(transaction.sequence, '13');
  assert.equal(transaction.signatures.length, 0);
  assert.equal(transaction.operations.length, 1);
  const operation = transaction.operations[0];
  assert.equal(operation.type, 'invokeHostFunction');
  const invocation = operation.func.invokeContract;
  assert.equal(Address.fromScAddress(invocation.contractAddress).toString(), TESTNET_USDC_SAC);
  assert.equal(invocation.functionName.toString(), 'transfer');
  const args = invocation.args.map(scValToNative);
  assert.equal(args[0], payer);
  assert.equal(args[1], recipient);
  assert.equal(args[2], 10000001n);
});
test('direct transaction is Testnet-bound and time-limited', () => {
  const transaction = buildDirect(new Account(payer, '1'), { address: payer, recipient, amount: '5' });
  assert.equal(transaction.networkPassphrase, TESTNET_PASSPHRASE);
  assert.ok(Number(transaction.timeBounds.maxTime) > Date.now() / 1000);
  assert.ok(Number(transaction.timeBounds.maxTime) <= Date.now() / 1000 + 181);
  assert.match(transactionHex(transaction), /^[a-f0-9]{64}$/);
  assert.equal(transactionHex(TransactionBuilder.fromXDR(transaction.toXDR(), TESTNET_PASSPHRASE)), transactionHex(transaction));
});
test('self-payment and mismatched payer are refused', () => {
  assert.throws(() => buildDirect(new Account(payer, '1'), { address: payer, recipient: payer, amount: '1' }));
  assert.throws(() => buildDirect(new Account(recipient, '1'), { address: payer, recipient, amount: '1' }));
});
test('wallet response must contain the exact reviewed transaction and payer signature', () => {
  const key = Keypair.random();
  const address = key.publicKey();
  const original = buildDirect(new Account(address, '1'), { address, recipient, amount: '5' });
  const prepared = { address, xdr: original.toXDR(), hash: transactionHex(original), expiresAt: Number(original.timeBounds.maxTime) };
  original.sign(key);
  const signed = { signerAddress: address, signedTxXdr: original.toXDR() };
  assert.equal(transactionHex(verifySigned(prepared, signed)), prepared.hash);
  assert.throws(() => verifySigned(prepared, { ...signed, signerAddress: recipient }), /signer/);
  const changed = buildDirect(new Account(address, '1'), { address, recipient, amount: '6' });
  changed.sign(key);
  assert.throws(() => verifySigned(prepared, { ...signed, signedTxXdr: changed.toXDR() }), /differs/);
  assert.throws(() => verifySigned({ ...prepared, hash: '0'.repeat(64) }, signed), /hash changed/);
  assert.throws(() => verifySigned({ ...prepared, expiresAt: 1 }, signed), /expired/);
});
test('unsigned and wrong-key signatures cannot be submitted by the wallet flow', () => {
  const key = Keypair.random();
  const address = key.publicKey();
  const original = buildDirect(new Account(address, '1'), { address, recipient, amount: '5' });
  const prepared = { address, xdr: original.toXDR(), hash: transactionHex(original), expiresAt: Number(original.timeBounds.maxTime) };
  assert.throws(() => verifySigned(prepared, { signerAddress: address, signedTxXdr: original.toXDR() }), /no valid/);
  original.sign(Keypair.random());
  assert.throws(() => verifySigned(prepared, { signerAddress: address, signedTxXdr: original.toXDR() }), /no valid/);
});

const pageFiles = ['index.html', 'send.html', 'receive.html', 'recovery.html', 'developers.html'];
for (const file of pageFiles) {
  test(`${file} has independent content and complete shared navigation`, async () => {
    const html = await readFile(new URL(`./${file}`, import.meta.url), 'utf8');
    assert.doesNotMatch(html, /undefined/);
    assert.match(html, /<title>[^<]+ — Keygate<\/title>/);
    assert.equal((html.match(/aria-current="page"/g) ?? []).length, 1);
    for (const target of pageFiles) assert.ok(html.includes(`href="./${target}"`), target);
    const ids = [...html.matchAll(/\bid="([^"]+)"/g)].map(([, id]) => id);
    assert.equal(new Set(ids).size, ids.length, 'IDs must be unique');
    assert.equal(html.includes('id="transfer-form"'), file === 'send.html');
    assert.equal(html.includes('id="review-dialog"'), file === 'send.html');
    assert.equal(html.includes('id="receive-address"'), file === 'receive.html');
    assert.equal(html.includes('id="history-list"'), file === 'receive.html');
    assert.equal(html.includes('id="bridge-hash"'), file === 'recovery.html');
    for (const [, asset] of html.matchAll(/(?:src|href)="\.\/([^"#]+)"/g)) {
      await readFile(new URL(`./${asset}`, import.meta.url));
    }
    assert.doesNotMatch(html, /<script[^>]+src="https?:/);
  });
}
test('motion includes reduced-motion access and native navigation transitions', async () => {
  const css = await readFile(new URL('./motion.css', import.meta.url), 'utf8');
  assert.match(css, /@view-transition\s*\{\s*navigation:\s*auto/);
  assert.match(css, /@media \(prefers-reduced-motion: reduce\)/);
  assert.match(css, /\.js-motion \.reveal \{ opacity: 1; transform: none; \}/);
});
