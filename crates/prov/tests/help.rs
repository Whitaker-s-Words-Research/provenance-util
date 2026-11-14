//! The help texts of every command, in one file for review:
//! `tests/snapshots/help.txt`.
//!
//! The help texts are the doc comments in `src/main.rs`. All other text that
//! prov shows is in the crate `prov-messages`. After a change to a help text,
//! write the file again and review its diff:
//!
//! ```sh
//! PROV_UPDATE_SNAPSHOTS=1 ci/dev.sh test -p prov --test help
//! ```
//!
//! In the container, the file is written in the container only. On the host:
//! `PROV_UPDATE_SNAPSHOTS=1 cargo test -p prov --test help`.

// A panic in a test is a failed test.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;
use std::process::Command;

fn help(args: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_prov"))
        .args(args)
        .arg("--help")
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .output()
        .unwrap();
    assert!(out.status.success(), "prov {args:?} --help: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout).unwrap()
}

/// The names in the `Commands:` section of a help text, without `help`.
fn subcommands(text: &str) -> Vec<String> {
    text.lines()
        .skip_while(|l| *l != "Commands:")
        .skip(1)
        .take_while(|l| l.starts_with("  "))
        .filter_map(|l| l.split_whitespace().next())
        .filter(|name| *name != "help")
        .map(str::to_string)
        .collect()
}

/// `prov --help`, then each command, depth first.
fn all(args: &mut Vec<String>, out: &mut String) {
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let text = help(&refs);
    out.push_str("$ prov ");
    for a in &refs {
        out.push_str(a);
        out.push(' ');
    }
    out.push_str("--help\n");
    out.push_str(&text);
    out.push('\n');
    for name in subcommands(&text) {
        args.push(name);
        all(args, out);
        args.pop();
    }
}

#[test]
fn help_texts_match_the_snapshot() {
    let mut text = String::new();
    all(&mut Vec::new(), &mut text);
    let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots/help.txt");
    if std::env::var_os("PROV_UPDATE_SNAPSHOTS").is_some() {
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, &text).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&file).unwrap_or_default();
    assert!(
        text == expected,
        "the help texts differ from {}. If the change is intended, write the file again: \
         PROV_UPDATE_SNAPSHOTS=1 cargo test -p prov --test help\n\n{text}",
        file.display()
    );
}
