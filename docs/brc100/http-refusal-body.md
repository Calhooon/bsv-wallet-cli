# Specify the body an HTTP realisation serves for a refusal, including a request the wallet cannot read

Repository: bsv-blockchain/BRCs

Prepared for filing by Calhooon (owner). Draft only; no issue was created. bsv-wallet-cli #122 item 3: the one divergence row of items 3 to 7 where BRC-100 and the MetaNet Client disagree. The filing is the captain's, on the owner's cadence.

BRC-100 gives every error one structure (`status`, `code`, `description`, optional `context`) and asks a parameter error to carry a code and a description naming the parameter, then leaves to each realisation "how these errors are communicated". The HTTP realisations answer differently, and a request the wallet cannot read (a body that does not parse, a required field missing) shows it most plainly. The MetaNet Client answers every wallet failure as HTTP 400 with `{message}`: no code and no description. A body sent as JSON that does not parse never reaches its wallet; Express's default handler answers it with an HTML page (`express.json()` with no error middleware). The SDK's `HTTPWalletJSON` parses a refusal's body as JSON before it reads the status. It reads a typed error only from a 400 carrying `isError` (the review-actions code 5), and turns any other refusal into `failed with HTTP status N`. A code or description the server did send is dropped, and a non-JSON body becomes a parse failure. So a client cannot read which parameter was refused from any HTTP wallet. Please specify the HTTP JSON substrate's refusal body: its members (the BRC's `code` and `description`, or the deployed `message`), the status, and the answer for a request that does not parse or lacks a required parameter. This asks for a clarification of the transport; it does not claim that the BRC already requires one realisation's choice.

Vector (one request each; the BRC's structure, the MetaNet Client's answer as read from its source, this wallet's answer as recorded):

| request to `POST /createSignature` | BRC-100 asks | MetaNet Client (b791cb2) | bsv-wallet-cli (#122 item 3) |
|---|---|---|---|
| `{"data": [1]}` (no `protocolID`) | `status: "error"`, a missing-parameter code, a description naming `protocolID` | 400 `{"message": "<the toolbox's validation text>"}` | 400 `{"code": "VALIDATION_ERROR", "message": "Failed to deserialize the JSON body into the target type: missing field `protocolID` at line 1 column 12"}` |
| `{"data": [1` as `application/json` | an error with a code and a description | 400, Express's HTML error page, before the wallet | 400 `{"code": "VALIDATION_ERROR", "message": "Failed to parse the request body as JSON: data: EOF while parsing a list at line 1 column 11"}` |
| a valid body as `text/plain` | not addressed | read as text and parsed by the wallet: answered as the call | 200, the call's answer (`{"signature": [...]}`), read as the client reads it; 400 `{"code": "VALIDATION_ERROR", "message": "Expected request with `Content-Type: application/json`"}` before #122 item 3's content-type case |
| the same body with no content type | not addressed | no body parsed; the call runs with `{}`: 400 `{"message": "<the toolbox's validation text>"}` | 400 `{"code": "VALIDATION_ERROR", "message": "Failed to deserialize the JSON body into the target type: missing field `protocolID` at line 1 column 2"}`, the call's answer to `{}` |

Witness:

- [tests/wire/031-createSignature.missingField.json](https://github.com/Calhooon/bsv-wallet-cli/blob/0d5b7f472ebf0a2809c2d3653e9c2c0076c59086/tests/wire/031-createSignature.missingField.json)
- [tests/wire/032-createSignature.malformedJson.json](https://github.com/Calhooon/bsv-wallet-cli/blob/0d5b7f472ebf0a2809c2d3653e9c2c0076c59086/tests/wire/032-createSignature.malformedJson.json)
- [tests/wire/177-createSignature.textPlain.json](https://github.com/Calhooon/bsv-wallet-cli/blob/cf5c5deefbab864efa8dae14c0e35c5112e588a1/tests/wire/177-createSignature.textPlain.json)
- [tests/wire/178-createSignature.noContentType.json](https://github.com/Calhooon/bsv-wallet-cli/blob/cf5c5deefbab864efa8dae14c0e35c5112e588a1/tests/wire/178-createSignature.noContentType.json)

Pinned reference reading:

- [bsv-desktop@b791cb2 electron/httpServer.ts:88-90](https://github.com/bsv-blockchain/bsv-desktop/blob/b791cb2b571d62f23256e97abf97557739278a49/electron/httpServer.ts#L88): `express.json()`, then `express.text({ type: '*/*' })`, and no error middleware.
- [bsv-desktop@b791cb2 electron/httpServer.ts:183-189](https://github.com/bsv-blockchain/bsv-desktop/blob/b791cb2b571d62f23256e97abf97557739278a49/electron/httpServer.ts#L183): the body forwarded as the string, or `JSON.stringify(req.body)` (`{}` when nothing parsed it), and `JSON.parse`d by every call (`src/walletByteJson.ts:140`).
- [bsv-desktop@b791cb2 src/onWalletReady.ts:579-599](https://github.com/bsv-blockchain/bsv-desktop/blob/b791cb2b571d62f23256e97abf97557739278a49/src/onWalletReady.ts#L579): `createSignature`'s `catch` answers 400 `{message}`, as every call's does.
- [ts-stack@edf6e03da packages/sdk/src/wallet/substrates/HTTPWalletJSON.ts:319-340](https://github.com/bsv-blockchain/ts-stack/blob/edf6e03dad39bd7bc4fc6e73e64de3a2b6f805c9/packages/sdk/src/wallet/substrates/HTTPWalletJSON.ts#L319): the body is parsed before `res.ok`; only 400 with `isError` is read as typed.

BRC reading: [BRCs@5119a9b51 wallet/0100.md:289-298,312-317](https://github.com/bsv-blockchain/BRCs/blob/5119a9b5148e9ec90eb4865b16f9f732a29703df/wallet/0100.md#L289).

Evidence status: source-read at the pins in this lane. The MetaNet Client was not run (its checkout on this host has no installed dependencies), and the HTML page is Express's documented default for an unhandled error, not a recorded answer. The linked fixtures are the reproducible witness of this wallet's answer. The draft contains no key literal.

Related: bsv-wallet-cli #149 (this wallet serves `message` where the BRC names `description`, and no `status`) and #117 (the refusal statuses the SDK reads). This draft asks the BRC the question those rows wait on and decides neither.
