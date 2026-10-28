// The subject imports this local certifier's answer through direct acquisition.
// The published issuance URL guard requires public HTTPS, so the hermetic
// test does not send that door to a loopback HTTP endpoint.
const http = require('http');
const crypto = require('crypto');
const TYPE = Buffer.alloc(32, 6).toString('base64');
const SERIAL = Buffer.alloc(32, 7).toString('base64');
const REVOCATION = `${'de'.repeat(32)}.0`;
const VALUES = { role: 'e2e-tester', project: 'bsv-wallet-cli', scope: 'integration' };
function sized(text) {
  const bytes = Buffer.from(text);
  if (bytes.length >= 253) throw new Error('the certifier fixture expects short fields');
  return Buffer.concat([Buffer.from([bytes.length]), bytes]);
}
// BRC-52, Certificate::to_binary(false): values are signed base64 text.
function unsigned(c) {
  const fields = Object.keys(c.fields).sort((a, b) => a.localeCompare(b));
  return Buffer.concat([
    Buffer.from(c.type, 'base64'), Buffer.from(c.serialNumber, 'base64'),
    Buffer.from(c.subject, 'hex'), Buffer.from(c.certifier, 'hex'),
    Buffer.from(c.revocationOutpoint.split('.')[0], 'hex'), Buffer.from([0, fields.length]),
    ...fields.flatMap(k => [sized(k), sized(c.fields[k])]),
  ]);
}
function fieldCipher(name, value) {
  // Fixed fixture material, only for the local test's public plaintext.
  const key = crypto.createHash('sha256').update(`e2e certificate key ${name}`).digest();
  const iv = crypto.createHash('sha256').update(`e2e certificate iv ${name}`).digest();
  const cipher = crypto.createCipheriv('aes-256-gcm', key, iv);
  return { key, ciphertext: Buffer.concat([iv, cipher.update(value), cipher.final(), cipher.getAuthTag()]) };
}
async function issue(client, subject, legacy) {
  const certifier = await client.identityKey();
  const fields = {}, keyring = {};
  for (const [name, value] of Object.entries(VALUES)) {
    const field = fieldCipher(name, value);
    const encrypted = await client.encrypt(legacy ? Buffer.from(value) : field.key,
      [2, 'certificate field encryption'], name, subject);
    keyring[name] = Buffer.from(encrypted.ciphertext).toString('base64');
    fields[name] = legacy ? keyring[name] : field.ciphertext.toString('base64');
  }
  const certificate = { type: TYPE, serialNumber: SERIAL, subject, certifier,
    revocationOutpoint: REVOCATION, fields };
  const signed = await client.createSignature(unsigned(certificate),
    [2, 'certificate signature'], `${TYPE} ${SERIAL}`, 'anyone');
  certificate.signature = Buffer.from(signed.signature).toString('hex');
  return { certificate, keyring };
}
async function start(client) {
  const server = http.createServer(async (req, res) => {
    try {
      let body = '';
      for await (const chunk of req) body += chunk;
      const { subject, legacy = false } = JSON.parse(body);
      const issued = await issue(client, subject, legacy);
      res.writeHead(200, { 'content-type': 'application/json',
        'x-bsv-auth-identity-key': issued.certificate.certifier });
      res.end(JSON.stringify(issued));
    } catch (error) { res.writeHead(500); res.end(String(error)); }
  });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  return {
    async issue(subject, legacy = false) {
      const response = await fetch(`http://127.0.0.1:${server.address().port}`, {
        method: 'POST', body: JSON.stringify({ subject, legacy }),
      });
      if (!response.ok) throw new Error(await response.text());
      const issued = await response.json();
      if (response.headers.get('x-bsv-auth-identity-key') !== issued.certificate.certifier)
        throw new Error('the certifier identity header must name the signer');
      return issued;
    },
    stop: () => new Promise(resolve => server.close(resolve)),
  };
}
function decryptField(key, encoded) {
  const bytes = Buffer.from(encoded, 'base64');
  const cipher = crypto.createDecipheriv('aes-256-gcm', key, bytes.subarray(0, 32));
  cipher.setAuthTag(bytes.subarray(-16));
  return Buffer.concat([cipher.update(bytes.subarray(32, -16)), cipher.final()]).toString();
}
module.exports = { start, TYPE, SERIAL, VALUES, unsigned, decryptField };
