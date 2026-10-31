const certifier = require('../lib/certifier');
module.exports = {
  name: 'certificate-lifecycle',
  description: 'BRC-52 signed certificate, selective proof, trusted discovery and relinquish (0 sats)',
  async run(walletA, walletB, assert) {
    const aKey = await walletA.client.identityKey();
    const bKey = await walletB.client.identityKey();
    const stub = await certifier.start(walletA.client);
    try {
      const issued = await stub.issue(bKey);
      const { type, subject, ...fields } = issued.certificate;
      const args = { type, ...fields, acquisitionProtocol: 'direct',
        keyringRevealer: 'certifier', keyringForSubject: issued.keyring };
      // The differential drives both pins with one request. Each serde reads
      // its own type key; the ordinary beta.2 run sends only the BRC spelling.
      if (process.env.DIFF_RECORD) args.certificateType = type;
      const cert = await walletB.client.post('acquireCertificate', args);
      const legacy = !Object.hasOwn(cert, 'type');
      assert(cert[legacy ? 'certificateType' : 'type'] === certifier.TYPE, 'certificate type is served');
      assert(cert.subject === bKey, 'the certificate names its subject');
      assert(typeof cert.signature === 'string' && /^[0-9a-f]+$/.test(cert.signature), 'signature is DER hex');
      const listed = await walletB.client.listCertificates([aKey], [certifier.TYPE]);
      const entry = listed.certificates.find(c => (c.certificate || c).serialNumber === certifier.SERIAL);
      assert(entry, 'the signed certificate is stored');
      const certificate = entry.certificate || entry;
      const proof = await walletB.client.proveCertificate(certificate, aKey, ['role']);
      assert(Object.keys(proof.keyringForVerifier).join(',') === 'role', 'only the requested field key is revealed');
      const unwrapped = await walletA.client.decrypt(Buffer.from(proof.keyringForVerifier.role, 'base64'),
        [2, 'certificate field encryption'], `${certifier.SERIAL} role`, bKey);
      assert(certifier.decryptField(Buffer.from(unwrapped.plaintext), certificate.fields.role) === certifier.VALUES.role,
        'the verifier decrypts the field with its symmetric key (toolbox #203)');
      const verified = await walletA.client.verifySignature(certifier.unsigned(issued.certificate),
        Buffer.from(cert.signature, 'hex'), [2, 'certificate signature'],
        `${certifier.TYPE} ${certifier.SERIAL}`, 'anyone', true);
      assert(verified.valid === true, 'the certificate signature verifies under the BRC-52 child key');
      // Both calls run behind the proxy wall. Wallet B's TRUST_SETTINGS
      // names wallet A, this certificate's certifier (#331, toolbox #388):
      // identity discovery's local leg answers it with certifierInfo, the
      // configured entry; attribute discovery has no local leg (it asks the
      // overlay, which answers nothing behind the wall), so it stays empty.
      const byIdentity = await walletB.client.discoverByIdentityKey(bKey);
      const byAttributes = await walletB.client.discoverByAttributes({ role: certifier.VALUES.role });
      if (!legacy) {
        assert(byIdentity.totalCertificates === 1 && byIdentity.certificates.length === 1,
          `identity discovery answers the trusted certifier's certificate (#331): ${JSON.stringify(byIdentity)}`);
        const found = byIdentity.certificates[0];
        assert(found.serialNumber === certifier.SERIAL && found.certifier === aKey && found.subject === bKey,
          'the discovered certificate is the one wallet A issued to wallet B');
        const info = found.certifierInfo || {};
        assert(Object.keys(info).sort().join(',') === 'description,iconUrl,name,trust'
          && Object.entries(certifier.INFO).every(([k, v]) => info[k] === v),
          `certifierInfo is the configured entry (#331): ${JSON.stringify(found.certifierInfo)}`);
        assert(byAttributes.totalCertificates === 0 && byAttributes.certificates.length === 0,
          'attribute discovery is empty behind the proxy wall: it has no local leg (toolbox #388)');
      }
      const removed = await walletB.client.post('relinquishCertificate', {
        [legacy ? 'certificateType' : 'type']: certifier.TYPE, serialNumber: certifier.SERIAL, certifier: aKey,
      });
      assert(removed.relinquished === true, 'certificate relinquished');
      const after = await walletB.client.listCertificates([aKey], [certifier.TYPE]);
      assert(after.totalCertificates === 0, 'no certificate remains');
      return { txid: null, sats: 0, wocConfirmed: 'n/a',
        discovery: legacy ? 'beta.1 recorded answer' : 'identity: the trusted certificate; attributes: empty behind the wall (#331)' };
    } finally { await stub.stop(); }
  },
};
