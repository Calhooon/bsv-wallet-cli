//! A power's key (#414, the design's L5a exit test): its canonical form
//! per kind, the validators' readings (W17), the admin-reserved names,
//! level 0, a non-canonical key refused naming the canonical one, the
//! certificate's subset rule and the prompt's words.

use super::*;

/// The generator's public key, and twice it.
const G: &str = "0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798";
const G2: &str = "02c6047f9441ed7d6d3045406e95c07cd85c778e4b8cef3ca7abac09b95c709ee5";
/// A certificate type, 32 bytes in base64.
const TYPE: &str = "AGfk/WrT1eBDXpz3mcw386Zww2HmqcIn3uY6x4Af1eo=";

fn refused<T: std::fmt::Debug>(r: Result<T, Refused>) -> Refused {
    r.expect_err("refused")
}

#[test]
fn each_kind_writes_its_canonical_key_and_reads_it_back() {
    let protocol = Power::protocol(false, 2, " Todo List ", None).unwrap();
    let label = Power::label(" Invoices ").unwrap();
    let basket = Power::basket("Todo Items").unwrap();
    let certificate = Power::certificate(false, TYPE, G, &["name", "email"]).unwrap();
    for (power, kind, key) in [
        (
            &protocol,
            Kind::Protocol,
            r#"[false,2,"todo list","self"]"#.to_string(),
        ),
        (
            &label,
            Kind::Protocol,
            r#"[false,1,"action label invoices",""]"#.to_string(),
        ),
        (&basket, Kind::Basket, r#"["todo items"]"#.to_string()),
        (
            &certificate,
            Kind::Certificate,
            format!(r#"[false,"{TYPE}","{G}",["email","name"]]"#),
        ),
        (&Power::Spending, Kind::Spending, "[]".to_string()),
    ] {
        assert_eq!((power.kind(), power.key()), (kind, key.clone()));
        assert_eq!(&Power::parse(kind, &key).unwrap(), power, "{key}");
    }
}

#[test]
fn level_one_clears_the_counterparty_and_level_two_names_it() {
    let any = Power::protocol(false, 1, "todo list", Some(G)).unwrap();
    assert_eq!(any.key(), r#"[false,1,"todo list",""]"#);
    let unnamed = Power::protocol(false, 2, "todo list", None).unwrap();
    assert_eq!(
        unnamed,
        Power::protocol(false, 2, "todo list", Some("self")).unwrap()
    );
    let anyone = Power::protocol(false, 2, "todo list", Some("anyone")).unwrap();
    assert_eq!(anyone.key(), r#"[false,2,"todo list","anyone"]"#);
    // A key's hex in either case is printed compressed lowercase.
    let named = Power::protocol(false, 2, "todo list", Some(&G.to_uppercase())).unwrap();
    assert_eq!(named.key(), format!(r#"[false,2,"todo list","{G}"]"#));
    let bad = refused(Power::protocol(false, 2, "todo list", Some("02ab")));
    assert!(matches!(bad, Refused::BadKey { .. }), "{bad}");
    assert!(matches!(
        refused(Power::protocol(false, 3, "todo list", None)),
        Refused::BadKey { .. }
    ));
}

#[test]
fn level_zero_is_never_a_row() {
    assert_eq!(
        refused(Power::protocol(false, 0, "todo list", None)),
        Refused::LevelZero
    );
    let key = r#"[false,0,"todo list",""]"#;
    let e = refused(Power::parse(Kind::Protocol, key));
    assert_eq!(e, Refused::LevelZero);
    assert!(e
        .to_string()
        .contains("Level 0: No restrictions, open access"));
}

#[test]
fn a_non_canonical_key_is_refused_naming_the_canonical_one() {
    for (kind, key, canonical) in [
        (
            Kind::Protocol,
            r#"[false, 2, "todo list", "self"]"#.to_string(),
            r#"[false,2,"todo list","self"]"#.to_string(),
        ),
        (
            Kind::Protocol,
            r#"[false,2,"Todo List","self"]"#.to_string(),
            r#"[false,2,"todo list","self"]"#.to_string(),
        ),
        (
            Kind::Protocol,
            r#"[false,1,"todo list","self"]"#.to_string(),
            r#"[false,1,"todo list",""]"#.to_string(),
        ),
        (
            Kind::Protocol,
            r#"[false,1,"action label Invoices",""]"#.to_string(),
            r#"[false,1,"action label invoices",""]"#.to_string(),
        ),
        (
            Kind::Basket,
            r#"[" Todo Items "]"#.to_string(),
            r#"["todo items"]"#.to_string(),
        ),
        (
            Kind::Certificate,
            format!(
                r#"[false," {TYPE}","{}",["name","email","name"]]"#,
                G.to_uppercase()
            ),
            format!(r#"[false,"{TYPE}","{G}",["email","name"]]"#),
        ),
    ] {
        let e = refused(Power::parse(kind, &key));
        assert_eq!(
            e,
            Refused::NotCanonical {
                kind,
                key: key.clone(),
                canonical: canonical.clone()
            }
        );
        assert_eq!(
            e.to_string(),
            format!(
                "the {} key {key} is not in its canonical form; write it {canonical}",
                kind.as_str()
            )
        );
        // The canonical text reads.
        Power::parse(kind, &canonical).unwrap();
    }
}

#[test]
fn a_certificates_fields_are_sorted_and_deduplicated() {
    let power = Power::certificate(false, &format!(" {TYPE} "), G, &["phone", "email", "phone"]);
    let Power::Certificate {
        cert_type, fields, ..
    } = power.unwrap()
    else {
        panic!("not a certificate");
    };
    assert_eq!(cert_type, TYPE, "the type is trimmed");
    assert_eq!(fields, ["email", "phone"]);
    assert!(matches!(
        refused(Power::certificate(false, "not base64!", G, &["email"])),
        Refused::BadKey { .. }
    ));
}

#[test]
fn the_admin_reserved_names_are_refused_on_the_validated_form() {
    for (power, text) in [
        (
            Power::basket("Default"),
            "Basket \u{201c}default\u{201d} is admin-only.",
        ),
        (
            Power::basket(" admin x"),
            "Basket \u{201c}admin x\u{201d} is admin-only.",
        ),
        (
            Power::protocol(false, 1, " Admin X", None),
            "Protocol \u{201c}admin x\u{201d} is admin-only.",
        ),
        (
            Power::label("Admin notes"),
            "Label \u{201c}admin notes\u{201d} is admin-only.",
        ),
        (
            Power::label("p 1"),
            "Label \u{201c}p 1\u{201d} is admin-only.",
        ),
    ] {
        let e = refused(power);
        assert_eq!(e, Refused::AdminOnly(text.to_string()));
        assert_eq!(e.to_string(), text);
    }
    let e = refused(Power::parse(Kind::Basket, r#"["default"]"#));
    assert_eq!(
        e.to_string(),
        "Basket \u{201c}default\u{201d} is admin-only."
    );
    // A near name is not reserved.
    Power::basket("defaults").unwrap();
}

#[test]
fn a_key_off_its_kinds_shape_is_refused_by_name() {
    for (kind, key) in [
        (Kind::Basket, "todo"),
        (Kind::Basket, r#"["a","b"]"#),
        (Kind::Protocol, r#"[false,2,"todo list"]"#),
        (Kind::Protocol, r#"["false",2,"todo list","self"]"#),
        (Kind::Certificate, r#"[false,"x","y","email"]"#),
        (Kind::Spending, "[1]"),
        (Kind::Spending, "{}"),
    ] {
        let e = refused(Power::parse(kind, key));
        assert!(matches!(e, Refused::BadKey { .. }), "{kind:?} {key}: {e}");
        assert!(e.to_string().starts_with(&format!("{key} is not a")), "{e}");
    }
    // A name the validator refuses names the validator's words.
    let e = refused(Power::parse(Kind::Protocol, r#"[false,2,"ab","self"]"#));
    assert!(e.to_string().contains("at least 5 characters"), "{e}");
    assert_eq!(Power::parse(Kind::Spending, "[]").unwrap(), Power::Spending);
}

#[test]
fn a_direct_grant_names_its_key_but_for_spending() {
    assert_eq!(Power::named(Kind::Spending, None).unwrap(), Power::Spending);
    for kind in [Kind::Protocol, Kind::Basket, Kind::Certificate] {
        assert_eq!(
            refused(Power::named(kind, None)),
            Refused::KeyRequired(kind)
        );
    }
    let basket = Power::named(Kind::Basket, Some(r#"["todo items"]"#)).unwrap();
    assert_eq!(basket, Power::basket("todo items").unwrap());
}

#[test]
fn the_linkage_powers_keep_level_two_and_the_verifier() {
    let counterparty = Power::counterparty_linkage(G, G2).unwrap();
    let key = format!(r#"[false,2,"counterparty key linkage revelation {G}","{G2}"]"#);
    assert_eq!(counterparty.key(), key);
    assert_eq!(Power::parse(Kind::Protocol, &key).unwrap(), counterparty);

    // A key id the protocol validator would refuse still reads back.
    let specific = Power::specific_linkage("Todo List", Some("Key 1"), G2).unwrap();
    let key = format!(r#"[false,2,"specific key linkage revelation todo list Key 1","{G2}"]"#);
    assert_eq!(specific.key(), key);
    assert_eq!(Power::parse(Kind::Protocol, &key).unwrap(), specific);
    let all = Power::specific_linkage("todo list", None, G2).unwrap();
    assert!(
        all.key().contains("revelation todo list all"),
        "{}",
        all.key()
    );
}

#[test]
fn a_certificate_row_covers_a_subset_of_its_fields() {
    let row = Power::certificate(false, TYPE, G, &["email", "name"]).unwrap();
    let asked = |fields: &[&str]| Power::certificate(false, TYPE, G, fields).unwrap();
    assert!(row.covers(&asked(&["email"])));
    assert!(row.covers(&asked(&["name", "email"])));
    assert!(row.covers(&asked(&[])));
    assert!(!row.covers(&asked(&["phone"])));
    assert!(!row.covers(&asked(&["email", "phone"])));
    let other = Power::certificate(false, TYPE, G2, &["email"]).unwrap();
    assert!(!row.covers(&other), "another verifier");
    let privileged = Power::certificate(true, TYPE, G, &["email"]).unwrap();
    assert!(!row.covers(&privileged));
    // Every other kind covers its own power alone.
    let basket = Power::basket("todo items").unwrap();
    assert!(basket.covers(&basket.clone()));
    assert!(!basket.covers(&Power::basket("other").unwrap()));
}

#[test]
fn the_prompt_names_the_power() {
    let protocol = Power::protocol(false, 2, "todo list", None).unwrap();
    assert_eq!(
        protocol.asked(Some("signing")),
        r#"use protocol "todo list" (level 2) with counterparty self for signing"#
    );
    let any = Power::protocol(false, 1, "todo list", None).unwrap();
    assert_eq!(
        any.asked(Some("encrypting")),
        r#"use protocol "todo list" (level 1) with any counterparty for encrypting"#
    );
    let label = Power::label("invoices").unwrap();
    assert_eq!(
        label.asked(Some("apply")),
        r#"apply the action label "invoices""#
    );
    assert_eq!(
        label.asked(Some("list")),
        r#"list actions by the label "invoices""#
    );
    let basket = Power::basket("todo items").unwrap();
    for (usage, text) in [
        ("insertion", r#"insert into the basket "todo items""#),
        ("listing", r#"list the basket "todo items""#),
        ("removal", r#"remove from the basket "todo items""#),
    ] {
        assert_eq!(basket.asked(Some(usage)), text);
    }
    let certificate = Power::certificate(false, TYPE, G, &["name", "email"]).unwrap();
    assert_eq!(
        certificate.asked(Some("disclosure")),
        format!("reveal the fields email, name of certificates of type {TYPE} to verifier {G}")
    );
}
