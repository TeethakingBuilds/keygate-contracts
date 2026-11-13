import { TESTNET_PASSPHRASE, IRIS_SANDBOX, parseAmount, formatUnits, validateEvmAddress, decodeSolanaAddress, circleState, transactionHash, referenceList } from './web-core.js';

const $ = (id) => document.getElementById(id);
const menuToggle = document.querySelector('.menu-toggle');
const navLinks = $('nav-links');
const navConnect = $('nav-connect');
const consoleConnect = $('console-connect');
const amountInput = $('transfer-amount');
const recipientInput = $('recipient-address');
const transferAction = $('transfer-action');
const dialog = $('review-dialog');
const reviewSign = $('review-sign');
let transferMode = 'direct';
let destination = { name: 'Ethereum', domain: '0', addressLabel: 'Ethereum address' };
let wallet = null;
let sdkPromise;
let prepared = null;
let signing = false;
let connecting = false;
let preparing = false;
let currentProof = null;
let connectionVersion = 0;
let references = [];
let intentVersion = 0;
let proofVersion = 0;
const ACCESS_FLAG = 'keygate:freighter:granted';
try { references = referenceList(JSON.parse(localStorage.getItem('keygate:testnet:references') ?? '[]')); } catch { references = []; }

function hasGrantedAccess() {
  try { return sessionStorage.getItem(ACCESS_FLAG) === 'true'; } catch { return false; }
}
function rememberAccess(granted) {
  try {
    if (granted) sessionStorage.setItem(ACCESS_FLAG, 'true');
    else sessionStorage.removeItem(ACCESS_FLAG);
  } catch { /* Storage restrictions must not manufacture a connection. */ }
}
function setText(id, text) { const element = $(id); if (element) element.textContent = text; }
function setDisabled(id, disabled) { const element = $(id); if (element) element.disabled = disabled; }
function setHidden(id, hidden) { const element = $(id); if (element) element.hidden = hidden; }
function setProgress(step, confirmed = false) {
  const steps = ['connect', 'prepare', 'review', 'confirm'];
  const current = steps.indexOf(step);
  document.querySelectorAll('#send-progress [data-step]').forEach((item) => {
    const index = steps.indexOf(item.dataset.step);
    item.classList.toggle('active', index === current && !confirmed);
    item.classList.toggle('complete', index < current || (confirmed && index === current));
    if (index === current && !confirmed) item.setAttribute('aria-current', 'step');
    else item.removeAttribute('aria-current');
  });
}
function invalidateIntent() {
  intentVersion++;
  prepared = null;
  if (!signing) setProgress(wallet && isTestnet() ? 'prepare' : 'connect');
}
amountInput?.addEventListener('input', invalidateIntent);
recipientInput?.addEventListener('input', invalidateIntent);
$('bridge-hash')?.addEventListener('input', () => {
  proofVersion++;
  currentProof = null;
  setDisabled('download-attestation', true);
  setHidden('attestation-details', true);
  setText('attestation-status', 'Hash changed. Query Circle again; no status is assumed.');
});

menuToggle?.addEventListener('click', () => {
  if (navLinks) menuToggle.setAttribute('aria-expanded', String(navLinks.classList.toggle('open')));
});
navLinks?.querySelectorAll('a, button').forEach((control) => control.addEventListener('click', closeMenu));
function closeMenu() {
  navLinks?.classList.remove('open');
  menuToggle?.setAttribute('aria-expanded', 'false');
}

function selectFlow(tab) {
  document.querySelectorAll('.flow-tab').forEach((candidate) => candidate.setAttribute('aria-selected', String(candidate === tab)));
  document.querySelectorAll('.flow-view').forEach((view) => {
    view.hidden = view.id !== tab.dataset.flow;
    view.classList.toggle('active', !view.hidden);
  });
}
document.querySelectorAll('.flow-tab').forEach((tab) => tab.addEventListener('click', () => selectFlow(tab)));
document.querySelectorAll('.route-button').forEach((button) => button.addEventListener('click', () => {
  document.querySelectorAll('.route-button').forEach((candidate) => candidate.classList.toggle('active', candidate === button));
  document.querySelectorAll('[data-route-domain]').forEach((domain) => { domain.textContent = button.dataset.domain; });
  document.querySelectorAll('[data-route-name]').forEach((name) => { name.textContent = button.dataset.name; });
}));

document.querySelectorAll('.transfer-tab').forEach((tab) => tab.addEventListener('click', () => {
  if (!recipientInput) return;
  transferMode = tab.dataset.transferMode;
  document.querySelectorAll('.transfer-tab').forEach((candidate) => candidate.setAttribute('aria-selected', String(candidate === tab)));
  recipientInput.value = '';
  invalidateIntent();
  updateTransferCopy();
}));
document.querySelectorAll('.destination-button').forEach((button) => button.addEventListener('click', () => {
  if (!recipientInput) return;
  document.querySelectorAll('.destination-button').forEach((candidate) => {
    candidate.classList.toggle('active', candidate === button);
    candidate.setAttribute('aria-checked', String(candidate === button));
  });
  destination = { name: button.dataset.destination, domain: button.dataset.domain, addressLabel: button.dataset.addressLabel };
  recipientInput.value = '';
  invalidateIntent();
  updateTransferCopy();
}));
document.querySelectorAll('.amount-preset').forEach((button) => button.addEventListener('click', () => {
  if (!amountInput) return;
  amountInput.value = button.dataset.amount;
  invalidateIntent();
  amountInput.focus();
}));
$('payment-execution')?.addEventListener('change', updateTransferCopy);
$('transfer-form')?.addEventListener('submit', (event) => { event.preventDefault(); void handleTransferAction(); });
navConnect?.addEventListener('click', () => { void connectWallet(); });
consoleConnect?.addEventListener('click', () => { void connectWallet(); });
transferAction?.addEventListener('click', handleTransferAction);
$('disconnect-wallet')?.addEventListener('click', disconnectWallet);
$('copy-address')?.addEventListener('click', async () => {
  if (!wallet || !isTestnet()) return;
  try {
    await navigator.clipboard.writeText(wallet.address);
    setText('receive-status', 'Testnet public address copied. Confirm the official USDC asset and trustline before receiving.');
  } catch { setText('receive-status', 'Clipboard unavailable. Select and copy the displayed public address.'); }
});
$('destination-connect')?.addEventListener('click', connectDestination);
$('refresh-quote')?.addEventListener('click', loadCircleQuote);
$('lookup-transaction')?.addEventListener('click', async () => {
  if ($('lookup-transaction').disabled || !$('history-hash') || !$('history-list')) return;
  $('lookup-transaction').disabled = true;
  try {
    const hash = transactionHash($('history-hash').value.trim());
    setText('history-status', `Querying ${hash}…`);
    await displayTransaction(hash);
    setText('history-status', `Ledger lookup returned for ${hash}. This does not identify the transaction as a Keygate payment.`);
  } catch (error) { setText('history-status', `${describe(error)} Outcome unknown; saved references are unchanged. Query the ledger before retrying.`); }
  finally { $('lookup-transaction').disabled = false; }
});
$('check-attestation')?.addEventListener('click', loadAttestation);
$('download-attestation')?.addEventListener('click', () => {
  if (!currentProof) return;
  const blob = new Blob([JSON.stringify(currentProof, null, 2)], { type: 'application/json' });
  const url = URL.createObjectURL(blob);
  const link = document.createElement('a');
  link.href = url;
  link.download = `circle-testnet-${currentProof.sourceTransaction}.json`;
  link.click();
  URL.revokeObjectURL(url);
});
$('review-accept')?.addEventListener('change', () => { setDisabled('review-sign', !$('review-accept').checked || signing); });
$('review-cancel')?.addEventListener('click', () => { if (!signing) dialog?.close(); });
dialog?.addEventListener('cancel', (event) => { if (signing) event.preventDefault(); });
dialog?.addEventListener('close', () => { invalidateIntent(); });
reviewSign?.addEventListener('click', signPayment);

function sdk() { sdkPromise ??= import('./wallet.bundle.js'); return sdkPromise; }
function isTestnet() { return wallet?.passphrase === TESTNET_PASSPHRASE; }
function setMessage(message, tone = '') {
  const element = $('app-message') ?? $('page-message');
  if (!element) return;
  element.textContent = message;
  element.classList.add('app-message');
  element.classList.toggle('error', tone === 'error');
  element.classList.toggle('success', tone === 'success');
}
function shorten(address) { return `${address.slice(0, 6)}…${address.slice(-6)}`; }
function describe(error) { return error instanceof Error ? error.message : 'Request failed.'; }
function setConnectBusy(busy) {
  connecting = busy;
  setDisabled('nav-connect', busy || signing);
  setDisabled('console-connect', busy || signing);
  setDisabled('transfer-action', busy || signing || preparing);
  setDisabled('disconnect-wallet', busy || signing || wallet === null);
  if (busy) { setText('nav-connect', 'Connecting…'); setText('console-connect', 'Connecting…'); }
  else {
    setText('nav-connect', wallet ? shorten(wallet.address) : 'Connect wallet');
    setText('console-connect', wallet ? 'Refresh' : 'Connect Freighter');
  }
}

async function connectWallet({ restore = false } = {}) {
  if (connecting || signing || (restore && !hasGrantedAccess())) return;
  const version = ++connectionVersion;
  invalidateIntent();
  setMessage(restore ? 'Checking previously granted Freighter access…' : 'Waiting for Freighter access approval…');
  setConnectBusy(true);
  try {
    const api = await sdk();
    if (version !== connectionVersion) return;
    const connection = await (restore ? api.restore() : api.connect());
    if (version !== connectionVersion) return;
    wallet = connection;
    if (!restore) rememberAccess(true);
    prepared = null;
    renderWallet();
    if (!isTestnet()) {
      setMessage('Mainnet signing is disabled. Switch Freighter to Stellar Testnet and reconnect.', 'error');
      return;
    }
    setProgress('prepare');
    if ($('wallet-balance')) {
      setMessage('Connected to Testnet. Reading native USDC from the official asset contract…');
      await refreshBalance(version);
    }
    if (version === connectionVersion) setMessage(restore ? 'Freighter access restored on Testnet. No transaction has been signed or submitted.' : transferAction ? 'Wallet connected. Choose a route and review before signing. Policy/CCTP execution still needs a verified deployment.' : 'Wallet connected to Stellar Testnet. No transaction has been signed or submitted.', 'success');
  } catch (error) {
    if (version !== connectionVersion) return;
    wallet = null;
    rememberAccess(false);
    renderWallet();
    setMessage(restore ? `Freighter access could not be restored: ${describe(error)} Disconnected; use Connect wallet to try again.` : describe(error), 'error');
  } finally { if (version === connectionVersion) setConnectBusy(false); }
}

function disconnectWallet() {
  if (signing) return;
  connectionVersion++;
  wallet = null;
  invalidateIntent();
  rememberAccess(false);
  setConnectBusy(false);
  renderWallet();
  setMessage('Disconnected for this browser session. Revoke site access in Freighter if desired.');
}
function renderWallet() {
  $('wallet-dot')?.classList.toggle('live', wallet !== null && isTestnet());
  navConnect?.classList.toggle('connected', wallet !== null);
  setText('wallet-address', wallet ? shorten(wallet.address) : 'No wallet connected');
  setText('wallet-detail', wallet ? `Freighter · ${wallet.network}` : 'Freighter · Testnet required');
  setText('wallet-balance', wallet ? (isTestnet() ? 'Reading SAC…' : 'Testnet only') : 'Connect wallet');
  setText('console-network', wallet ? wallet.network.toLowerCase() : 'testnet');
  setText('nav-connect', wallet ? shorten(wallet.address) : 'Connect wallet');
  setText('console-connect', wallet ? 'Refresh' : 'Connect Freighter');
  setText('receive-address', wallet && isTestnet() ? wallet.address : 'Connect a Testnet wallet to display your address.');
  setDisabled('copy-address', !isTestnet());
  setDisabled('disconnect-wallet', connecting || signing || wallet === null);
  updateTransferCopy();
}
async function refreshBalance(version = connectionVersion) {
  const address = wallet?.address;
  if (!$('wallet-balance') || !address || !isTestnet()) return;
  try {
    const balance = await (await sdk()).usdcBalance(address);
    if (version === connectionVersion && wallet?.address === address) setText('wallet-balance', formatUnits(balance));
  } catch {
    if (version === connectionVersion && wallet?.address === address) setText('wallet-balance', 'Unavailable');
  }
}
function updateTransferCopy() {
  if (!recipientInput || !transferAction || !$('payment-execution')) return;
  const bridge = transferMode === 'bridge';
  const directSac = !bridge && $('payment-execution').value === 'sac';
  setHidden('destination-field', !bridge);
  setHidden('execution-field', bridge);
  setHidden('destination-connect', !bridge);
  setHidden('refresh-quote', !bridge);
  setText('transfer-title', bridge ? `Stellar → ${destination.name}` : 'Stellar USDC payment');
  setText('amount-hint', bridge ? 'CCTP accepts 6 non-zero decimal places' : 'Up to 7 decimals for Stellar payments');
  setText('recipient-label', bridge ? destination.addressLabel : 'Stellar recipient');
  recipientInput.placeholder = bridge ? (destination.name === 'Solana' ? 'Base58 address' : '0x…') : 'G…';
  setText('destination-connect', `Connect ${destination.name === 'Solana' ? 'Phantom' : 'EVM wallet'} to fill recipient`);
  setText('quote-route', bridge ? `Stellar → ${destination.name} · domain ${destination.domain}` : 'Stellar → Stellar');
  setText('execution-readout', directSac ? 'Direct SAC · no policy guards' : 'Policy deployment required');
  setText('execution-note', directSac ? 'Direct SAC · Testnet only' : 'Policy path not deployed');
  setText('fee-label', bridge ? 'Circle Standard fee' : 'Network fee (XLM)');
  setText('fee-value', bridge ? 'Refresh live Circle quote' : 'Available after simulation');
  transferAction.textContent = !wallet ? 'Connect wallet to continue' : !isTestnet() ? 'Switch to Testnet' : directSac ? 'Simulate & review Testnet payment' : 'Review preflight';
  invalidateIntent();
}

async function handleTransferAction() {
  if (!amountInput || !recipientInput || !transferAction || !dialog || !$('payment-execution') || !$('review-details') || !$('review-accept') || !reviewSign || !$('review-cancel') || !$('review-status')) return;
  if (connecting || signing || preparing || dialog.open) return;
  if (!wallet) { await connectWallet(); return; }
  if (!isTestnet()) { setMessage('Switch Freighter to Testnet and refresh the connection.', 'error'); return; }
  preparing = true;
  const version = intentVersion;
  try {
    parseAmount(amountInput.value, transferMode === 'bridge' ? 6 : 7);
    if (transferMode === 'bridge') {
      if (destination.name === 'Solana') decodeSolanaAddress(recipientInput.value.trim());
      else validateEvmAddress(recipientInput.value.trim());
      setMessage('Recipient and precision validated locally. CCTP burns are disabled: Keygate deployment and destination claim adapters must be verified first.');
      return;
    }
    const api = await sdk();
    api.validStellarAddress(recipientInput.value.trim());
    if ($('payment-execution').value !== 'sac') {
      setMessage('Policy-routed signing is unavailable until the audited/tested Keygate deployment is configured. Direct SAC is a separate, explicitly unguarded Testnet option.');
      return;
    }
    transferAction.disabled = true;
    setMessage('Simulating the exact native USDC SAC call. No transaction has been signed or submitted.');
    const input = { address: wallet.address, recipient: recipientInput.value.trim(), amount: amountInput.value };
    const current = await api.connect();
    if (version !== intentVersion) { setMessage('Payment inputs changed. Prepare the current payment again.'); return; }
    if (current.address !== input.address || current.passphrase !== TESTNET_PASSPHRASE) throw new Error('Wallet or network changed. Reconnect and prepare again.');
    const result = await api.prepareDirect(input);
    if (version !== intentVersion) { setMessage('Payment inputs changed. The old simulation was discarded; prepare again.'); return; }
    if (!wallet || wallet.address !== input.address || !isTestnet()) throw new Error('Wallet changed during preparation.');
    prepared = result;
    setText('fee-value', `${formatUnits(result.fee)} XLM max`);
    showReview(result);
    setMessage('Simulation accepted. Review the asset, recipient, amount and maximum fee before opening Freighter.');
  } catch (error) {
    setMessage(version === intentVersion ? describe(error) : 'Payment inputs changed. Prepare the current payment again.', 'error');
  } finally { preparing = false; transferAction.disabled = connecting || signing; }
}

function showReview(result) {
  if (!dialog || !reviewSign || !$('review-details') || !$('review-status') || !$('review-accept')) return;
  const rows = [ ['Network', 'Stellar Testnet'], ['Path', 'Official USDC SAC · NO Keygate policy'], ['From', result.address], ['To', result.recipient], ['Amount', `${result.amount} native USDC`], ['Maximum network fee', `${formatUnits(result.fee)} XLM`], ['USDC SAC', result.token], ['USDC issuer', result.issuer], ['Expires', new Date(result.expiresAt * 1000).toISOString()] ];
  $('review-details').replaceChildren();
  for (const [label, value] of rows) {
    const term = document.createElement('dt'); term.textContent = label;
    const definition = document.createElement('dd'); definition.textContent = value;
    $('review-details').append(term, definition);
  }
  $('review-status').textContent = 'Nothing has been submitted.';
  $('review-accept').checked = false;
  reviewSign.disabled = true;
  setProgress('review');
  dialog.showModal();
}
async function signPayment() {
  if (!dialog || !reviewSign || !$('review-accept') || !$('review-cancel') || !$('review-status')) return;
  if (!prepared || !$('review-accept').checked || signing) return;
  const snapshot = prepared;
  let submissionAttempted = false;
  signing = true;
  reviewSign.disabled = true;
  $('review-cancel').disabled = true;
  setConnectBusy(false);
  try {
    $('review-status').textContent = 'Approve only this reviewed Testnet transaction in Freighter.';
    const api = await sdk();
    const signed = await api.signPrepared(snapshot);
    setProgress('confirm');
    saveReference(snapshot.address, snapshot.hash);
    $('review-status').textContent = `Submitting ${snapshot.hash}. Do not resend if the response is delayed.`;
    let submission;
    submissionAttempted = true;
    try { submission = await api.submitSigned(signed); }
    catch (error) {
      $('review-status').textContent = `${describe(error)} Outcome unknown: ${snapshot.hash}. Query the ledger before retrying.`;
      setMessage(`Submission not confirmed: ${snapshot.hash}. Query the ledger before any retry.`, 'error');
      await displayTransaction(snapshot.hash);
      return;
    }
    const accepted = submission.status !== 'TRY_AGAIN_LATER';
    $('review-status').textContent = accepted ? `RPC reports ${submission.status}: ${snapshot.hash}. Waiting for ledger confirmation.` : `RPC deferred submission: ${snapshot.hash}. Acceptance is not confirmed; query the ledger before retrying. Checking without resending.`;
    setMessage(accepted ? 'RPC reports pending or duplicate, not settlement. Polling Stellar Testnet RPC.' : `RPC deferred submission: ${snapshot.hash}. Outcome unknown; query the ledger before retrying. Checking the saved hash without resending.`);
    let outcome;
    for (let attempt = 0; attempt < 20; attempt++) {
      outcome = await api.getTransaction(snapshot.hash);
      updateHistoryStatus(snapshot.hash, outcome);
      if (outcome.status !== 'NOT_FOUND') break;
      await new Promise((resolve) => setTimeout(resolve, 2000));
    }
    if (outcome.status === 'SUCCESS') {
      setProgress('confirm', true);
      $('review-status').textContent = `Ledger ${outcome.ledger} confirmed the transaction.`;
      setMessage(`Testnet transaction confirmed in ledger ${outcome.ledger}.`, 'success');
      await refreshBalance();
    } else if (outcome.status === 'FAILED') {
      $('review-status').textContent = 'Ledger reports FAILED. Network fees may still have been charged.';
      setMessage('The ledger reports transaction failure.', 'error');
    } else {
      $('review-status').textContent = `Confirmation unknown: ${snapshot.hash}. Query the ledger before retrying; do not resend blindly.`;
      setMessage(`Confirmation timeout is unknown, not failure: ${snapshot.hash}. Query the ledger before retrying.`);
    }
  } catch (error) {
    const message = submissionAttempted ? `${describe(error)} Outcome unknown: ${snapshot.hash}. Query the ledger before retrying; the transaction may still settle.` : describe(error);
    $('review-status').textContent = message;
    setMessage(message, 'error');
  } finally {
    signing = false;
    prepared = null;
    $('review-cancel').disabled = false;
    setConnectBusy(false);
  }
}

function saveReference(account, hash) {
  references = [{ account, hash }, ...references.filter((entry) => entry.hash !== hash)].slice(0, 30);
  try { localStorage.setItem('keygate:testnet:references', JSON.stringify(references)); } catch { setMessage('Browser storage unavailable. Copy the transaction hash before leaving.'); }
  renderReferences();
}
function renderReferences() {
  const list = $('history-list');
  if (!list) return;
  list.replaceChildren();
  if (references.length === 0) { list.textContent = 'No tracked transactions in this browser.'; return; }
  for (const entry of references) {
    const row = document.createElement('div'); row.className = 'history-item'; row.dataset.hash = entry.hash;
    const text = document.createElement('div');
    const code = document.createElement('code'); code.textContent = entry.hash;
    const status = document.createElement('small'); status.textContent = 'Saved reference — status not checked'; status.dataset.status = '';
    const button = document.createElement('button'); button.className = 'button'; button.textContent = 'Refresh';
    button.addEventListener('click', async () => {
      button.disabled = true;
      try { updateHistoryStatus(entry.hash, await (await sdk()).getTransaction(entry.hash)); }
      catch { status.textContent = 'RPC unavailable — outcome unknown. Query the ledger before retrying.'; }
      finally { button.disabled = false; }
    });
    text.append(code, status); row.append(text, button); list.append(row);
  }
}
function updateHistoryStatus(hash, result) {
  const list = $('history-list');
  if (!list) return;
  const row = [...list.children].find((entry) => entry.dataset.hash === hash);
  const status = row?.querySelector('[data-status]');
  if (!status) return;
  status.textContent = result.status === 'SUCCESS' ? `Ledger-confirmed SUCCESS · ledger ${result.ledger}` : result.status === 'FAILED' ? `Ledger-confirmed FAILED · ledger ${result.ledger}` : 'Not found in RPC retention — outcome unknown, not failed. Query the ledger before retrying.';
}
async function displayTransaction(hash) {
  const list = $('history-list');
  if (!list) return;
  const api = await sdk();
  const outcome = await api.getTransaction(hash);
  if (![...list.children].some((entry) => entry.dataset.hash === hash)) {
    if (!list.querySelector('.history-item')) list.replaceChildren();
    const row = document.createElement('div'); row.className = 'history-item'; row.dataset.hash = hash;
    const text = document.createElement('div'); const code = document.createElement('code'); code.textContent = hash;
    const status = document.createElement('small'); status.dataset.status = ''; text.append(code, status); row.append(text); list.append(row);
  }
  updateHistoryStatus(hash, outcome);
}

async function connectDestination() {
  if (!$('destination-connect') || !recipientInput) return;
  $('destination-connect').disabled = true;
  const selected = { ...destination };
  try {
    let address;
    if (selected.name === 'Solana') {
      const provider = window.phantom?.solana;
      if (!provider?.isPhantom) throw new Error('Phantom Solana wallet not detected. Paste your recipient address instead.');
      const result = await provider.connect();
      address = result.publicKey.toString();
      decodeSolanaAddress(address);
    } else {
      const provider = window.ethereum;
      if (!provider?.request) throw new Error('No injected EVM wallet detected. Paste a recipient address instead.');
      const chain = await provider.request({ method: 'eth_chainId' });
      const required = selected.name === 'Base' ? '0x14a34' : '0xaa36a7';
      if (chain?.toLowerCase() !== required) throw new Error(`Select ${selected.name === 'Base' ? 'Base Sepolia' : 'Ethereum Sepolia'} in your EVM wallet first.`);
      const accounts = await provider.request({ method: 'eth_requestAccounts' });
      address = accounts[0];
      validateEvmAddress(address);
    }
    if (destination.name !== selected.name || transferMode !== 'bridge') return;
    recipientInput.value = address;
    invalidateIntent();
    setMessage(`${selected.name} recipient filled from wallet access. This does not authorize any burn or mint.`, 'success');
  } catch (error) { setMessage(describe(error), 'error'); }
  finally { $('destination-connect').disabled = false; }
}

async function fetchJson(url) {
  const response = await fetch(url, { headers: { Accept: 'application/json' }, signal: AbortSignal.timeout(15000) });
  if (!response.ok) throw new Error(`Provider returned ${response.status}. No fee or status is assumed.`);
  return response.json();
}
async function loadCircleQuote() {
  if (!$('refresh-quote')) return;
  const domain = destination.domain;
  $('refresh-quote').disabled = true;
  setText('fee-value', 'Querying Circle…');
  try {
    const rows = await fetchJson(`${IRIS_SANDBOX}/v2/burn/USDC/fees/27/${domain}?forward=true`);
    const standard = Array.isArray(rows) ? rows.find((row) => row.finalityThreshold === 2000) : null;
    if (!standard || !Number.isFinite(Number(standard.minimumFee)) || Number(standard.minimumFee) < 0) throw new Error('Circle returned no valid Standard-finality quote.');
    if (destination.domain !== domain || transferMode !== 'bridge') return;
    setText('fee-value', `${standard.minimumFee} bps · quoted ${new Date().toLocaleTimeString()}`);
    setMessage('Live Circle Standard fee received. This is not a signed max-fee commitment; forwarding and recipient setup may cost extra. Burn execution is not enabled.');
  } catch (error) {
    if (destination.domain === domain && transferMode === 'bridge') { setText('fee-value', 'Quote unavailable'); setMessage(describe(error), 'error'); }
  } finally { $('refresh-quote').disabled = false; }
}
async function loadAttestation() {
  if (!$('check-attestation') || !$('bridge-hash') || !$('download-attestation') || !$('attestation-details') || !$('attestation-status')) return;
  if ($('check-attestation').disabled) return;
  const version = ++proofVersion;
  let hash;
  currentProof = null;
  $('download-attestation').disabled = true;
  $('attestation-details').hidden = true;
  $('check-attestation').disabled = true;
  try {
    hash = transactionHash($('bridge-hash').value.trim());
    $('attestation-status').textContent = `Querying Circle Iris Sandbox for ${hash} — no state assumed.`;
    const response = await fetchJson(`${IRIS_SANDBOX}/v2/messages/27?transactionHash=${hash}`);
    if (version !== proofVersion || $('bridge-hash').value.trim().toLowerCase() !== hash) return;
    if (!Array.isArray(response.messages) || response.messages.some((message) => !message || typeof message !== 'object')) throw new Error('Unexpected Circle response. Settlement remains unknown.');
    if (response.messages.length === 0) { $('attestation-status').textContent = `Circle has no message for Testnet source ${hash} yet.`; return; }
    const states = response.messages.map(circleState);
    $('attestation-status').textContent = `Source ${hash} · ${states.map((state, index) => `Message ${index + 1}: ${state.label}`).join(' | ')}`;
    $('attestation-details').hidden = false;
    $('attestation-details').textContent = `Source transaction: ${hash}\n${response.messages.map((message, index) => `Message ${index + 1} · Circle status: ${message.status ?? 'unknown'} · forward: ${message.forwardState ?? 'not reported'} · destination tx: ${message.forwardTxHash ?? 'not reported'}`).join('\n')}`;
    if (states.some((state) => state.attested)) {
      currentProof = { provider: IRIS_SANDBOX, sourceDomain: 27, sourceTransaction: hash, messages: response.messages };
      $('download-attestation').disabled = false;
    }
  } catch (error) {
    if (version === proofVersion) $('attestation-status').textContent = `${hash ? `Source ${hash} · ` : ''}${describe(error)} Transfer status unknown. Query the source ledger before retrying any transfer.`;
  } finally { $('check-attestation').disabled = false; }
}

renderReferences();
renderWallet();
if (hasGrantedAccess()) void connectWallet({ restore: true });
