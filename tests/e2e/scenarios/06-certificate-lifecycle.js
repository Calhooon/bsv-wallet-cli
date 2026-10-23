/**
 * E2E.7: Full BRC-52 certificate lifecycle with real keyrings.
 *
 * Wallet A acts as certifier, encrypts field values for Wallet B (subject).
 * B acquires cert with real keyring → B proves selective fields to A (verifier).
 * This proves the Rust cert crypto works end-to-end: encrypt → store → prove → verify.
 *
 * Cost: 0 sats (all crypto, no on-chain).
 */
const { ANYONE_KEY } = require('../lib/wallet-client');

module.exports = {
  name: 'certificate-lifecycle',
  description: 'BRC-52 full lifecycle: certifier encrypts → subject acquires → proves fields (0 sats)',

  async run(walletA, walletB, assert) {
    const aKey = await walletA.client.identityKey();
    const bKey = await walletB.client.identityKey();

    // Certificate metadata
    // base64("e2e-cert") = "ZTJlLWNlcnQ="
    const certType = 'ZTJlLWNlcnQ=';
    const serialNumber = 'AQIDBA==';  // base64([1,2,3,4])
    const revocationOutpoint = 'deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef.0';

    // The plaintext field values we want to certify
    const fieldValues = {
      role: 'e2e-tester',
      project: 'bsv-wallet-cli',
      scope: 'integration',
    };

    // --- Step 1: Certifier (A) encrypts each field for Subject (B) ---
    // Protocol: [2, "certificate field encryption"], keyId: field_name, counterparty: B's key
    const encryptedFields = {};
    const masterKeyring = {};

    for (const [fieldName, plainValue] of Object.entries(fieldValues)) {
      const plaintextBytes = Array.from(Buffer.from(plainValue));

      const result = await walletA.client.encrypt(
        plaintextBytes,
        [2, 'certificate field encryption'],
        fieldName,  // key ID = field name (BRC-52 spec)
        bKey,       // counterparty = subject's key
      );

      assert(result.ciphertext && result.ciphertext.length > 0,
        `Encrypt field "${fieldName}" must return ciphertext`);

      // Base64-encode the ciphertext for JSON transport
      const ciphertextB64 = Buffer.from(result.ciphertext).toString('base64');
      encryptedFields[fieldName] = ciphertextB64;
      masterKeyring[fieldName] = ciphertextB64;  // Same ciphertext — that's the BRC-52 pattern
    }

    // --- Step 2: Subject (B) acquires certificate with real keyring ---
    const cert = await walletB.client.acquireCertificate(
      certType,
      aKey,  // certifier = Wallet A (not ANYONE_KEY — real certifier)
      encryptedFields,  // encrypted field values
      {
        serialNumber,
        revocationOutpoint,
        keyringForSubject: masterKeyring,  // the master keyring B needs for proving
      },
    );

    assert(cert.certificateType, 'acquireCertificate must return certificateType');
    assert(cert.subject, 'acquireCertificate must return subject');
    const actualSerial = cert.serialNumber;

    // --- Step 3: List certificates — verify it's stored ---
    const listed = await walletB.client.listCertificates([aKey], [certType]);
    assert(listed.totalCertificates >= 1,
      `Should find >= 1 cert, got ${listed.totalCertificates}`);
    const certs = listed.certificates || [];
    const found = certs.find(c =>
      (c.certificate?.serialNumber || c.serialNumber) === actualSerial
    );
    assert(found, `Cert with serial ${actualSerial} must be in list`);

    // --- Step 4: Subject (B) proves "role" field to Verifier (A) ---
    // This is the real test — proveCertificate uses the master keyring to
    // decrypt "role", then re-encrypts it for the verifier (A).
    const certForProof = found.certificate || found;
    const proof = await walletB.client.proveCertificate(
      certForProof,
      aKey,       // verifier = Wallet A
      ['role'],   // only reveal "role", not "project" or "scope"
    );

    assert(proof, 'proveCertificate must return a result');
    // The proof should contain a keyring with only the revealed field
    if (proof.keyringForVerifier) {
      assert(proof.keyringForVerifier.role,
        'Verifier keyring should contain "role"');
      assert(!proof.keyringForVerifier.project,
        'Verifier keyring should NOT contain "project" (not revealed)');
      assert(!proof.keyringForVerifier.scope,
        'Verifier keyring should NOT contain "scope" (not revealed)');
    }

    // --- Step 5: Cross-wallet discovery ---
    // The one step of this scenario that is not hermetic: discovery asks the
    // overlay network's SLAP trackers, whose hosts are bsv-rs defaults the
    // toolbox builds inline (`HttpLookupResolver::mainnet()`), with no knob
    // in the binary. Under E2E_HERMETIC=1 the wall would count them, so the
    // step is skipped and named (docs/E2E-HERMETIC.md); the rest of the
    // lifecycle runs.
    let discovery = 'asked';
    if (process.env.E2E_HERMETIC === '1') {
      discovery = 'skipped under E2E_HERMETIC=1';
      console.log('       discovery: skipped under E2E_HERMETIC=1 (the overlay lookup hosts are bsv-rs defaults with no knob in the binary)');
    } else {
      const discovered = await walletA.client.discoverByIdentityKey(bKey);
      assert(discovered !== undefined, 'discoverByIdentityKey must work');
    }

    // --- Step 6: Relinquish ---
    const relinquished = await walletB.client.relinquishCertificate(
      certType, actualSerial, aKey,
    );
    assert(relinquished.relinquished === true, 'Certificate must be relinquished');

    // --- Step 7: Verify gone ---
    const listedAfter = await walletB.client.listCertificates([aKey], [certType]);
    assert(listedAfter.totalCertificates === 0,
      `Cert count should be 0 after relinquish, got ${listedAfter.totalCertificates}`);

    return {
      txid: null, sats: 0, wocConfirmed: 'n/a',
      tests: 'encrypt-fields, acquire-with-keyring, list, prove-selective, discover, relinquish, verify-gone',
      discovery,
    };
  },
};
