//! `prov approve <path>...`: a human decision for each path.
//!
//! - `--merge`: while a merge is in progress, after the gate refused. The
//!   approval exists only in git's merge message.
//! - `--human`, `--ai`: before the human's own commit, for lines whose author
//!   git-ai cannot know. The approval stays in live state until the commit.

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use prov_messages as msg;
use prov_core::trailer::{self, Trailer};
use prov_core::{now, ApprovalRecord};
use serde::Serialize;

use crate::context::{full_version, Ctx};
use crate::gate::{self, Reason};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Merge,
    Human,
    Ai,
}

pub struct Args {
    pub paths: Vec<PathBuf>,
    pub mode: Mode,
    pub agent: Option<String>,
    pub model: Option<String>,
    pub dry_run: bool,
    pub json: bool,
}

#[derive(Debug, Serialize)]
struct Report {
    dry_run: bool,
    /// `ack`, `human` or `ai`.
    choice: &'static str,
    paths: Vec<String>,
    approved: bool,
}

/// Asks for a typed `yes` at the terminal. With no terminal, nobody can approve.
fn confirm(question: &str) -> Result<bool> {
    let tty = std::fs::OpenOptions::new().read(true).write(true).open("/dev/tty").context(msg::approve::NEEDS_TERMINAL)?;
    let mut out = tty.try_clone()?;
    write!(out, "{question}\n{}", msg::approve::TYPE_YES)?;
    out.flush()?;
    let mut line = String::new();
    BufReader::new(tty).read_line(&mut line)?;
    Ok(line.trim() == "yes")
}

pub fn run(args: &Args) -> Result<bool> {
    let ctx = Ctx::here()?;
    let repo = &ctx.repo;
    if let Some(var) = prov_adapters::agent_env_marker() {
        bail!(msg::approve::agent_env(var));
    }
    if ctx.on_agent_branch() {
        bail!(msg::approve::AGENT_WORKTREE);
    }
    if args.paths.is_empty() {
        bail!(msg::approve::NO_PATHS);
    }
    if args.mode != Mode::Ai && (args.agent.is_some() || args.model.is_some()) {
        bail!(msg::approve::AGENT_MODEL_WITHOUT_AI);
    }
    let mut paths = Vec::new();
    for path in &args.paths {
        let rel = repo.relative_path(path)?;
        if !paths.contains(&rel) {
            paths.push(rel);
        }
    }

    let mut report = Report {
        dry_run: args.dry_run,
        choice: match args.mode {
            Mode::Merge => "ack",
            Mode::Human => "human",
            Mode::Ai => "ai",
        },
        paths: paths.clone(),
        approved: false,
    };
    let approved = match args.mode {
        Mode::Merge => approve_merge(&ctx, &paths, args)?,
        Mode::Human | Mode::Ai => approve_own(&ctx, &paths, args)?,
    };
    report.approved = approved;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else if args.dry_run {
        println!("{}", msg::approve::DRY_RUN);
    } else if !approved {
        println!("{}", msg::approve::NOT_APPROVED);
    }
    Ok(approved || args.dry_run)
}

fn approve_merge(ctx: &Ctx, paths: &[String], args: &Args) -> Result<bool> {
    let repo = &ctx.repo;
    let heads = repo.merge_heads()?;
    let [tip] = heads.as_slice() else {
        if heads.is_empty() {
            bail!(msg::approve::NO_MERGE);
        }
        bail!(msg::approve::OCTOPUS);
    };
    let message_file = repo.git_path("MERGE_MSG")?;
    if !message_file.is_file() {
        bail!(msg::approve::no_message_file(message_file.display()));
    }
    let config = repo.config_at("HEAD")?.context(msg::approve::HEAD_NO_CONFIG)?;
    let mut analysis = gate::analyze(repo, &ctx.gitai, &config, "HEAD", tip)?;
    analysis.drop_unknown_sessions(|s| gate::has_record(ctx, s));

    let message = repo.message_trailers(&message_file)?;
    let done = trailer::approvals(&message);
    let mut new = Vec::new();
    for path in paths {
        let items: Vec<&gate::Item> = analysis.items.iter().filter(|i| i.path == *path).collect();
        if items.is_empty() {
            let open: Vec<&str> = gate::unapproved(&analysis.items, &done).iter().map(|i| i.path.as_str()).collect();
            let mut text = msg::approve::nothing_to_approve(path);
            if !open.is_empty() {
                text.push_str(&msg::approve::open_items(open.join(", ")));
            }
            bail!(text);
        }
        println!("{}", msg::approve::path_header(path));
        for item in &items {
            println!("{}", msg::approve::item(item.describe()));
        }
        if items.iter().any(|i| i.reason == Reason::Protected) {
            // HEAD is the human branch before the integration.
            let diff = repo.git(["diff", &format!("HEAD...{tip}"), "--", path])?;
            println!("{diff}\n");
        }
        if !done.iter().any(|a| a.path == *path) {
            new.push(Trailer::new(trailer::PROV_APPROVED, format!("{path} ack"))?);
        }
    }
    if args.dry_run {
        return Ok(false);
    }
    if new.is_empty() {
        println!("{}", msg::approve::ALREADY_APPROVED);
        return Ok(true);
    }
    if !confirm(&msg::approve::confirm_merge(new.len()))? {
        return Ok(false);
    }
    repo.add_trailers(&message_file, &new)?;
    println!("{}", msg::approve::APPROVED_MERGE);
    Ok(true)
}

fn approve_own(ctx: &Ctx, paths: &[String], args: &Args) -> Result<bool> {
    let repo = &ctx.repo;
    let mut files = Vec::new();
    for path in paths {
        let file = repo.toplevel.join(path);
        if !file.is_file() {
            bail!(msg::approve::not_a_file(path));
        }
        files.push(file);
    }
    if !ctx.gitai.service_available(&repo.toplevel) {
        bail!(msg::approve::GITAI_UNAVAILABLE);
    }
    let (choice, who) = match args.mode {
        Mode::Ai => (
            "ai",
            msg::approve::who_ai(args.agent.as_deref().unwrap_or("unknown"), args.model.as_deref().unwrap_or("unknown")),
        ),
        _ => ("human", msg::approve::WHO_HUMAN.to_string()),
    };
    println!("{}", msg::approve::author(who));
    for path in paths {
        println!("{}", msg::approve::path(path));
    }
    if args.dry_run {
        return Ok(false);
    }
    if !confirm(msg::approve::CONFIRM_OWN)? {
        return Ok(false);
    }
    let at = now();
    match args.mode {
        Mode::Ai => ctx.gitai.checkpoint_ai(
            &repo.toplevel,
            &files,
            args.agent.as_deref().unwrap_or("unknown"),
            args.model.as_deref().unwrap_or("unknown"),
            &format!("prov-approve-{at}"),
        )?,
        _ => ctx.gitai.checkpoint_known_human(&repo.toplevel, &files, &full_version())?,
    }
    // The checkpoint is applied after it is received; the commit can follow at once.
    if let Err(e) = ctx.gitai.await_notes(&repo.toplevel) {
        crate::context::say(msg::context::warning(format!("{e:#}")));
    }
    let lock = ctx.store.lock()?;
    for (path, file) in paths.iter().zip(&files) {
        // The blob ID is the content hash: a change to the file cancels the approval.
        let content_hash = repo.git(["hash-object".as_ref(), "--".as_ref(), file.as_os_str()])?;
        lock.put_approval(&ApprovalRecord {
            worktree: repo.toplevel.clone(),
            path: path.clone(),
            choice: choice.to_string(),
            content_hash,
            approved_at: at,
        })?;
    }
    println!("{}", msg::approve::recorded(choice));
    Ok(true)
}
