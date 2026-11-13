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
use crate::{copy_check, gate, sweep};

/// Returns false to make the hook fail.
pub fn run(name: &str, args: &[String]) -> Result<bool> {
    let ctx = Ctx::here()?;
    let ok = match name {
        "pre-commit" => pre_commit(&ctx)?,
        "prepare-commit-msg" => prepare_commit_msg(&ctx, args)?,
        "post-commit" => post(&ctx, false)?,
        "post-merge" => post(&ctx, true)?,
        other => bail!("unknown git hook `{other}`"),
    };
    Ok(ok && run_local(&ctx, name, args)?)
}

/// A hook that existed before `prov init` stays as `<name>.local`. It runs
/// after the work of `prov`, with the same arguments. If it fails, the hook fails.
fn run_local(ctx: &Ctx, name: &str, args: &[String]) -> Result<bool> {
    let local = ctx.repo.hooks_dir()?.join(format!("{name}.local"));
    if !local.is_file() {
        return Ok(true);
    }
    let status = std::process::Command::new(&local)
        .args(args)
        .current_dir(&ctx.repo.toplevel)
        .status()
        .with_context(|| format!("cannot run {}", local.display()))?;
    if !status.success() {
        say(format!("the hook {} failed", local.display()));
    }
    Ok(status.success())
}

/// The approvals of `prov approve --human` and `--ai` become trailers of the
/// commit that holds the approved content. A change to the file since the
/// approval cancels it.
fn add_own_approvals(ctx: &Ctx, message_file: &std::path::Path) -> Result<()> {
    let repo = &ctx.repo;
    let pending = ctx.store.approvals(&repo.toplevel)?;
    if pending.is_empty() {
        return Ok(());
    }
    let staged = repo.git(["diff", "--cached", "--name-only", "--no-renames", "-z"])?;
    let staged: Vec<&str> = staged.split('\0').filter(|p| !p.is_empty()).collect();
    let existing = trailer::approvals(&repo.message_trailers(message_file)?);
    let mut add = Vec::new();
    for approval in pending {
        // Not in this commit: the approval waits for a later one.
        if !staged.contains(&approval.path.as_str()) {
            continue;
        }
        let blob = repo.git_opt(["rev-parse", "-q", "--verify", &format!(":{}", approval.path)])?;
        if blob.as_deref() != Some(approval.content_hash.as_str()) {
            say(format!(
                "warning: {} changed after `prov approve --{}`. The approval is cancelled; approve the file again.",
                approval.path, approval.choice
            ));
            ctx.store.lock()?.delete_approval(&approval)?;
            continue;
        }
        if !existing.iter().any(|a| a.path == approval.path) {
            add.push(Trailer::new(trailer::PROV_APPROVED, format!("{} {}", approval.path, approval.choice))?);
        }
    }
    repo.add_trailers(message_file, &add)
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
                 that matches `agent/**`. Agent work reaches this branch only through a merge of an agent branch.",
                repo.branch.as_deref().unwrap_or("detached HEAD"),
            ));
            return Ok(false);
        }
        if !ctx.gitai.service_available(&repo.toplevel) {
            // Without the service, the commit gets no note, and an amend
            // leaves the note on the old commit. Nothing reports it later.
            say("refused: the git-ai service is not available, so this commit would get no git-ai note \
                 (and an amend would leave the note on the old commit). Start it with `git-ai bg start`, \
                 check it with `git-ai bg status`, then commit again.");
            return Ok(false);
        }
        // The copy guard: a file of `prov cp` is committed as it was copied.
        return copy_check::guard(ctx);
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
    // Every hook looks at the worktree first: if the occupant is not the
    // session that prov saw here last, what is changed now is tainted. With
    // no occupant, that is every change: prov must not attribute a change to
    // a session that did not make it. Such lines become doubtful lines.
    {
        let lock = ctx.store.lock()?;
        let open = lock.open_in(&repo.toplevel)?;
        sweep::observe(ctx, &lock, &open).context("the taint checkpoint failed")?;
    }
    if let [session] = open.as_slice() {
        let session = refresh_model(ctx, session)?;
        sweep::sweep(ctx, &session).context("the sweep failed")?;
    }
    // `git-ai checkpoint` returns when the service has received the
    // checkpoint, not when it has applied it. A commit right after can be
    // recorded first, and its lines then go to a session that git-ai invents.
    // Seen in a container: the sweep's checkpoint was applied 50 ms after the
    // commit. Wait until the service has done its work.
    if let Err(e) = ctx.gitai.await_notes(&repo.toplevel) {
        say(format!("warning: {e:#}. The checkpoints of this commit may not be applied yet."));
    }
    Ok(true)
}

/// The latest model of the session, from its transcript, before the sweep.
///
/// The sweep checkpoint gives git-ai the model of the session, and git-ai
/// keeps one model per session. An agent can commit before its first Stop,
/// where `prov` otherwise reads the model; and with `claude -p` the record
/// had no model from `SessionStart`. The assistant message that runs
/// `git commit` is in the transcript before the command runs. git-ai takes
/// the model only from a checkpoint that records a change, so this helps
/// only when the sweep has changes.
fn refresh_model(ctx: &Ctx, session: &prov_core::SessionRecord) -> Result<prov_core::SessionRecord> {
    let latest = prov_adapters::by_agent_name(&session.agent)
        .zip(session.transcript_path.as_deref())
        .and_then(|(adapter, transcript)| adapter.latest_model(transcript));
    let Some(model) = latest.filter(|m| *m != session.model) else { return Ok(session.clone()) };
    let lock = ctx.store.lock()?;
    let Some(mut record) = lock.get(&session.id)? else { return Ok(session.clone()) };
    record.model = model;
    lock.put(&record)?;
    Ok(record)
}

fn prepare_commit_msg(ctx: &Ctx, args: &[String]) -> Result<bool> {
    let message_file = PathBuf::from(args.first().context("prepare-commit-msg needs the message file")?);
    let repo = &ctx.repo;

    if !ctx.on_agent_branch() {
        if !squash_allowed(ctx)? {
            return Ok(false);
        }
        let heads = repo.merge_heads()?;
        if !heads.is_empty() && !gate::run(ctx, &message_file, &heads)? {
            return Ok(false);
        }
        add_own_approvals(ctx, &message_file)?;
        copy_check::add_trailers(ctx, &message_file)?;
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
        // The trailers of the commit that was made: the human can change the
        // message after the gate.
        let groups = trailer::session_groups(&head.trailers);
        let lock = ctx.store.lock()?;
        for r in lock.sessions()? {
            let group = groups.iter().find(|(s, _)| s.id == r.id && s.agent == r.agent);
            // A session of the integration: only if it is Archived and the
            // commit records its transcript hash. It can be Open again by
            // now, or archived again with another transcript.
            let is_named = group.is_some_and(|(_, hash)| {
                r.state == State::Archived && hash.is_some() && *hash == r.archive_sha256
            });
            // No other record is deleted: `prov doctor` lists a record whose
            // branch is merged, and `prov session drop` removes it.
            if is_named {
                lock.delete(&r.id)?;
            }
        }
    }

    // An approval that became a trailer of this commit is done.
    let done = trailer::approvals(&head.trailers);
    if !done.is_empty() {
        let pending = ctx.store.approvals(&repo.toplevel)?;
        if !pending.is_empty() {
            let lock = ctx.store.lock()?;
            for approval in pending {
                if done.iter().any(|a| a.path == approval.path && a.choice.as_str() == approval.choice) {
                    lock.delete_approval(&approval)?;
                }
            }
        }
    }

    // A copy of `prov cp` in this commit: start the deferred check.
    if !post_merge {
        copy_check::committed(ctx, &head.sha)?;
    }

    if post_merge {
        warn_fast_forward(ctx)?;
    }
    Ok(true)
}

/// No hook can refuse a fast-forward. `post-merge` runs after one: if a
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
