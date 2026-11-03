//! A power: one grant row's instance, of one of the four kinds (#414, the
//! design's section (1) under W1 (a)), and its key's canonical text.
//!
//! The key is a compact JSON array written by `serde_json::to_string` of
//! a fixed tuple per kind, so every surface prints one string and the
//! unique index compares bytes: a protocol `[privileged,level,"name",
//! "counterparty"]`, a label `[false,1,"action label <label>",""]`, a
//! basket `["name"]`, a certificate `[privileged,"type","verifier",
//! ["field",...]]`, spending `[]`. Every name passes its bsv-rs validator
//! (`v0.9.0-beta.6`) before a key is built (the design's W17), and the
//! admin-reserved names are refused on that validated form with the
//! reference's words (ts-stack `edf6e03` `WalletPermissionsManager.ts`
//! 1509-1511, 1589, 1806-1808, 5587-5624). A key text from the owner is
//! parsed, rebuilt through the same functions and refused naming the
//! canonical text when the two differ.

use std::collections::BTreeSet;

use bsv_sdk::wallet::validation::{validate_base64_string, validate_basket, validate_label};
use bsv_sdk::wallet::{validate_protocol_name, validate_public_key};
use serde_json::Value;

use super::rows::{Kind, Refused};

/// A label's protocol name prefix (`WalletPermissionsManager.ts:1817-1837`).
const LABEL: &str = "action label ";
/// The two linkage protocols' name prefixes the manager builds
/// (`WalletPermissionsManager.ts:5190-5221`).
const COUNTERPARTY_LINKAGE: &str = "counterparty key linkage revelation ";
const SPECIFIC_LINKAGE: &str = "specific key linkage revelation ";
/// The two certificate protocols' name prefixes the manager builds over a
/// certificate type (`WalletPermissionsManager.ts:5387-5420`): a base64
/// type holds what a protocol name may not, so the type is read by
/// `validate_base64_string`, not the name's rule.
const CERT_ACQUISITION: &str = "certificate acquisition ";
const CERT_RELINQUISHMENT: &str = "certificate relinquishment ";

/// Whether a built name's tail reads.
type Tail = fn(&str) -> bool;

/// The protocol names the manager builds whose tail a protocol name's
/// rule would refuse: each prefix and whether its tail reads.
const BUILT_NAMES: [(&str, Tail); 3] = [
    (SPECIFIC_LINKAGE, specific_tail),
    (CERT_ACQUISITION, type_tail),
    (CERT_RELINQUISHMENT, type_tail),
];

/// A specific linkage name's tail: the protocol's validated name, a space
/// and its key id or `all`. The name and the key id are joined by a space
/// and either may hold one: any split whose name is canonical reads.
fn specific_tail(rest: &str) -> bool {
    rest.match_indices(' ').any(|(at, _)| {
        let inner = &rest[..at];
        at + 1 < rest.len() && validate_protocol_name(inner).is_ok_and(|v| v == inner)
    })
}

/// A certificate protocol's tail: a type `validate_base64_string` returns
/// unchanged.
fn type_tail(rest: &str) -> bool {
    validate_base64_string(rest, "type", None, None).is_ok_and(|t| t == rest)
}

/// One power: a kind and its instance (section (1)'s key table).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Power {
    /// A protocol at security level 1 or 2 (a label rides it).
    Protocol {
        /// The call's `privileged` (every row the port writes is `false`).
        privileged: bool,
        /// 1 or 2; level 0 is never a row.
        level: u8,
        /// The protocol name, validated.
        name: String,
        /// `""` at level 1; `self`, `anyone` or a key's lowercase hex at 2.
        counterparty: String,
    },
    /// An output basket, validated.
    Basket(String),
    /// A certificate type from a verifier, with the fields revealed.
    Certificate {
        /// The call's `privileged`.
        privileged: bool,
        /// The type, base64, trimmed.
        cert_type: String,
        /// The verifier's public key, compressed lowercase hex.
        verifier: String,
        /// The fields, sorted by their bytes, duplicates dropped.
        fields: Vec<String>,
    },
    /// `createAction`'s spend: one row per originator.
    Spending,
}

/// The reference's admin-reserved name rule, one for the three kinds that
/// name: a protocol, a label or a basket starting `admin`
/// (`WalletPermissionsManager.ts` 1509-1511, 1589, 1806-1808).
fn admin_reserved(name: &str) -> bool {
    name.starts_with("admin")
}

/// A validator's refusal as a key's.
fn bad(kind: Kind, key: &str, why: impl std::fmt::Display) -> Refused {
    Refused::BadKey {
        kind,
        key: key.to_string(),
        why: why.to_string(),
    }
}

impl Power {
    /// A protocol power from a call's `protocolID` and counterparty: the
    /// name through `validate_protocol_name`, level 0 refused before any
    /// name rule (the reference's order, `WalletPermissionsManager.ts`
    /// 1503-1511: level 0 never asks), the counterparty cleared at level 1
    /// and `self` when unnamed at level 2.
    ///
    /// # Errors
    ///
    /// [`Refused::LevelZero`], a level above 2, a name or counterparty
    /// the validators refuse, an admin-reserved name.
    pub fn protocol(
        privileged: bool,
        level: u8,
        name: &str,
        counterparty: Option<&str>,
    ) -> Result<Self, Refused> {
        let name = validate_protocol_name(name).map_err(|e| bad(Kind::Protocol, name, e))?;
        if level == 0 {
            return Err(Refused::LevelZero);
        }
        if admin_reserved(&name) {
            return Err(Refused::AdminOnly(format!(
                "Protocol \u{201c}{name}\u{201d} is admin-only."
            )));
        }
        Self::leveled(privileged, level, name, counterparty)
    }

    /// The protocol power with a validated name: the level's rule and the
    /// counterparty's form.
    fn leveled(
        privileged: bool,
        level: u8,
        name: String,
        counterparty: Option<&str>,
    ) -> Result<Self, Refused> {
        let counterparty = match (level, counterparty) {
            (0, _) => return Err(Refused::LevelZero),
            (1, _) => String::new(),
            (2, None | Some("self")) => "self".to_string(),
            (2, Some("anyone")) => "anyone".to_string(),
            (2, Some(hex)) => validate_public_key(hex, "counterparty")
                .map_err(|e| bad(Kind::Protocol, hex, e))?
                .to_hex(),
            (other, _) => {
                return Err(bad(
                    Kind::Protocol,
                    &other.to_string(),
                    "a security level is 0, 1 or 2",
                ))
            }
        };
        Ok(Self::Protocol {
            privileged,
            level,
            name,
            counterparty,
        })
    }

    /// A label's power: the protocol `[false,1,"action label <label>",""]`,
    /// the label through `validate_label`.
    ///
    /// # Errors
    ///
    /// A label the validator refuses, or an admin-reserved one (`admin`,
    /// `p ` first).
    pub fn label(label: &str) -> Result<Self, Refused> {
        let label = validate_label(label).map_err(|e| bad(Kind::Protocol, label, e))?;
        if admin_reserved(&label) || label.starts_with("p ") {
            return Err(Refused::AdminOnly(format!(
                "Label \u{201c}{label}\u{201d} is admin-only."
            )));
        }
        Ok(Self::Protocol {
            privileged: false,
            level: 1,
            name: format!("{LABEL}{label}"),
            counterparty: String::new(),
        })
    }

    /// The counterparty linkage power the manager builds for
    /// `revealCounterpartyKeyLinkage`: level 2 always, the verifier as its
    /// counterparty.
    ///
    /// # Errors
    ///
    /// A key the validator refuses.
    pub fn counterparty_linkage(counterparty: &str, verifier: &str) -> Result<Self, Refused> {
        let named = validate_public_key(counterparty, "counterparty")
            .map_err(|e| bad(Kind::Protocol, counterparty, e))?;
        let name = format!("{COUNTERPARTY_LINKAGE}{}", named.to_hex());
        Self::leveled(false, 2, name, Some(verifier))
    }

    /// The specific linkage power the manager builds for
    /// `revealSpecificKeyLinkage`: level 2 always, the call's protocol name
    /// validated and then its `keyID` at level 2 or `all`.
    ///
    /// # Errors
    ///
    /// A name or key the validators refuse.
    pub fn specific_linkage(
        protocol_name: &str,
        key_id: Option<&str>,
        verifier: &str,
    ) -> Result<Self, Refused> {
        let inner = validate_protocol_name(protocol_name)
            .map_err(|e| bad(Kind::Protocol, protocol_name, e))?;
        let name = format!("{SPECIFIC_LINKAGE}{inner} {}", key_id.unwrap_or("all"));
        Self::leveled(false, 2, name, Some(verifier))
    }

    /// The certificate protocol the manager builds for
    /// `acquireCertificate` (`certificate acquisition <type>`) or
    /// `relinquishCertificate` (`certificate relinquishment <type>`): level
    /// 1, the type through `validate_base64_string` (trimmed).
    ///
    /// # Errors
    ///
    /// A type the validator refuses.
    pub fn certificate_protocol(relinquish: bool, cert_type: &str) -> Result<Self, Refused> {
        let read = validate_base64_string(cert_type, "type", None, None)
            .map_err(|e| bad(Kind::Protocol, cert_type, e))?;
        let prefix = if relinquish {
            CERT_RELINQUISHMENT
        } else {
            CERT_ACQUISITION
        };
        Self::leveled(false, 1, format!("{prefix}{read}"), None)
    }

    /// A basket's power, the basket through `validate_basket`.
    ///
    /// # Errors
    ///
    /// A basket the validator refuses, or an admin-reserved one
    /// (`default`, or `admin` first).
    pub fn basket(basket: &str) -> Result<Self, Refused> {
        let basket = validate_basket(basket).map_err(|e| bad(Kind::Basket, basket, e))?;
        if basket == "default" || admin_reserved(&basket) {
            return Err(Refused::AdminOnly(format!(
                "Basket \u{201c}{basket}\u{201d} is admin-only."
            )));
        }
        Ok(Self::Basket(basket))
    }

    /// A certificate's power: the type through `validate_base64_string`,
    /// the verifier through `validate_public_key`, the fields sorted and
    /// deduplicated.
    ///
    /// # Errors
    ///
    /// A type or verifier the validators refuse.
    pub fn certificate<S: AsRef<str>>(
        privileged: bool,
        cert_type: &str,
        verifier: &str,
        fields: &[S],
    ) -> Result<Self, Refused> {
        let cert_type = validate_base64_string(cert_type, "type", None, None)
            .map_err(|e| bad(Kind::Certificate, cert_type, e))?;
        let verifier = validate_public_key(verifier, "verifier")
            .map_err(|e| bad(Kind::Certificate, verifier, e))?
            .to_hex();
        let fields: BTreeSet<String> = fields.iter().map(|f| f.as_ref().to_string()).collect();
        Ok(Self::Certificate {
            privileged,
            cert_type,
            verifier,
            fields: fields.into_iter().collect(),
        })
    }

    /// The power's kind.
    #[must_use]
    pub fn kind(&self) -> Kind {
        match self {
            Self::Protocol { .. } => Kind::Protocol,
            Self::Basket(_) => Kind::Basket,
            Self::Certificate { .. } => Kind::Certificate,
            Self::Spending => Kind::Spending,
        }
    }

    /// The key's canonical text.
    #[must_use]
    pub fn key(&self) -> String {
        let text = match self {
            Self::Protocol {
                privileged,
                level,
                name,
                counterparty,
            } => serde_json::to_string(&(privileged, level, name, counterparty)),
            Self::Basket(name) => serde_json::to_string(&(name,)),
            Self::Certificate {
                privileged,
                cert_type,
                verifier,
                fields,
            } => serde_json::to_string(&(privileged, cert_type, verifier, fields)),
            Self::Spending => Ok("[]".to_string()),
        };
        // A tuple of bools, numbers and strings always serializes.
        text.unwrap_or_default()
    }

    /// The power `key` names, read as the owner writes it and refused
    /// unless it is the canonical text of the power its names read to
    /// (section (4)).
    ///
    /// # Errors
    ///
    /// [`Refused::BadKey`] (not the kind's shape, or a name a validator
    /// refuses), [`Refused::NotCanonical`] naming the canonical text, an
    /// admin-reserved name, [`Refused::LevelZero`].
    pub fn parse(kind: Kind, key: &str) -> Result<Self, Refused> {
        let shape = |why: &str| bad(kind, key, why);
        let value: Value = serde_json::from_str(key).map_err(|_| shape(Self::shape(kind)))?;
        let items = value.as_array().ok_or_else(|| shape(Self::shape(kind)))?;
        let text = |v: &Value| v.as_str().map(str::to_string);
        let power = match (kind, items.as_slice()) {
            (Kind::Protocol, [privileged, level, name, counterparty]) => {
                let (Some(privileged), Some(level), Some(name), Some(counterparty)) = (
                    privileged.as_bool(),
                    level.as_u64().and_then(|l| u8::try_from(l).ok()),
                    text(name),
                    text(counterparty),
                ) else {
                    return Err(shape(Self::shape(kind)));
                };
                Self::protocol_named(privileged, level, &name, &counterparty)?
            }
            (Kind::Basket, [name]) => {
                Self::basket(&text(name).ok_or_else(|| shape(Self::shape(kind)))?)?
            }
            (Kind::Certificate, [privileged, cert_type, verifier, fields]) => {
                let fields: Option<Vec<String>> =
                    fields.as_array().and_then(|f| f.iter().map(text).collect());
                let (Some(privileged), Some(cert_type), Some(verifier), Some(fields)) = (
                    privileged.as_bool(),
                    text(cert_type),
                    text(verifier),
                    fields,
                ) else {
                    return Err(shape(Self::shape(kind)));
                };
                Self::certificate(privileged, &cert_type, &verifier, &fields)?
            }
            (Kind::Spending, []) => Self::Spending,
            _ => return Err(shape(Self::shape(kind))),
        };
        let canonical = power.key();
        if canonical != key {
            return Err(Refused::NotCanonical {
                kind,
                key: key.to_string(),
                canonical,
            });
        }
        Ok(power)
    }

    /// A protocol key's power by its name's form: a name the protocol
    /// validator returns unchanged is a protocol's (a label or a linkage
    /// name of that text is the same key); else a label's, by the label's
    /// rule; else a specific linkage name, whose key id may hold any text;
    /// else a protocol's, refused or rewritten by the validator.
    fn protocol_named(
        privileged: bool,
        level: u8,
        name: &str,
        counterparty: &str,
    ) -> Result<Self, Refused> {
        let named = (!counterparty.is_empty()).then_some(counterparty);
        if validate_protocol_name(name).is_ok_and(|v| v == name) {
            return Self::protocol(privileged, level, name, named);
        }
        if let Some(label) = name.strip_prefix(LABEL) {
            return Self::label(label);
        }
        for (prefix, reads) in BUILT_NAMES {
            if let Some(rest) = name.strip_prefix(prefix) {
                if !reads(rest) {
                    return Err(bad(
                        Kind::Protocol,
                        name,
                        "a name the manager builds is its prefix and its tail: a specific \
                         linkage's protocol name, a space and its key id or all; a certificate \
                         protocol's type as validate_base64_string returns it",
                    ));
                }
                return Self::leveled(privileged, level, name.to_string(), named);
            }
        }
        Self::protocol(privileged, level, name, named)
    }

    /// The kind's key shape, as a refusal names it.
    fn shape(kind: Kind) -> &'static str {
        match kind {
            Kind::Protocol => {
                "a protocol key is [privileged,level,\"name\",\"counterparty\"], level 1 or 2"
            }
            Kind::Basket => "a basket key is [\"name\"]",
            Kind::Certificate => {
                "a certificate key is [privileged,\"type\",\"verifier\",[\"field\",...]]"
            }
            Kind::Spending => "a spending key is []",
        }
    }

    /// The power the owner named for a direct grant: `key` read by
    /// [`Power::parse`]; a spending grant may leave it out.
    ///
    /// # Errors
    ///
    /// [`Refused::KeyRequired`] for another kind with no key, or
    /// [`Power::parse`]'s refusal.
    pub fn named(kind: Kind, key: Option<&str>) -> Result<Self, Refused> {
        match (kind, key) {
            (Kind::Spending, None) => Ok(Self::Spending),
            (kind, None) => Err(Refused::KeyRequired(kind)),
            (kind, Some(key)) => Self::parse(kind, key),
        }
    }

    /// Whether a row of this power admits `asked`: the same power, or for
    /// a certificate the same `privileged`, type and verifier with every
    /// field asked among the row's (the reference's subset rule,
    /// `WalletPermissionsManager.ts:3046-3051`).
    #[must_use]
    pub fn covers(&self, asked: &Self) -> bool {
        match (self, asked) {
            (
                Self::Certificate {
                    privileged,
                    cert_type,
                    verifier,
                    fields,
                },
                Self::Certificate {
                    privileged: p,
                    cert_type: t,
                    verifier: v,
                    fields: f,
                },
            ) => {
                privileged == p
                    && cert_type == t
                    && verifier == v
                    && f.iter().all(|x| fields.contains(x))
            }
            _ => self == asked,
        }
    }

    /// The prompt's words for this power at `usage` (section (3)'s table;
    /// a spending prompt is the pending row's, which holds the amount).
    #[must_use]
    pub fn asked(&self, usage: Option<&str>) -> String {
        let usage_words = usage.map_or(String::new(), |u| format!(" for {u}"));
        match self {
            Self::Protocol {
                name,
                level,
                counterparty,
                privileged,
            } => {
                let marked = if *privileged { " (privileged)" } else { "" };
                if let Some(label) = name.strip_prefix(LABEL) {
                    return match usage {
                        Some("apply") => format!("apply the action label {label:?}{marked}"),
                        Some("list") => format!("list actions by the label {label:?}{marked}"),
                        _ => format!("use the action label {label:?}{usage_words}{marked}"),
                    };
                }
                let with = if counterparty.is_empty() {
                    "any counterparty".to_string()
                } else {
                    format!("counterparty {counterparty}")
                };
                format!("use protocol {name:?} (level {level}) with {with}{usage_words}{marked}")
            }
            Self::Basket(name) => match usage {
                Some("insertion") => format!("insert into the basket {name:?}"),
                Some("listing") => format!("list the basket {name:?}"),
                Some("removal") => format!("remove from the basket {name:?}"),
                _ => format!("use the basket {name:?}{usage_words}"),
            },
            Self::Certificate {
                privileged,
                cert_type,
                verifier,
                fields,
            } => {
                let marked = if *privileged { " (privileged)" } else { "" };
                let what = if fields.is_empty() {
                    "reveal no fields".to_string()
                } else {
                    format!("reveal the fields {}", fields.join(", "))
                };
                format!("{what} of certificates of type {cert_type} to verifier {verifier}{marked}")
            }
            Self::Spending => format!("spend{usage_words}"),
        }
    }
}

#[cfg(test)]
mod tests;
