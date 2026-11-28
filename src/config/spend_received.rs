//! `SPEND_RECEIVED` (#659): the host's rule for a received payment that is
//! not mined, `taken` by default, `hold` the one other word.

use super::{ConfigError, Reader};
use crate::spend_gate::ReceivedSetting;

/// `SPEND_RECEIVED`, blank as unset: `taken` or `hold` (any case), any
/// other value refused by name; unset, or a refused value, is the default.
pub(super) fn read(raw: &Reader) -> ReceivedSetting {
    let setting = match raw.given("SPEND_RECEIVED") {
        None => Ok(None),
        Some(value) => match ReceivedSetting::of(&value.to_ascii_lowercase()) {
            Some(setting) => Ok(Some(setting)),
            None => Err(ConfigError {
                variable: "SPEND_RECEIVED",
                value: value.to_string(),
                expected: "a rule for received payments that are not mined: `taken` (the \
                           default: one a broadcaster took is offered) or `hold` (offered \
                           only under an action's allowPending: received)"
                    .into(),
            }),
        },
    };
    raw.kept(setting).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::super::tests::{config, refusal, row};
    use super::super::{Config, Source};

    /// `SPEND_RECEIVED` (#659): `taken`, the default, or `hold`, any case,
    /// blank as unset; any other value is refused at start by name, the
    /// configuration `config` prints holding the default and the refused row
    /// as set.
    #[test]
    fn the_received_rule_is_taken_or_hold_and_any_other_value_is_refused_by_name() {
        use crate::spend_gate::ReceivedSetting::{Hold, Taken};
        let received = |v| config(&[("SPEND_RECEIVED", v)]).host.spend_received;
        assert_eq!(Config::default().host.spend_received, Taken);
        for taken in ["taken", " Taken ", "TAKEN", "", "  "] {
            assert_eq!(received(taken), Taken, "{taken:?}");
        }
        for hold in ["hold", "HOLD", " Hold\t"] {
            assert_eq!(received(hold), Hold, "{hold:?}");
        }
        assert_eq!(
            row(&Config::default(), "SPEND_RECEIVED"),
            ("taken (default)".to_string(), Source::Default)
        );
        assert_eq!(
            row(&config(&[("SPEND_RECEIVED", "hold")]), "SPEND_RECEIVED"),
            ("hold".to_string(), Source::Env)
        );
        for refused in ["0", "off", "held", "received", "taken (default)"] {
            let e = refusal(&[("SPEND_RECEIVED", refused)]);
            assert_eq!(e.variable, "SPEND_RECEIVED");
            assert_eq!(
                e.to_string(),
                format!(
                    "SPEND_RECEIVED is {refused:?}, which is not a rule for received payments that \
                     are not mined: `taken` (the default: one a broadcaster took is offered) or \
                     `hold` (offered only under an action's allowPending: received): set it so, or \
                     unset it for the default"
                )
            );
        }
        let refused = Config::from_pairs(&[("SPEND_RECEIVED", "bogus")]).unwrap_err();
        assert_eq!(refused.config.host.spend_received, Taken);
        let k = refused
            .config
            .knobs()
            .into_iter()
            .find(|k| k.name == "SPEND_RECEIVED")
            .unwrap();
        assert_eq!(k.value.to_string(), "bogus");
        assert!(k
            .refused
            .unwrap()
            .starts_with("a rule for received payments"));
    }
}
