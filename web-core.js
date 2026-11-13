export const TESTNET_HORIZON = 'https://horizon-testnet.stellar.org';
export const TESTNET_RPC = 'https://soroban-testnet.stellar.org';
export const TESTNET_PASSPHRASE = 'Test SDF Network ; September 2015';
export const TESTNET_USDC_ISSUER = 'GBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5';
export const TESTNET_USDC_SAC = 'CBIELTK6YBZJU5UP2WWQEUCYKLPU6AUNZ2BQ4WWFEIE3USCIHMXQDAMA';
export const IRIS_SANDBOX = 'https://iris-api-sandbox.circle.com';

export function parseAmount(value, decimals = 7) {
  if (typeof value !== 'string' || value.length > 48 || !new RegExp(`^(?:0|[1-9]\\d*)(?:\\.\\d{1,7})?$`).test(value)) {
    throw new Error('Enter a positive decimal USDC amount without commas or exponents.');
  }
  const [whole, fraction = ''] = value.split('.');
  if (fraction.length > decimals && /[1-9]/.test(fraction.slice(decimals))) {
    throw new Error(`This route accepts at most ${decimals} non-zero decimal places.`);
  }
  const units = BigInt(whole) * 10n ** BigInt(decimals) + BigInt(fraction.slice(0, decimals).padEnd(decimals, '0'));
  if (units <= 0n || units > 2n ** 127n - 1n) throw new Error('Amount must be positive and within the contract i128 range.');
  return units;
}

export function formatUnits(value, decimals = 7) {
  const units = BigInt(value);
  const whole = units / 10n ** BigInt(decimals);
  const fraction = (units % 10n ** BigInt(decimals)).toString().padStart(decimals, '0').replace(/0+$/, '');
  return `${whole}${fraction ? `.${fraction}` : ''}`;
}

export function validateEvmAddress(value) {
  if (!/^0x[a-fA-F0-9]{40}$/.test(value) || /^0x0{40}$/i.test(value)) {
    throw new Error('Enter a non-zero 20-byte Ethereum/Base address beginning with 0x.');
  }
}

export function decodeSolanaAddress(value) {
  const alphabet = '123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz';
  if (typeof value !== 'string' || value.length < 32 || value.length > 44) throw new Error('Enter a 32-byte Solana base58 address.');
  let number = 0n;
  for (const char of value) {
    const digit = alphabet.indexOf(char);
    if (digit < 0) throw new Error('Solana address contains an invalid base58 character.');
    number = number * 58n + BigInt(digit);
  }
  const bytes = [];
  while (number > 0n) {
    bytes.unshift(Number(number & 255n));
    number >>= 8n;
  }
  for (let index = 0; value[index] === '1'; index++) bytes.unshift(0);
  if (bytes.length !== 32 || bytes.every((byte) => byte === 0)) throw new Error('Solana address must encode 32 non-zero bytes.');
  return new Uint8Array(bytes);
}

export function circleState(message) {
  if (!message || typeof message !== 'object') return { label: 'Not found', attested: false };
  const attested = message.status === 'complete' && /^0x(?:[0-9a-f]{2})+$/i.test(message.attestation ?? '') && /^0x(?:[0-9a-f]{2})+$/i.test(message.message ?? '');
  if (message.forwardState === 'FAILED') return { label: 'Forwarding failed — recovery required', attested };
  if (message.forwardState === 'COMPLETE') return { label: 'Circle reports forwarded — verify destination transaction', attested };
  if (attested) return { label: 'Attestation ready — destination settlement not verified', attested };
  return { label: 'Waiting for Circle attestation', attested: false };
}

export function transactionHash(value) {
  if (!/^[0-9a-f]{64}$/i.test(value)) throw new Error('Enter a 64-character Stellar transaction hash.');
  return value.toLowerCase();
}

export function referenceList(value) {
  if (!Array.isArray(value)) return [];
  return value.filter((entry) => entry && typeof entry.account === 'string' && /^G[A-Z2-7]{55}$/.test(entry.account) && typeof entry.hash === 'string' && /^[0-9a-f]{64}$/i.test(entry.hash)).slice(0, 30).map(({ account, hash }) => ({ account, hash: hash.toLowerCase() }));
}
