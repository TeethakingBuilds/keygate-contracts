import test from 'node:test';
import assert from 'node:assert/strict';
import { existsSync } from 'node:fs';
import { readFile } from 'node:fs/promises';
import { Account, Address, Asset, Keypair, nativeToScVal, scValToNative, TransactionBuilder, xdr } from '@stellar/stellar-sdk';
import { buildDirect, buildPolicy, connect, decodeAllowed, decodeRemaining, decodeSpendingPolicy, disconnect, newPaymentReference, restore, signPrepared, supportedWalletIds, transactionHex, validStellarAddress, verifySigned } from './wallet.js';
import { TESTNET_USDC_ISSUER, TESTNET_USDC_SAC, TESTNET_KEYGATE_PAYMENTS, TESTNET_PASSPHRASE, parseAmount, formatUnits, validateEvmAddress, decodeSolanaAddress, circleState, transactionHash, referenceList } from './web-core.js';

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
test('policy payment builds exactly the payments pay invocation', () => {
  const reference = new Uint8Array(32).fill(9);
  const deadline = Math.floor(Date.now() / 1000) + 3600;
  const transaction = buildPolicy(new Account(payer, '12'), { address: payer, recipient, amount: '1.0000001', reference, deadline });
  assert.equal(transaction.source, payer);
  assert.equal(transaction.sequence, '13');
  assert.equal(transaction.signatures.length, 0);
  assert.equal(transaction.operations.length, 1);
  const operation = transaction.operations[0];
  assert.equal(operation.type, 'invokeHostFunction');
  const invocation = operation.func.invokeContract;
  assert.equal(Address.fromScAddress(invocation.contractAddress).toString(), TESTNET_KEYGATE_PAYMENTS);
  assert.equal(invocation.functionName.toString(), 'pay');
  const args = invocation.args.map(scValToNative);
  assert.equal(args[0], payer);
  assert.equal(args[1], recipient);
  assert.equal(args[2], 10000001n);
  assert.deepEqual(Array.from(args[3]), Array.from(reference));
  assert.equal(args[4], BigInt(deadline));
  assert.deepEqual(args[5], []);
});
test('policy transaction is Testnet-bound and time-limited', () => {
  const reference = newPaymentReference();
  assert.equal(reference.length, 32);
  const transaction = buildPolicy(new Account(payer, '1'), { address: payer, recipient, amount: '5', reference, deadline: Math.floor(Date.now() / 1000) + 3600 });
  assert.equal(transaction.networkPassphrase, TESTNET_PASSPHRASE);
  assert.ok(Number(transaction.timeBounds.maxTime) > Date.now() / 1000);
  assert.ok(Number(transaction.timeBounds.maxTime) <= Date.now() / 1000 + 181);
  assert.match(transactionHex(transaction), /^[a-f0-9]{64}$/);
  assert.equal(transactionHex(TransactionBuilder.fromXDR(transaction.toXDR(), TESTNET_PASSPHRASE)), transactionHex(transaction));
});
test('policy payment refuses a short reference, a stale deadline and a self-payment', () => {
  const reference = newPaymentReference();
  const deadline = Math.floor(Date.now() / 1000) + 3600;
  assert.throws(() => buildPolicy(new Account(payer, '1'), { address: payer, recipient, amount: '1', reference: new Uint8Array(31), deadline }), /32 bytes/);
  assert.throws(() => buildPolicy(new Account(payer, '1'), { address: payer, recipient, amount: '1', reference, deadline: Math.floor(Date.now() / 1000) + 8 * 24 * 3600 }), /7 days/);
  assert.throws(() => buildPolicy(new Account(payer, '1'), { address: payer, recipient: payer, amount: '1', reference, deadline }));
  assert.throws(() => buildPolicy(new Account(recipient, '1'), { address: payer, recipient, amount: '1', reference, deadline }));
});
test('policy reads decode the contract shapes, including an absent policy', () => {
  const policy = xdr.ScVal.scvMap([
    new xdr.ScMapEntry({ key: xdr.ScVal.scvSymbol('daily_limit'), val: nativeToScVal(100_000_000n, { type: 'i128' }) }),
    new xdr.ScMapEntry({ key: xdr.ScVal.scvSymbol('monthly_limit'), val: nativeToScVal(1_000_000_000n, { type: 'i128' }) }),
    new xdr.ScMapEntry({ key: xdr.ScVal.scvSymbol('approval_above'), val: nativeToScVal(100_000_000n, { type: 'i128' }) }),
    new xdr.ScMapEntry({ key: xdr.ScVal.scvSymbol('approvers'), val: xdr.ScVal.scvVec([new Address(payer).toScVal()]) }),
    new xdr.ScMapEntry({ key: xdr.ScVal.scvSymbol('approval_threshold'), val: nativeToScVal(0, { type: 'u32' }) }),
    new xdr.ScMapEntry({ key: xdr.ScVal.scvSymbol('allowlist_only'), val: nativeToScVal(false, { type: 'bool' }) }),
  ]);
  assert.deepEqual(decodeSpendingPolicy(policy), {
    dailyLimit: 100_000_000n,
    monthlyLimit: 1_000_000_000n,
    approvalAbove: 100_000_000n,
    approvers: [payer],
    approvalThreshold: 0,
    allowlistOnly: false,
  });
  assert.equal(decodeSpendingPolicy(xdr.ScVal.scvLedgerKeyContractInstance()), null);
  assert.deepEqual(decodeRemaining(xdr.ScVal.scvVec([nativeToScVal(90_000_000n, { type: 'i128' }), nativeToScVal(990_000_000n, { type: 'i128' })])), {
    daily: 90_000_000n,
    monthly: 990_000_000n,
  });
  assert.equal(decodeAllowed(nativeToScVal(true, { type: 'bool' })), true);
  assert.equal(decodeAllowed(nativeToScVal(false, { type: 'bool' })), false);
  assert.throws(() => decodeSpendingPolicy(xdr.ScVal.scvMap([])), /missing field/);
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

/**
 * A stand-in for the Stellar Wallets Kit: the real kit cannot load under
 * Node (its Varial module imports named exports from the freighter-api
 * UMD bundle), so the connect state machine is tested through this fake
 * while the bundle build proves the real kit loads.
 */
function fakeKit({ key, network = { network: 'Testnet', networkPassphrase: TESTNET_PASSPHRASE }, walletName = 'Varial', signWith = key, addressOverride } = {}) {
  const calls = { authModal: 0, getAddress: 0, getNetwork: 0, signTransaction: 0, disconnect: 0 };
  const address = addressOverride ?? key.publicKey();
  const kit = {
    calls,
    selectedModule: { productName: walletName },
    async authModal() { calls.authModal++; return { address }; },
    async getAddress() { calls.getAddress++; return { address }; },
    async getNetwork() { calls.getNetwork++; return network; },
    async signTransaction(xdr, { address: signer, networkPassphrase } = {}) {
      calls.signTransaction++;
      const transaction = TransactionBuilder.fromXDR(xdr, networkPassphrase);
      transaction.sign(signWith);
      return { signedTxXdr: transaction.toXDR(), signerAddress: signer };
    },
    async disconnect() { calls.disconnect++; },
  };
  return kit;
}

test('supportedWalletIds lists the eight enabled wallets', () => {
  assert.deepEqual(supportedWalletIds(), ['varial', 'xbull', 'albedo', 'rabet', 'lobstr', 'hana', 'hot-wallet', 'klever']);
});
test('connect opens the wallet picker and returns the connection', async () => {
  const key = Keypair.random();
  const kit = fakeKit({ key });
  const connection = await connect(kit);
  assert.equal(kit.calls.authModal, 1);
  assert.deepEqual(connection, { address: key.publicKey(), walletName: 'Varial', network: 'Testnet', passphrase: TESTNET_PASSPHRASE });
});
test('connect surfaces a closed picker as an error', async () => {
  const key = Keypair.random();
  const kit = fakeKit({ key });
  kit.authModal = async () => { throw { code: -1, message: 'The user closed the modal.' }; };
  await assert.rejects(() => connect(kit), /closed the modal/);
});
test('restore reads the stored connection without opening the picker', async () => {
  const key = Keypair.random();
  const kit = fakeKit({ key });
  const connection = await restore(kit);
  assert.equal(kit.calls.authModal, 0);
  assert.equal(connection.address, key.publicKey());
  assert.equal(connection.walletName, 'Varial');
});
test('restore throws when no wallet was connected', async () => {
  const key = Keypair.random();
  const kit = fakeKit({ key });
  kit.getAddress = async () => { throw { code: -1, message: 'No wallet has been connected.' }; };
  await assert.rejects(() => restore(kit), /No wallet has been connected/);
});
test('signPrepared signs through the kit without reopening the picker', async () => {
  const key = Keypair.random();
  const address = key.publicKey();
  const original = buildDirect(new Account(address, '1'), { address, recipient, amount: '5' });
  const prepared = { address, xdr: original.toXDR(), hash: transactionHex(original), expiresAt: Number(original.timeBounds.maxTime) };
  const kit = fakeKit({ key });
  const transaction = await signPrepared(prepared, kit);
  assert.equal(kit.calls.authModal, 0);
  assert.equal(kit.calls.signTransaction, 1);
  assert.equal(transactionHex(transaction), prepared.hash);
});
test('signPrepared refuses a wallet on another network', async () => {
  const key = Keypair.random();
  const address = key.publicKey();
  const original = buildDirect(new Account(address, '1'), { address, recipient, amount: '5' });
  const prepared = { address, xdr: original.toXDR(), hash: transactionHex(original), expiresAt: Number(original.timeBounds.maxTime) };
  const kit = fakeKit({ key, network: { network: 'Mainnet', networkPassphrase: 'Public Global Stellar Network ; September 2015' } });
  await assert.rejects(() => signPrepared(prepared, kit), /Testnet/);
});
test('signPrepared refuses a changed wallet account', async () => {
  const key = Keypair.random();
  const address = key.publicKey();
  const original = buildDirect(new Account(address, '1'), { address, recipient, amount: '5' });
  const prepared = { address, xdr: original.toXDR(), hash: transactionHex(original), expiresAt: Number(original.timeBounds.maxTime) };
  const kit = fakeKit({ key, addressOverride: Keypair.random().publicKey() });
  await assert.rejects(() => signPrepared(prepared, kit), /changed/);
});
test('disconnect clears the kit session', async () => {
  const kit = fakeKit({ key: Keypair.random() });
  await disconnect(kit);
  assert.equal(kit.calls.disconnect, 1);
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

/**
 * Regression test for issue #23: a kit dependency (randombytes, via the
 * HOT Wallet SDK) reads bare `global` at module scope, and browsers have
 * no `global` — the bundle threw "global is not defined" the moment it
 * loaded on a real phone. The build passes `--define:global=window` to
 * esbuild, so the built bundle must contain no bare `global` identifier
 * (`globalThis` and `.global` property accesses are fine in a browser).
 * Skipped when the bundle has not been built yet; run `pnpm run build:web`.
 */
const bundlePath = new URL('./wallet.bundle.js', import.meta.url);
test('the built wallet bundle has no bare global reference', { skip: !existsSync(bundlePath) && 'wallet.bundle.js not built — run `pnpm run build:web` first' }, async () => {
  const bundle = await readFile(bundlePath, 'utf8');
  const bareGlobal = /(?<![\w$.])global(?![\w$])/g;
  const hits = [...bundle.matchAll(bareGlobal)].slice(0, 3).map((match) => bundle.slice(Math.max(0, match.index - 60), match.index + 80));
  assert.equal(hits.length, 0, `bare global in wallet.bundle.js: ${hits.join(' | ')}`);
});
