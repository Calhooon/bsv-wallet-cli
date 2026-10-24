//! `bsv-wallet config`: every setting with its effective value and its
//! source (env, flag, default), as a table or, with `--json`, as an array
//! of `{name, value, source}`. Keys and tokens print as `<set>`
//! (`crate::config::Secret`).

use anyhow::Result;

use crate::config::{Config, Knob};

pub fn run(config: &Config, json: bool) -> Result<()> {
    let knobs = config.knobs();
    if json {
        println!("{}", serde_json::to_string(&knobs)?);
    } else {
        print!("{}", table(&knobs));
    }
    Ok(())
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
        out.push_str(&format!(
            "{:<name_width$}  {:<value_width$}  {}\n",
            k.name, k.value, k.source
        ));
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
}
