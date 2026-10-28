/**
 * E2E.13: Key linkage reveal between wallets (BRC-69 / BRC-70).
 * Tests revealCounterpartyKeyLinkage and revealSpecificKeyLinkage
 * with real wallet identity keys.
 * Cost: 0 sats (crypto only)
 */

module.exports = {
  name: 'key-linkage',
  description: 'Key linkage reveal between wallets (BRC-69/BRC-70)',

  async run(walletA, walletB, assert) {
    const aKey = await walletA.client.identityKey();
    const bKey = await walletB.client.identityKey();

    // --- BRC-69: revealCounterpartyKeyLinkage ---
    // A reveals its key linkage with B to B (B is both counterparty and verifier)
    const linkageResp = await walletA.client.post('revealCounterpartyKeyLinkage', {
      counterparty: bKey,
      verifier: bKey,
    });

    const linkage = linkageResp.linkage || linkageResp; // beta.1 / beta.2 (#105)
    assert(linkage, 'counterparty linkage is present');
    assert(linkage.prover, 'linkage should include prover');
    assert(linkage.verifier, 'linkage should include verifier');
    assert(linkage.counterparty, 'linkage should include counterparty');
    assert(linkage.encryptedLinkage, 'linkage should include encryptedLinkage');
    assert(linkage.encryptedLinkageProof, 'linkage should include encryptedLinkageProof');
    assert(linkageResp.revelationTime, 'response should include revelationTime');

    // --- BRC-70: revealSpecificKeyLinkage ---
    const specificResp = await walletA.client.post('revealSpecificKeyLinkage', {
      counterparty: bKey,
      verifier: bKey,
      protocolID: [2, 'e2e linkage test'],
      keyID: '1',
    });

    const specific = specificResp.linkage || specificResp;
    assert(specific, 'specific linkage is present');
    assert(specific.prover, 'specific linkage should include prover');
    assert(specific.verifier, 'specific linkage should include verifier');
    assert(specific.counterparty, 'specific linkage should include counterparty');
    assert(specific.encryptedLinkage, 'specific linkage should include encryptedLinkage');
    assert(specificResp.protocolID || specificResp.protocol, 'specific linkage includes its protocol');

    // --- Invalid input: bad counterparty key should return a clear error ---
    try {
      await walletA.client.post('revealCounterpartyKeyLinkage', {
        counterparty: 'not-a-valid-pubkey',
        verifier: bKey,
      });
      assert(false, 'bad counterparty should have thrown');
    } catch (e) {
      assert(
        e.message.includes('400') || e.message.includes('error'),
        `bad counterparty should return error, got: ${e.message}`,
      );
    }

    return { txid: null, wocConfirmed: 'n/a', sats: 0 };
  },
};
