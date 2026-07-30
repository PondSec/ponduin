import { describe, expect, it } from 'vitest';
import { normalizeCertificateFingerprint } from './certificateFingerprint';

const COLON_FINGERPRINT =
  '0A:47:E9:7E:7B:2A:FD:AB:B8:41:1B:79:32:C6:F6:2A:05:C4:9F:7E:40:48:B1:BC:25:BD:6C:08:DF:98:0A:EB';
const BASE64_FINGERPRINT = 'Ckfpfnsq/au4QRt5Msb2KgXEn35ASLG8Jb1sCN+YCus=';

describe('normalizeCertificateFingerprint', () => {
  it('normalizes colon-separated hexadecimal fingerprints', () => {
    expect(normalizeCertificateFingerprint(`  ${COLON_FINGERPRINT.toLowerCase()}  `)).toBe(
      COLON_FINGERPRINT
    );
  });

  it.each(['sha256', 'SHA256', 'Sha256'])(
    'normalizes %s base64 fingerprints emitted by Electron',
    (prefix) => {
      expect(normalizeCertificateFingerprint(`${prefix}/${BASE64_FINGERPRINT}`)).toBe(
        COLON_FINGERPRINT
      );
    }
  );

  it('does not reinterpret malformed SHA-256 base64 values', () => {
    expect(normalizeCertificateFingerprint('sha256/not-a-complete-digest')).toBe(
      'SHA256/NOT-A-COMPLETE-DIGEST'
    );
  });
});
