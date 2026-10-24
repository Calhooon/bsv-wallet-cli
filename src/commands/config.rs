//! `bsv-wallet config`: every setting with its effective value and its
//! source (env, flag, default), as a table or, with `--json`, as an array
//! of `{name, value, source}`. Keys and tokens print as `<set>`
//! (`crate::config::Secret`). A setting refused at start is printed as set
//! and marked (`refused` in the JSON), and the command then exits 1 naming
//! every refused variable.

use anyhow::Result;

use crate::config::{Config, ConfigErrors, Knob};

pub fn run(config: &Config, json: bool) -> Result<()> {
    let knobs = config.knobs();
    if json {
        println!("{}", serde_json::to_string(&knobs)?);
    } else {
        print!("{}", table(&knobs));
    }
    if config.refused().is_empty() {
        Ok(())
    } else {
        Err(ConfigErrors(config.refused().to_vec()).into())
    }
}

/// The knobs as three aligned columns under a header.
fn table(knobs: &[Knob]) -> String {
    let name_width = knobs.iter().map(|k| k.name.len()).max().unwrap_or(0).max(4);
    let value_width = knobs
        .iter()
        .map(|k| k.value.len())
        .max()
        .unwrap_or(0)
        .max(5);
    let mut out = format!(
        "{:<name_width$}  {:<value_width$}  SOURCE\n",
        "NAME", "VALUE"
    );
    for k in knobs {
        let line = format!(
            "{:<name_width$}  {:<value_width$}  {}",
            k.name, k.value, k.source
        );
        match k.refused {
            Some(expected) => out.push_str(&format!("{line}  REFUSED: not {expected}\n")),
            None => out.push_str(&format!("{line}\n")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_names_every_knob_with_its_source_and_hides_secrets() {
        let config = Config::from_pairs(&[
            ("AUTH_TOKEN", "a-token-never-printed"),
            ("SPEND_DEPTH", "3"),
        ])
        .unwrap();
        let out = table(&config.knobs());
        assert!(!out.contains("a-token-never-printed"), "{out}");
        let line = |name: &str| {
            out.lines()
                .find(|l| l.split_whitespace().next() == Some(name))
                .unwrap_or_else(|| panic!("no {name} line in {out}"))
                .split_whitespace()
                .collect::<Vec<_>>()
        };
        assert_eq!(line("AUTH_TOKEN"), ["AUTH_TOKEN", "<set>", "env"]);
        assert_eq!(line("SPEND_DEPTH"), ["SPEND_DEPTH", "3", "env"]);
        assert_eq!(line("MIN_UTXOS"), ["MIN_UTXOS", "3", "default"]);
        assert_eq!(line("--db"), ["--db", "wallet.db", "default"]);
        for knob in config.knobs() {
            line(knob.name);
        }
    }

    #[test]
    fn a_refused_setting_is_marked_and_the_command_exits_non_zero() {
        let refused =
            Config::from_pairs(&[("MIN_UTXOS", "x"), ("RELAY_POLL_SECS", "0")]).unwrap_err();
        let out = table(&refused.config.knobs());
        let line = |name: &str| {
            out.lines()
                .find(|l| l.split_whitespace().next() == Some(name))
                .unwrap()
                .split_whitespace()
                .collect::<Vec<_>>()
        };
        assert_eq!(
            line("MIN_UTXOS"),
            [
                "MIN_UTXOS",
                "x",
                "env",
                "REFUSED:",
                "not",
                "a",
                "whole",
                "number"
            ]
        );
        assert_eq!(line("SPEND_DEPTH"), ["SPEND_DEPTH", "1", "default"]);
        assert_eq!(
            line("RELAY_POLL_SECS"),
            [
                "RELAY_POLL_SECS",
                "0",
                "env",
                "REFUSED:",
                "not",
                "a",
                "number",
                "of",
                "seconds,",
                "at",
                "least",
                "1"
            ]
        );
        let e = run(&refused.config, true).unwrap_err().to_string();
        assert!(
            e.contains("MIN_UTXOS") && e.contains("RELAY_POLL_SECS"),
            "{e}"
        );
        assert!(run(&Config::default(), true).is_ok());
    }

    #[test]
    fn a_urls_credentials_are_never_printed() {
        let planted = [
            (
                "CHAINTRACKS_URL",
                "https://u1:secretpw1@headers.example/v1?key=secretq1",
            ),
            (
                "ARC_URL",
                "https://u2:secretpw2@arc.example:8443/arc?token=secretq2",
            ),
            (
                "PUBLIC_CALLBACK_URL",
                "https://cb.example/hook?secret=secretq3",
            ),
            ("RELAY_URL", "http://u4:secretpw4@relay.example"),
            (
                "BANANABLOCKS_BASE",
                "https://u5:secretpw5@bb.example/api?k=secretq5",
            ),
            ("WOC_BASE", "https://woc.example/v1?apikey=secretq6"),
            ("BITAILS_BASE", "https://u7:secretpw7@bitails.example/"),
            (
                "ARC_GORILLAPOOL_URL",
                "https://u8:secretpw8@gp.example/arc#secretf8",
            ),
        ];
        let config = Config::from_pairs(&planted).unwrap();
        let shown = format!("{}\n{}", table(&config.knobs()), config.log_line());
        let mut errors = String::new();
        for (name, value) in planted {
            let refused = value
                .replacen("https://", "ftp://", 1)
                .replacen("http://", "ftp://", 1);
            let e = Config::from_pairs(&[(name, &refused)]).unwrap_err();
            errors.push_str(&format!("{e}\n"));
        }
        for text in [&shown, &errors] {
            for secret in ["secretpw", "secretq", "secretf", "u1:", "u4:"] {
                assert!(!text.contains(secret), "{secret} printed:\n{text}");
            }
        }
        assert!(
            shown.contains("ARC_URL=https://<set>@arc.example:8443/arc?<set>"),
            "{shown}"
        );
        assert!(
            shown.contains("RELAY_URL=http://<set>@relay.example"),
            "{shown}"
        );
        assert!(
            errors.contains("RELAY_URL is \"ftp://<set>@relay.example\""),
            "{errors}"
        );
    }
}
