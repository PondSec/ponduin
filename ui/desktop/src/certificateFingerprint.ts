import { Buffer } from 'node:buffer';

const SHA256_BYTES = 32;
const SHA256_BASE64_PREFIX = /^sha256\/(.+)$/i;

const toColonSeparatedHex = (bytes: Uint8Array): string =>
  Array.from(bytes)
    .map((byte) => byte.toString(16).padStart(2, '0'))
    .join(':')
    .toUpperCase();

export function normalizeCertificateFingerprint(fingerprint: string): string {
  const trimmed = fingerprint.trim();
  const base64Match = SHA256_BASE64_PREFIX.exec(trimmed);
  if (base64Match) {
    const digest = Buffer.from(base64Match[1], 'base64');
    if (digest.length === SHA256_BYTES) {
      return toColonSeparatedHex(digest);
    }
  }

  return trimmed.toUpperCase();
}
