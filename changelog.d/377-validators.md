- **Key arguments read by the reference's rule (#377, #386):** the doors'
  `counterparty` and `verifier` arguments are read by bsv-rs 0.9.0-beta.6's
  `wallet::validate_public_key` (bsv-rs #579), the reference wallet's
  `validatePublicKey`, in place of the host's strict reader. A bad key is
  400 `VALIDATION_ERROR` in the reference's words (`The counterparty
  parameter must be hexadecimal string.`), where it was 400 `ERROR` with the
  hex parser's text; an uncompressed `04` key is now refused and surrounding
  whitespace is trimmed, as the reference does.
