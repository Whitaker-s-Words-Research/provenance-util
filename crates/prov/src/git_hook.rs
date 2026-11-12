//! `prov hook git <name>`: the git hooks behind the shims.
//!
//! There is no `pre-merge-commit` hook: `MERGE_HEAD` does not exist when it
//! runs, and `--no-verify` skips it. `prepare-commit-msg` has `MERGE_HEAD`
//! and runs also with `--no-verify`.

use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use prov_core::trailer::{self, SessionRef, Trailer};
use prov_core::{Config, State};

use crate::context::{say, Ctx};
use crate::{gate, sweep};

/// Returns false to make the hook fail.
pub fn run(name: &str, args: &[String]) -> Result<bool> {
    let ctx = Ctx::here()?;
    match name {
        "pre-commit" => pre_commit(&ctx),
        "prepare-commit-msg" => prepare_commit_msg(&ctx, args),
        "post-commit" => post(&ctx, false),
        "post-merge" => post(&ctx, true),
        other => bail!("unknown git hook `{other}`"),
    }
}

/// The config of this checkout. During a merge, the file in the worktree can
/// be the agent's version; if it does not parse, the config of `HEAD` serves.
fn config_for_hook(ctx: &Ctx) -> Config {
    let config = match ctx.repo.config_here() {
        Ok(config) => config,
        Err(e) => {
            say(format!("warning: {e:#}. Using the config of HEAD."));
            ctx.repo.config_at("HEAD").ok().flatten().unwrap_or_default()
        }
    };
    // A mismatch stops only the merge gate and prov check: an agent branch
    // that started before a version change still has the old value.
    if let Err(e) = config.check_version(prov_core::VERSION) {
        say(format!("warning: {e}."));
    }
    config
}

fn pre_commit(ctx: &Ctx) -> Result<bool> {
    let config = config_for_hook(ctx);
    let repo = &ctx.repo;

    if !ctx.on_agent_branch() {
        // A commit by an agent: an agent environment variable, or the author
        // email of an agent.
        let marker = prov_adapters::agent_env_marker();
        let emails = prov_adapters::agent_emails(&config);
        let email = repo.author_email()?.map(|e| e.to_ascii_lowercase());
        let agent_email = email.as_ref().is_some_and(|e| emails.contains(e));
        if marker.is_some() || agent_email {
            let why = match marker {
                Some(var) => format!("{var} is set"),
                None => format!("the author email is {}", email.unwrap_or_default()),
            };
            say(format!(
                "refused: a commit by an agent ({why}) on the branch `{}`. An agent commits only on a branch \
                 that matches `agent/**`. Agent work reaches `{}` only through a merge of an agent branch.",
                repo.branch.as_deref().unwrap_or("detached HEAD"),
                config.human_branch,
            ));
            return Ok(false);
        }
        if !ctx.gitai.service_available(&repo.toplevel) {
            say("warning: the git-ai service is not available. This commit gets no checkpoints. Start it: git-ai bg start");
        }
        return Ok(true);
    }

    // An agent worktree.
    let open = ctx.store.open_in(&repo.toplevel)?;
    if open.len() > 1 {
        say(format!(
            "refused: this worktree is contended. {} sessions are open here: {}. End all but one, or run \
             `prov session close <id>`.",
            open.len(),
            open.iter().map(|s| s.id.as_str()).collect::<Vec<_>>().join(", "),
        ));
        return Ok(false);
    }
    if !ctx.gitai.service_available(&repo.toplevel) {
        // A checkpoint would exit 0 and record nothing.
        say("refused: the git-ai service is not available, so this commit would lose its attribution. \
             Start it with `git-ai bg start`, and commit where the service is reachable (not in a sandbox that blocks it).");
        return Ok(false);
    }
    // With no Open session there is no sweep: prov must not attribute a change
    // to a session that did not make it. Such lines become doubtful lines.
    if let [session] = open.as_slice() {
        sweep::sweep(ctx, session).context("the sweep failed")?;
    }
    Ok(true)
}

fn prepare_commit_msg(ctx: &Ctx, args: &[String]) -> Result<bool> {
    let message_file = PathBuf::from(args.first().context("prepare-commit-msg needs the message file")?);
    let repo = &ctx.repo;

    if !ctx.on_agent_branch() {
        if !squash_allowed(ctx)? {
            return Ok(false);
        }
        let heads = repo.merge_heads()?;
        if !heads.is_empty() {
            return gate::run(ctx, &message_file, &heads);
        }
        return Ok(true);
    }

    // A commit in an agent worktree with one Open session gets the session.
    if let [session] = ctx.store.open_in(&repo.toplevel)?.as_slice() {
        let this = SessionRef::new(&session.agent, &session.id);
        if let Some(adapter) = prov_adapters::by_agent_name(&session.agent) {
            // The record comes from the worktree. The agent's own variable is a cross-check.
            let env_id = adapter.session_id_env().and_then(|var| std::env::var(var).ok()).filter(|v| !v.is_empty());
            if env_id.as_ref().is_some_and(|id| *id != session.id) {
                say(format!(
                    "warning: the open session of this worktree is {}, but the environment names the session {}.",
                    session.id,
                    env_id.unwrap_or_default()
                ));
            }
        }
        let existing = trailer::sessions(&repo.message_trailers(&message_file)?);
        if !existing.contains(&this) {
            repo.add_trailers(&message_file, &[Trailer::new(trailer::PROV_SESSION, this.to_string())?])?;
        }
    }
    Ok(true)
}

/// After `git merge --squash`, the commit is refused if the squashed commits
/// are not all human commits. git lists them in `SQUASH_MSG`.
fn squash_allowed(ctx: &Ctx) -> Result<bool> {
    let repo = &ctx.repo;
    let text = match std::fs::read_to_string(repo.git_path("SQUASH_MSG")?) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(true),
        Err(e) => return Err(e).context("cannot read SQUASH_MSG"),
    };
    let shas: Vec<&str> = text
        .lines()
        .filter_map(|l| l.strip_prefix("commit "))
        .map(str::trim)
        .filter(|s| s.len() >= 40 && s.bytes().all(|b| b.is_ascii_hexdigit()))
        .collect();
    if shas.is_empty() {
        return Ok(true);
    }
    let emails = prov_adapters::agent_emails(&repo.config_at("HEAD").ok().flatten().unwrap_or_default());
    let mut args = vec!["--no-walk=unsorted"];
    args.extend(shas.iter().copied());
    let commits = repo.commits(args)?;
    let agent: Vec<_> = commits.iter().filter(|c| !gate::is_human_commit(c, &emails)).collect();
    if agent.is_empty() {
        return Ok(true);
    }
    say(format!(
        "refused: a squash of agent commits ({} of {} squashed commits, for example {} \"{}\"). \
         A squash is not permitted: integrate with `git merge <agent branch>`.\nTo stop the squash: git reset --hard",
        agent.len(),
        commits.len(),
        agent[0].short(),
        agent[0].subject,
    ));
    Ok(false)
}

/// `post-commit` and `post-merge`. git ignores the exit code of both.
fn post(ctx: &Ctx, post_merge: bool) -> Result<bool> {
    let repo = &ctx.repo;
    let head = repo.commit("HEAD")?;

    // After an integration: delete the live state of its sessions.
    if head.parents.len() == 2 && trailer::has(&head.trailers, trailer::PROV_VERSION) {
        let named = trailer::sessions(&head.trailers);
        let incoming = repo.git(["rev-list", &format!("{}..{}", head.parents[0], head.parents[1])])?;
        let incoming: Vec<&str> = incoming.lines().collect();
        let lock = ctx.store.lock()?;
        for r in lock.sessions()? {
            let is_named = named.iter().any(|s| s.id == r.id && s.agent == r.agent);
            // Also the other sessions on that branch that no trailer or note
            // names. Their transcript archives stay.
            let on_branch = r.state != State::Open
                && repo
                    .rev_parse(&format!("refs/heads/{}", r.branch))?
                    .is_some_and(|tip| incoming.contains(&tip.as_str()));
            if is_named || on_branch {
                lock.delete(&r.id)?;
            }
        }
    }

    if post_merge {
        warn_fast_forward(ctx)?;
    }
    Ok(true)
}

/// No hook can refuse a fast-forward. `post-merge` runs after one: if the
/// human branch moved onto commits that are not human commits, say how to undo it.
fn warn_fast_forward(ctx: &Ctx) -> Result<()> {
    let repo = &ctx.repo;
    if ctx.on_agent_branch() {
        return Ok(());
    }
    let (Some(orig), Some(head)) = (repo.rev_parse("ORIG_HEAD")?, repo.rev_parse("HEAD")?) else { return Ok(()) };
    if orig == head || !repo.is_ancestor(&orig, &head)? {
        return Ok(());
    }
    let emails = prov_adapters::agent_emails(&repo.config_at(&orig).ok().flatten().unwrap_or_default());
    let line = repo.commits(["--first-parent", &format!("{orig}..{head}")])?;
    let agent = line.iter().filter(|c| !gate::is_human_commit(c, &emails)).count();
    if agent > 0 {
        say(format!(
            "WARNING: this fast-forward put {agent} agent commit(s) on the branch `{}` with no merge commit, \
             so the merge gate did not run. A fast-forward of agent commits is not permitted.\n\
             Undo it: git reset --keep ORIG_HEAD\nThen integrate with: git merge --no-ff <agent branch>",
            repo.branch.as_deref().unwrap_or("HEAD"),
        ));
    }
    Ok(())
}
