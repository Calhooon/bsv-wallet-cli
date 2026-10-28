const certifier = require('../lib/certifier');
module.exports = {
  name: 'certificate-lifecycle',
  description: 'BRC-52 signed certificate, selective proof, empty discovery and relinquish (0 sats)',
  async run(walletA, walletB, assert) {
    const aKey = await walletA.client.identityKey();
    const bKey = await walletB.client.identityKey();
    const stub = await certifier.start(walletA.client);
    try {
      let issued = await stub.issue(bKey);
      let legacy = false;
      const acquire = () => {
        const { type, subject, ...fields } = issued.certificate;
        return walletB.client.post('acquireCertificate', {
          [legacy ? 'certificateType' : 'type']: type, ...fields,
          acquisitionProtocol: 'direct', keyringRevealer: 'certifier', keyringForSubject: issued.keyring,
        });
      };
      let cert;
      try { cert = await acquire(); }
      catch (error) {
        // The differential drives beta.1 with this same harness. Only its exact
        // missing-field refusal selects its recorded, pre-#203 scheme.
        if (error.status !== 422 || !error.message.includes('missing field `certificateType`')) throw error;
        legacy = true;
        issued = await stub.issue(bKey, true);
        cert = await acquire();
      }
      assert(cert[legacy ? 'certificateType' : 'type'] === certifier.TYPE, 'certificate type is served');
      assert(cert.subject === bKey, 'the certificate names its subject');
      assert(typeof cert.signature === 'string' && /^[0-9a-f]+$/.test(cert.signature), 'signature is DER hex');
      const listed = await walletB.client.listCertificates([aKey], [certifier.TYPE]);
      const entry = listed.certificates.find(c => (c.certificate || c).serialNumber === certifier.SERIAL);
      assert(entry, 'the signed certificate is stored');
      const certificate = entry.certificate || entry;
      const proof = await walletB.client.proveCertificate(certificate, aKey, ['role']);
      assert(Object.keys(proof.keyringForVerifier).join(',') === 'role', 'only the requested field key is revealed');
      if (!legacy) {
        const unwrapped = await walletA.client.decrypt(Buffer.from(proof.keyringForVerifier.role, 'base64'),
          [2, 'certificate field encryption'], `${certifier.SERIAL} role`, bKey);
        assert(certifier.decryptField(Buffer.from(unwrapped.plaintext), certificate.fields.role) === certifier.VALUES.role,
          'the verifier decrypts the field with its symmetric key (toolbox #203)');
        const verified = await walletA.client.verifySignature(certifier.unsigned(issued.certificate),
          Buffer.from(cert.signature, 'hex'), [2, 'certificate signature'],
          `${certifier.TYPE} ${certifier.SERIAL}`, 'anyone', true);
        assert(verified.valid === true, 'the certificate signature verifies under the BRC-52 child key');
      }
      // Both calls run behind the proxy wall. Published trust lookup returns
      // None: toolbox #388 (CLI #331), never fabricated trust.
      for (const result of [await walletB.client.discoverByIdentityKey(bKey),
        await walletB.client.discoverByAttributes({ role: certifier.VALUES.role })]) {
        if (!legacy) assert(result.totalCertificates === 0 && result.certificates.length === 0,
          'discovery is empty: toolbox #388 (CLI #331)');
      }
      const removed = await walletB.client.post('relinquishCertificate', {
        [legacy ? 'certificateType' : 'type']: certifier.TYPE, serialNumber: certifier.SERIAL, certifier: aKey,
      });
      assert(removed.relinquished === true, 'certificate relinquished');
      const after = await walletB.client.listCertificates([aKey], [certifier.TYPE]);
      assert(after.totalCertificates === 0, 'no certificate remains');
      return { txid: null, sats: 0, wocConfirmed: 'n/a',
        discovery: legacy ? 'beta.1 recorded answer' : 'empty: toolbox #388 (CLI #331)' };
    } finally { await stub.stop(); }
  },
};
