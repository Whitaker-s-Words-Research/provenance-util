//! `prov completions`: the line that turns on tab completion in a shell's
//! startup file, and with `--install`, that line added to the file once.
//!
//! The startup file belongs to the user, not to a clone, so `prov init` does
//! not change it. It only says when the line is missing (owner decision,
//! 2026-11-12).

use std::fs;
use std::io::Write;
use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use serde::Serialize;

pub struct Args {
    pub shell: Option<String>,
    pub install: bool,
    pub dry_run: bool,
    pub json: bool,
}

#[derive(Debug, Serialize)]
struct Report {
    shell: String,
    file: PathBuf,
    line: String,
    /// True if the file already has the line.
    present: bool,
    /// True if this run added the line.
    added: bool,
    dry_run: bool,
}

/// A shell's startup file and the line for it.
struct Setup {
    shell: &'static str,
    file: PathBuf,
    line: &'static str,
}

impl Setup {
    /// For `bash`, `zsh` or `fish`, or a path to one of them, as in `$SHELL`.
    fn new(shell: &str) -> Result<Setup> {
        let name = shell.rsplit('/').next().unwrap_or(shell);
        let home = || std::env::var_os("HOME").map(PathBuf::from).context("HOME is not set");
        let env_dir = |var: &str| std::env::var_os(var).filter(|v| !v.is_empty()).map(PathBuf::from);
        Ok(match name {
            "bash" => Setup { shell: "bash", file: home()?.join(".bashrc"), line: "source <(COMPLETE=bash prov)" },
            "zsh" => Setup {
                shell: "zsh",
                file: env_dir("ZDOTDIR").map_or_else(home, Ok)?.join(".zshrc"),
                line: "source <(COMPLETE=zsh prov)",
            },
            "fish" => Setup {
                shell: "fish",
                file: env_dir("XDG_CONFIG_HOME").map_or_else(|| home().map(|h| h.join(".config")), Ok)?.join("fish/config.fish"),
                line: "COMPLETE=fish prov | source",
            },
            _ => bail!("no tab completion for the shell `{shell}`. Give --shell bash, zsh or fish."),
        })
    }

    /// From `$SHELL`.
    fn from_env() -> Result<Setup> {
        let shell = std::env::var("SHELL").ok().filter(|s| !s.is_empty()).context("SHELL is not set. Give --shell.")?;
        Setup::new(&shell)
    }

    /// True if the startup file turns on completion for `prov` already. Any
    /// line with `COMPLETE=<shell> prov` counts, also one that the user wrote.
    fn present(&self) -> bool {
        let needle = format!("COMPLETE={} prov", self.shell);
        fs::read_to_string(&self.file).is_ok_and(|text| text.contains(&needle))
    }
}

/// For `prov init`: the hint when the startup file of the user's shell lacks
/// the line. `None` if it has it, or if the shell is unknown.
pub fn missing_hint() -> Option<String> {
    let setup = Setup::from_env().ok()?;
    (!setup.present()).then(|| "tab completion is not set up. Run: prov completions --install".to_string())
}

pub fn run(args: &Args) -> Result<bool> {
    let setup = match &args.shell {
        Some(shell) => Setup::new(shell)?,
        None => Setup::from_env()?,
    };
    let present = setup.present();
    let mut report = Report {
        shell: setup.shell.to_string(),
        file: setup.file.clone(),
        line: setup.line.to_string(),
        present,
        added: false,
        dry_run: args.dry_run,
    };
    if args.install && !present && !args.dry_run {
        append(&setup)?;
        report.added = true;
    }

    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(true);
    }
    let file = setup.file.display();
    if !args.install {
        // The line alone on standard output, for a person who edits the file.
        println!("{}", setup.line);
        crate::context::say(if present {
            format!("{file} has it already.")
        } else {
            format!("add this line to {file}, or run: prov completions --install")
        });
    } else if present {
        println!("already in {file}");
    } else if args.dry_run {
        println!("Dry run: will add to {file}: {}", setup.line);
    } else {
        println!("added to {file}: {}", setup.line);
        println!("Open a new shell, or run: source {file}");
    }
    Ok(true)
}

/// Adds the line at the end of the file, on a line of its own.
fn append(setup: &Setup) -> Result<()> {
    if let Some(dir) = setup.file.parent() {
        fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    }
    let ends_open = fs::read(&setup.file).is_ok_and(|text| !text.is_empty() && !text.ends_with(b"\n"));
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&setup.file)
        .with_context(|| format!("cannot open {}", setup.file.display()))?;
    let text = format!("{}\n# Tab completion for prov\n{}\n", if ends_open { "\n" } else { "" }, setup.line);
    file.write_all(text.as_bytes()).with_context(|| format!("cannot write {}", setup.file.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shells() {
        assert_eq!(Setup::new("/usr/bin/bash").unwrap().line, "source <(COMPLETE=bash prov)");
        assert_eq!(Setup::new("fish").unwrap().shell, "fish");
        assert!(Setup::new("/bin/tcsh").is_err());
    }
}
