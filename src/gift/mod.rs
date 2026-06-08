//! TimeLockedGift — on-chain time-locked BSV gift covenant.
//!
//! Pay someone such that they fully own the key (can back it up / email it to
//! themselves) but the network refuses to confirm any spend until `lockUntil`.
//! OP_CHECKLOCKTIMEVERIFY is a NOP on BSV post-Genesis, so the covenant instead
//! reads the spending tx's own nLockTime/nSequence from the sighash preimage and
//! requires `nLockTime >= lockUntil && sequence < 0xffffffff`; consensus then
//! forbids mining until median-time-past passes the lock.
//!
//! The covenant locking script is the sCrypt-compiled `TimeLockedGift` reduced to
//! `PREFIX · push(recipient) · push(lockUntil) · push(amount) · SUFFIX`, proven
//! byte-identical to the compiler over 110 vectors (see `covenant::tests`).

pub mod claim;
pub mod covenant;
mod template_data;
