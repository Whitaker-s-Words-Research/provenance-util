//! Every text that `prov` shows to a person or an agent: output, warnings,
//! errors and the context that hooks give an agent.
//!
//! Reason: one place to review the wording. The help texts of the command
//! line are the exception: they are the doc comments in `prov/src/main.rs`,
//! and `prov/tests/snapshots/help.txt` shows them.
//!
//! Rules:
//! - One module for each area, named after the source module that uses it.
//!   Text that several areas share is in [`fs`] and [`run`].
//! - A fixed text is a `const`. A text with values is a function that
//!   returns a `String`. The compiler checks the values at each call.
//! - The code adds no words of its own. It adds only values, and the
//!   `prov: ` prefix of each message line on standard error.
//! - Not here: the JSON output (a format for programs), the names of git
//!   trailers, and the files that `prov init` writes.

pub mod fs;
pub mod run;

pub mod config;
pub mod pattern;
pub mod plan;
pub mod state;
pub mod trailer;

pub mod claude_code;
pub mod git;
pub mod gitai;

pub mod cli;

pub mod agent_hook;
pub mod approve;
pub mod archive;
pub mod check;
pub mod completions;
pub mod context;
pub mod copy_check;
pub mod cp;
pub mod doctor;
pub mod finish;
pub mod gate;
pub mod git_hook;
pub mod init;
pub mod procs;
pub mod revise;
pub mod schema;
pub mod session;
pub mod start;
pub mod trace;
