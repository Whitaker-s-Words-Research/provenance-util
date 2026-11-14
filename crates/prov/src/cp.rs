//! `prov cp`: copies files, as they are at a commit, into this repository,
//! with an author for every line.
//!
//! The command works in two steps. It writes a plan: each file, and for each
//! file the line ranges with their authors. Then it replays each file in
//! stages, with one git-ai checkpoint after each stage, so that git-ai
//! records the authors of the plan.

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{bail, Context, Result};
use prov_messages as msg;
use prov_core::plan::{ranges_of, Plan, PlanFile, Range, Source, Who, UNKNOWN};
use prov_core::{now, CopyFile, CopyRecord};
use prov_git::Repo;
use serde::Serialize;

use crate::context::{full_version, say, Ctx};

pub struct Args {
    pub from: Option<PathBuf>,
    pub rev: Option<String>,
    /// The source paths, then the destination.
    pub paths: Vec<String>,
    pub all: Option<String>,
    pub missing: Option<String>,
    pub agent: Option<String>,
    pub model: Option<String>,
    pub session: Option<String>,
    pub plan: Option<PathBuf>,
    pub apply: Option<PathBuf>,
    pub dry_run: bool,
    pub json: bool,
}

const MODE_LINK: &str = "120000";
const MODE_EXEC: &str = "100755";

/// One file of the plan with its content at the source commit.
struct Item {
    file: PlanFile,
    mode: String,
    blob: String,
    bytes: Vec<u8>,
}

impl Item {
    fn lines(&self) -> Vec<&[u8]> {
        if self.file.is_text() {
            self.bytes.split_inclusive(|b| *b == b'\n').collect()
        } else {
            Vec::new()
        }
    }
}

#[derive(Debug, Serialize)]
struct Report {
    id: String,
    dry_run: bool,
    /// True if the files are written and staged.
    applied: bool,
    source: ReportSource,
    files: Vec<ReportFile>,
    /// The ranges with no author. The plan is not applied while one exists.
    unresolved: Vec<String>,
    notes: Vec<String>,
    /// The plan file, if one was written.
    #[serde(skip_serializing_if = "Option::is_none")]
    plan_file: Option<PathBuf>,
}

#[derive(Debug, Serialize)]
struct ReportSource {
    repo: String,
    commit: String,
}

#[derive(Debug, Serialize)]
struct ReportFile {
    path: String,
    dest: String,
    /// `text`, `binary` or `symlink`.
    kind: &'static str,
    lines: usize,
    ranges: Vec<Range>,
}

/// The author that `--all` or `--missing` gives.
fn author_option(name: &str, value: &str, args: &Args, id: &str) -> Result<Who> {
    match value {
        "human" => Ok(Who::Human),
        "ai" => Ok(Who::Ai {
            agent: args.agent.clone().unwrap_or_else(|| UNKNOWN.into()),
            model: args.model.clone().unwrap_or_else(|| UNKNOWN.into()),
            session: args.session.clone().unwrap_or_else(|| id.to_string()),
        }),
        other => bail!(msg::cp::author_option(name, other)),
    }
}

/// A new plan ID. It is a file name, and the conversation ID of an `ai`
/// range that has no `--session`.
fn new_id() -> Result<String> {
    use std::io::Read;
    let mut bytes = [0u8; 3];
    fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut bytes)).context(msg::start::CANNOT_READ_URANDOM)?;
    Ok(format!("prov-cp-{}-{:02x}{:02x}{:02x}", now(), bytes[0], bytes[1], bytes[2]))
}

fn git_bytes(repo: &Repo, args: &[&str]) -> Result<Vec<u8>> {
    repo.git_bytes(args)
}

/// An entry of the tree of the source commit.
struct Entry {
    mode: String,
    kind: String,
    blob: String,
    path: String,
}

/// The files under `path` that the commit tracks. `path` can be a file.
fn tree_entries(src: &Repo, commit: &str, path: &str, recursive: bool) -> Result<Vec<Entry>> {
    let mut args = vec!["ls-tree", "-z"];
    if recursive {
        args.push("-r");
    }
    args.push(commit);
    if !path.is_empty() {
        args.extend(["--", path]);
    }
    let text = String::from_utf8(git_bytes(src, &args)?).context(msg::cp::SOURCE_PATH_NOT_UTF8)?;
    let mut out = Vec::new();
    for record in text.split('\0').filter(|r| !r.is_empty()) {
        let (meta, name) = record.split_once('\t').context(msg::cp::UNEXPECTED_LS_TREE)?;
        let mut parts = meta.split(' ');
        let (Some(mode), Some(kind), Some(blob)) = (parts.next(), parts.next(), parts.next()) else {
            bail!(msg::cp::UNEXPECTED_LS_TREE);
        };
        out.push(Entry { mode: mode.into(), kind: kind.into(), blob: blob.into(), path: name.into() });
    }
    Ok(out)
}

fn is_binary(bytes: &[u8]) -> bool {
    // The rule of git: a NUL byte in the first 8000 bytes.
    bytes.iter().take(8000).any(|b| *b == 0)
}

/// Reads one file from the source commit, never from a working tree.
fn load(src: &Repo, commit: &str, entry: &Entry) -> Result<Vec<u8>> {
    if entry.kind != "blob" {
        bail!(msg::cp::submodule(&entry.path));
    }
    git_bytes(src, &["cat-file", "blob", &entry.blob])
        .with_context(|| msg::cp::cannot_read_source(&entry.path, commit, src.toplevel.display()))
}

/// The path of `dest` relative to the root of the worktree. Empty for the root.
fn worktree_relative(repo: &Repo, dest: &str) -> Result<String> {
    let cwd = std::env::current_dir().context(msg::fs::NO_CURRENT_DIR)?;
    let mut abs = PathBuf::new();
    for part in cwd.join(dest).components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                abs.pop();
            }
            other => abs.push(other.as_os_str()),
        }
    }
    let top = fs::canonicalize(&repo.toplevel).unwrap_or_else(|_| repo.toplevel.clone());
    let rel = abs
        .strip_prefix(&top)
        .or_else(|_| abs.strip_prefix(&repo.toplevel))
        .with_context(|| msg::cp::not_in_worktree(dest, repo.toplevel.display()))?;
    Ok(rel.to_str().with_context(|| msg::cp::not_utf8(dest))?.to_string())
}

fn join(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_string()
    } else {
        format!("{dir}/{name}")
    }
}

fn base_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// The `<source>` of a `Copied-From` trailer: the remote URL of the source
/// repository, or its absolute path if it has no remote.
fn source_label(src: &Repo) -> Result<String> {
    let remotes = src.git(["remote"])?;
    let remote = remotes.lines().find(|r| *r == "origin").or_else(|| remotes.lines().next());
    if let Some(url) = remote.and_then(|r| src.git_opt(["remote", "get-url", r]).ok().flatten()) {
        // A URL can hold a password. It does not go into a commit message.
        if let Some((scheme, rest)) = url.split_once("://") {
            let host_start = rest.split('/').next().and_then(|authority| authority.rfind('@')).map_or(0, |at| at + 1);
            return Ok(format!("{scheme}://{}", &rest[host_start..]));
        }
        return Ok(url);
    }
    let top = fs::canonicalize(&src.toplevel).unwrap_or_else(|_| src.toplevel.clone());
    Ok(top.to_str().context(msg::cp::SOURCE_NOT_UTF8)?.to_string())
}

/// A checkout of the source commit in a temporary directory.
///
/// `git-ai blame` reads a file of a working tree and has no option for a
/// commit. The checkout is a clone that shares the objects of the source, so
/// nothing in the source changes and the working tree of the source is never read.
struct Checkout {
    dir: PathBuf,
}

impl Checkout {
    fn new(src: &Repo, commit: &str, id: &str) -> Result<Checkout> {
        let dir = std::env::temp_dir().join(id);
        let checkout = Checkout { dir: dir.clone() };
        let run = |cwd: &Path, args: &[&OsStr]| -> Result<()> {
            let out = Command::new("git").current_dir(cwd).args(args).stdin(Stdio::null()).output().context(msg::run::CANNOT_RUN_GIT)?;
            if !out.status.success() {
                bail!(msg::cp::git_failed(args, String::from_utf8_lossy(&out.stderr).trim()));
            }
            Ok(())
        };
        let os = OsStr::new;
        run(&src.toplevel, &[os("clone"), os("-q"), os("--shared"), os("--no-checkout"), src.toplevel.as_os_str(), dir.as_os_str()])?;
        run(&dir, &[os("fetch"), os("-q"), os("origin"), os("refs/notes/ai:refs/notes/ai")])?;
        run(&dir, &[os("-c"), os("advice.detachedHead=false"), os("checkout"), os("-q"), os("--detach"), os(commit)])?;
        Ok(checkout)
    }
}

impl Drop for Checkout {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

/// Builds the plan from the source paths and the options.
fn build(ctx: &Ctx, src: &Repo, commit: &str, args: &Args, id: &str, notes: &mut Vec<String>) -> Result<Vec<Item>> {
    let (dest_arg, sources) = args.paths.split_last().context(msg::cp::NO_PATHS)?;
    if sources.is_empty() {
        bail!(msg::cp::NO_SOURCE_PATHS);
    }
    let dest = worktree_relative(&ctx.repo, dest_arg)?;
    let dest_is_dir = dest.is_empty() || dest_arg.ends_with('/') || ctx.repo.toplevel.join(&dest).is_dir();
    let many = sources.len() > 1;

    let mut found: Vec<(Entry, String)> = Vec::new();
    for source in sources {
        let path = source.trim_end_matches('/');
        let path = if path == "." { "" } else { path };
        if path.starts_with('/') || path.split('/').any(|c| c == "..") {
            bail!(msg::cp::absolute_source(source));
        }
        let entries = tree_entries(src, commit, path, true)?;
        if entries.is_empty() {
            bail!(msg::cp::no_source_file(commit, src.toplevel.display(), source));
        }
        let one_file = entries.len() == 1 && entries[0].path == path;
        if one_file {
            let to = if many || dest_is_dir { join(&dest, base_name(path)) } else { dest.clone() };
            found.extend(entries.into_iter().map(|e| (e, to.clone())));
        } else {
            // A directory. Alone, it becomes the destination; with other
            // sources, it goes into the destination under its name.
            let root = if many && !path.is_empty() { join(&dest, base_name(path)) } else { dest.clone() };
            for e in entries {
                let below = if path.is_empty() { e.path.as_str() } else { &e.path[path.len() + 1..] };
                let to = join(&root, below);
                found.push((e, to));
            }
        }
    }

    let all = args.all.as_deref().map(|v| author_option("--all", v, args, id)).transpose()?;
    let missing = args.missing.as_deref().map(|v| author_option("--missing", v, args, id)).transpose()?;

    // The AI sessions of the source, unless `--all` ignores them.
    let mut checkout = None;
    if all.is_none() {
        if src.git_ok(["rev-parse", "-q", "--verify", "refs/notes/ai"])? {
            checkout = Some(Checkout::new(src, commit, id)?);
        } else {
            notes.push(msg::cp::no_notes(src.toplevel.display()));
        }
    }

    let mut items = Vec::new();
    for (entry, to) in found {
        let bytes = load(src, commit, &entry)?;
        let symlink = entry.mode == MODE_LINK;
        let binary = !symlink && is_binary(&bytes);
        let mut file = PlanFile { path: entry.path.clone(), dest: to, binary, symlink, ranges: Vec::new() };
        if file.is_text() {
            let count = bytes.split_inclusive(|b| *b == b'\n').count();
            let mut authors = vec![all.clone().or_else(|| missing.clone()).unwrap_or(Who::Unresolved); count];
            // git-ai blame fails on a file with no lines.
            if let Some(checkout) = checkout.as_ref().filter(|_| count > 0) {
                for (start, end, session) in ctx.gitai.blame(&checkout.dir, &entry.path)? {
                    for line in start..=end {
                        if let Some(slot) = authors.get_mut(line as usize - 1) {
                            *slot = Who::Ai { agent: session.tool.clone(), model: session.model.clone(), session: session.id.clone() };
                        }
                    }
                }
            }
            file.ranges = ranges_of(&authors);
        }
        items.push(Item { file, mode: entry.mode, blob: entry.blob, bytes });
    }
    Ok(items)
}

/// Reads the content of each file of a plan from the source commit, and
/// checks the plan against it.
fn load_plan(src: &Repo, plan: &Plan) -> Result<Vec<Item>> {
    let mut items = Vec::new();
    for file in &plan.files {
        let entry = tree_entries(src, &plan.source.commit, &file.path, false)?
            .into_iter()
            .find(|e| e.path == file.path && e.kind != "tree")
            .with_context(|| msg::cp::plan_file_missing(&plan.source.commit, &file.path))?;
        let bytes = load(src, &plan.source.commit, &entry)?;
        let symlink = entry.mode == MODE_LINK;
        let binary = !symlink && is_binary(&bytes);
        if file.symlink != symlink || file.binary != binary {
            bail!(msg::cp::kind_mismatch(&file.path, kind(symlink, binary), kind(file.symlink, file.binary)));
        }
        items.push(Item { file: file.clone(), mode: entry.mode, blob: entry.blob, bytes });
    }
    Ok(items)
}

fn kind(symlink: bool, binary: bool) -> &'static str {
    match (symlink, binary) {
        (true, _) => msg::cp::KIND_SYMLINK,
        (false, true) => msg::cp::KIND_BINARY,
        (false, false) => msg::cp::KIND_TEXT,
    }
}

/// The rules for a plan, apart from unresolved ranges.
fn validate(items: &[Item]) -> Result<()> {
    let plan_files: Vec<PlanFile> = items.iter().map(|i| i.file.clone()).collect();
    let plan = Plan { source: Source { repo: String::new(), commit: String::new() }, files: plan_files };
    plan.check_dests()?;
    plan.check_models()?;
    for item in items {
        item.file.check_cover(u32::try_from(item.lines().len()).context(msg::cp::TOO_MANY_LINES)?)?;
        let ok = !item.file.dest.is_empty()
            && Path::new(&item.file.dest).components().all(|c| matches!(c, Component::Normal(_)))
            && !item.file.dest.split('/').any(|c| c.eq_ignore_ascii_case(".git"));
        if !ok {
            bail!(msg::cp::bad_dest(&item.file.dest));
        }
    }
    Ok(())
}

/// If identical lines of a file get different authors, git-ai's diff cannot
/// tell which copy each stage added: the authors can move between them.
fn identical_line_notes(items: &[Item], notes: &mut Vec<String>) {
    for item in items {
        let lines = item.lines();
        let authors = item.file.line_authors();
        let mut by_content: BTreeMap<&[u8], Vec<&Who>> = BTreeMap::new();
        for (line, who) in lines.iter().zip(&authors) {
            let seen = by_content.entry(line).or_default();
            if !seen.contains(who) {
                seen.push(who);
            }
        }
        let groups = by_content.values().filter(|a| a.len() > 1).count();
        if groups > 0 {
            notes.push(msg::cp::identical_lines(&item.file.dest, groups));
        }
    }
}

fn report_files(items: &[Item]) -> Vec<ReportFile> {
    items
        .iter()
        .map(|i| ReportFile {
            path: i.file.path.clone(),
            dest: i.file.dest.clone(),
            kind: if i.file.symlink {
                "symlink"
            } else if i.file.binary {
                "binary"
            } else {
                "text"
            },
            lines: i.lines().len(),
            ranges: i.file.ranges.clone(),
        })
        .collect()
}

/// One line for each file: path, line count and authors. The ranges only
/// for a file with more than one author.
fn print_summary(files: &[ReportFile]) {
    for f in files {
        if f.kind != "text" {
            println!("{}", if f.kind == "symlink" { msg::cp::summary_symlink(&f.dest) } else { msg::cp::summary_binary(&f.dest) });
            continue;
        }
        let mut counts: Vec<(&Who, u32)> = Vec::new();
        for r in &f.ranges {
            match counts.iter_mut().find(|(who, _)| *who == &r.who) {
                Some((_, n)) => *n += r.lines(),
                None => counts.push((&r.who, r.lines())),
            }
        }
        let authors: Vec<String> = counts.iter().map(|(who, n)| msg::cp::author_count(who, n)).collect();
        println!("{}", msg::cp::summary_text(&f.dest, f.lines, if authors.is_empty() { msg::cp::EMPTY.into() } else { authors.join(", ") }));
        if counts.len() > 1 {
            for r in &f.ranges {
                let lines = if r.start == r.end { r.start.to_string() } else { format!("{}-{}", r.start, r.end) };
                println!("{}", msg::cp::summary_range(lines, &r.who));
            }
        }
    }
}

/// Opens the plan in the editor that `git commit` uses, at the terminal.
/// Returns false if there is no terminal or no editor.
fn edit(repo: &Repo, file: &Path) -> Result<bool> {
    let Ok(tty) = fs::OpenOptions::new().read(true).write(true).open("/dev/tty") else { return Ok(false) };
    // GIT_EDITOR, core.editor, VISUAL, EDITOR: the order of git.
    let Some(editor) = repo.git_opt(["var", "GIT_EDITOR"])?.filter(|e| !e.is_empty()) else { return Ok(false) };
    let status = Command::new("sh")
        .arg("-c")
        .arg(format!("{editor} \"$@\""))
        .arg(&editor)
        .arg(file)
        .current_dir(&repo.toplevel)
        .stdin(tty.try_clone()?)
        .stdout(tty.try_clone()?)
        .stderr(tty)
        .status()
        .with_context(|| msg::cp::cannot_run_editor(&editor))?;
    if !status.success() {
        bail!(msg::cp::editor_failed(&editor, file.display()));
    }
    Ok(true)
}

fn plan_of(source: &Source, items: &[Item]) -> Plan {
    Plan { source: source.clone(), files: items.iter().map(|i| i.file.clone()).collect() }
}

pub fn run(args: &Args) -> Result<bool> {
    let ctx = Ctx::here()?;
    let repo = &ctx.repo;
    if let Some(var) = prov_adapters::agent_env_marker() {
        bail!(msg::cp::agent_env(var));
    }
    if ctx.on_agent_branch() {
        bail!(msg::cp::AGENT_WORKTREE);
    }
    if args.all.is_some() && args.missing.is_some() {
        bail!(msg::cp::ALL_AND_MISSING);
    }
    let ai_option = args.all.as_deref() == Some("ai") || args.missing.as_deref() == Some("ai");
    if (args.agent.is_some() || args.model.is_some()) && !ai_option {
        bail!(msg::cp::AGENT_MODEL_WITHOUT_AI);
    }
    if args.session.is_some() && args.all.as_deref() != Some("ai") {
        bail!(msg::cp::SESSION_WITHOUT_ALL_AI);
    }
    if args.plan.is_some() && args.apply.is_some() {
        bail!(msg::cp::PLAN_AND_APPLY);
    }

    let id = new_id()?;
    let mut notes = Vec::new();

    // The plan: from a file, or from the source and the options.
    let (source, src, mut items) = if let Some(file) = &args.apply {
        if args.from.is_some() || args.rev.is_some() || !args.paths.is_empty() || args.all.is_some() || args.missing.is_some() {
            bail!(msg::cp::APPLY_ONLY_FILE);
        }
        let text = fs::read_to_string(file).with_context(|| msg::fs::cannot_read(file.display()))?;
        let plan = Plan::parse(&text, &id)?;
        let src = Repo::discover(Path::new(&plan.source.repo))
            .ok()
            .flatten()
            .with_context(|| msg::cp::plan_source_not_repo(&plan.source.repo))?;
        let commit = src
            .rev_parse(&plan.source.commit)?
            .with_context(|| msg::cp::no_source_commit(&plan.source.commit))?;
        let source = Source { repo: plan.source.repo.clone(), commit };
        let items = load_plan(&src, &Plan { source: source.clone(), files: plan.files })?;
        (source, src, items)
    } else {
        let from = args.from.as_ref().context(msg::cp::NO_FROM)?;
        let src = Repo::discover(from)
            .ok()
            .flatten()
            .with_context(|| msg::cp::from_not_repo(from.display()))?;
        let rev = args.rev.as_deref().unwrap_or("HEAD");
        let commit = src.rev_parse(rev)?.with_context(|| msg::cp::no_source_commit(rev))?;
        let top = fs::canonicalize(&src.toplevel).unwrap_or_else(|_| src.toplevel.clone());
        let source = Source { repo: top.to_str().context(msg::cp::SOURCE_NOT_UTF8)?.to_string(), commit };
        let items = build(&ctx, &src, &source.commit, args, &id, &mut notes)?;
        (source, src, items)
    };
    if items.is_empty() {
        bail!(msg::cp::EMPTY_PLAN);
    }
    validate(&items)?;

    let mut report = Report {
        id: id.clone(),
        dry_run: args.dry_run,
        applied: false,
        source: ReportSource { repo: source.repo.clone(), commit: source.commit.clone() },
        files: Vec::new(),
        unresolved: Vec::new(),
        notes: Vec::new(),
        plan_file: None,
    };

    // `--plan <file>` writes the plan and stops.
    if let Some(file) = &args.plan {
        if !args.dry_run {
            fs::write(file, plan_of(&source, &items).to_toml()).with_context(|| msg::fs::cannot_write(file.display()))?;
        }
        report.files = report_files(&items);
        report.unresolved = unresolved_names(&items);
        report.notes = notes;
        report.plan_file = Some(file.clone());
        return finish(args, &report, true, || {
            if args.dry_run {
                println!("{}", msg::cp::DRY_RUN);
            } else {
                println!("{}", msg::cp::plan_written(file.display()));
            }
        });
    }

    // A range with no author: the editor, at a terminal.
    if !unresolved_names(&items).is_empty() && !args.dry_run {
        let file = ctx.store.dir().join("copies").join(format!("{id}.toml"));
        fs::create_dir_all(file.parent().context(msg::cp::PLAN_NO_DIR)?).with_context(|| msg::fs::cannot_create(file.display()))?;
        fs::write(&file, format!("{}{}", msg::cp::EDIT_HELP, plan_of(&source, &items).to_toml()))
            .with_context(|| msg::fs::cannot_write(file.display()))?;
        report.plan_file = Some(file.clone());
        if edit(repo, &file)? {
            let text = fs::read_to_string(&file).with_context(|| msg::fs::cannot_read(file.display()))?;
            let checked = Plan::parse(&text, &id).and_then(|plan| {
                if plan.source.repo != source.repo || plan.source.commit != source.commit {
                    bail!(msg::cp::SOURCE_CHANGED);
                }
                let items = load_plan(&src, &plan)?;
                validate(&items)?;
                Ok(items)
            });
            match checked {
                Ok(edited) => items = edited,
                Err(e) => bail!(msg::cp::plan_invalid(format!("{e:#}"), file.display())),
            }
        }
        if unresolved_names(&items).is_empty() {
            let _ = fs::remove_file(&file);
            report.plan_file = None;
        }
    }

    report.files = report_files(&items);
    report.unresolved = unresolved_names(&items);
    if !report.unresolved.is_empty() {
        report.notes = notes;
        let plan_file = report.plan_file.clone();
        return finish(args, &report, false, || {
            say(msg::cp::unresolved_count(report.unresolved.len()));
            for name in &report.unresolved {
                say(msg::cp::unresolved_item(name));
            }
            match &plan_file {
                Some(file) => say(msg::cp::set_authors(file.display())),
                None => say(msg::cp::GIVE_AUTHORS),
            }
        });
    }

    // No destination file exists, and git-ai records checkpoints here.
    for item in &items {
        let dest = &item.file.dest;
        let abs = repo.toplevel.join(dest);
        if abs.symlink_metadata().is_ok() || repo.git_ok(["ls-files", "--error-unmatch", "--", dest])? {
            bail!(msg::cp::dest_exists(dest));
        }
        let mut parent = abs.parent();
        while let Some(dir) = parent.filter(|d| *d != repo.toplevel && d.starts_with(&repo.toplevel)) {
            match dir.symlink_metadata() {
                Ok(meta) if !meta.is_dir() => bail!(msg::cp::parent_not_dir(dir.display(), dest)),
                _ => {}
            }
            parent = dir.parent();
        }
    }
    identical_line_notes(&items, &mut notes);
    report.notes = notes;
    if args.dry_run {
        return finish(args, &report, true, || println!("{}", msg::cp::DRY_RUN));
    }
    if !ctx.gitai.service_available(&repo.toplevel) {
        bail!(msg::approve::GITAI_UNAVAILABLE);
    }
    if let Some(problem) = crate::doctor::allowed(&ctx)? {
        bail!(problem);
    }

    // Store the plan, then replay each file.
    let mut record = CopyRecord {
        id: id.clone(),
        worktree: repo.toplevel.clone(),
        created_at: now(),
        source: source_label(&src)?,
        source_commit: source.commit.clone(),
        files: items
            .iter()
            .map(|i| CopyFile {
                path: i.file.path.clone(),
                dest: i.file.dest.clone(),
                blob: i.blob.clone(),
                mode: i.mode.clone(),
                text: i.file.is_text(),
                ranges: i.file.ranges.clone(),
                commit: None,
                committed_at: None,
                check: None,
                detail: None,
            })
            .collect(),
        failed: false,
    };
    {
        let lock = ctx.store.lock()?;
        // An earlier copy to the same destination that was never committed is given up.
        for mut old in lock.copies()? {
            if old.worktree != repo.toplevel {
                continue;
            }
            let before = old.files.len();
            old.files.retain(|f| f.commit.is_some() || !record.files.iter().any(|n| n.dest == f.dest));
            if old.files.is_empty() {
                lock.delete_copy(&old.id)?;
            } else if old.files.len() != before {
                lock.put_copy(&old)?;
            }
        }
        lock.put_copy(&record)?;
    }
    for item in &items {
        replay(&ctx, item).with_context(|| msg::cp::copy_stopped(&item.file.dest))?;
    }

    // Stage the files. The blob that git stores is what the commit must hold.
    let mut add = vec!["add".to_string(), "--".to_string()];
    add.extend(items.iter().map(|i| i.file.dest.clone()));
    repo.git(&add)?;
    let mut changed = false;
    for file in &mut record.files {
        let staged = repo.git(["rev-parse", &format!(":{}", file.dest)])?;
        if staged != file.blob {
            // A filter of this repository (for example `text=auto`) changed the bytes that git stores.
            report.notes.push(msg::cp::filter_changed(&file.dest));
            file.blob = staged;
            changed = true;
        }
    }
    if changed {
        ctx.store.lock()?.put_copy(&record)?;
    }
    report.applied = true;
    let count = items.len();
    finish(args, &report, true, || {
        println!("{}", msg::cp::copied(count));
        println!("{}", msg::cp::TRAILER_NOTE);
    })
}

fn unresolved_names(items: &[Item]) -> Vec<String> {
    items
        .iter()
        .flat_map(|i| {
            i.file.ranges.iter().filter(|r| r.who == Who::Unresolved).map(move |r| {
                if r.start == r.end {
                    msg::cp::unresolved_line(&i.file.dest, r.start)
                } else {
                    msg::cp::unresolved_lines(&i.file.dest, r.start, r.end)
                }
            })
        })
        .collect()
}

/// Prints the report, then the last lines for a person.
fn finish(args: &Args, report: &Report, ok: bool, last: impl FnOnce()) -> Result<bool> {
    if args.json {
        println!("{}", serde_json::to_string_pretty(report)?);
        return Ok(ok);
    }
    print_summary(&report.files);
    for note in &report.notes {
        println!("{}", msg::cp::note(note));
    }
    last();
    Ok(ok)
}

/// Writes one file in stages, with one checkpoint after each stage. The last
/// stage makes the file equal to the source, byte for byte.
fn replay(ctx: &Ctx, item: &Item) -> Result<()> {
    let worktree = &ctx.repo.toplevel;
    let path = worktree.join(&item.file.dest);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| msg::fs::cannot_create(parent.display()))?;
    }
    if item.file.symlink {
        return std::os::unix::fs::symlink(OsStr::from_bytes(&item.bytes), &path)
            .with_context(|| msg::cp::cannot_create_symlink(path.display()));
    }
    let write = |bytes: &[u8]| -> Result<()> {
        fs::write(&path, bytes).with_context(|| msg::fs::cannot_write(path.display()))?;
        if item.mode == MODE_EXEC {
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755))
                .with_context(|| msg::init::cannot_make_executable(path.display()))?;
        }
        Ok(())
    };
    if item.file.binary {
        return write(&item.bytes);
    }

    let lines = item.lines();
    let authors = item.file.line_authors();
    let stage = |included: &[bool]| -> Vec<u8> {
        lines.iter().zip(included).filter(|(_, yes)| **yes).flat_map(|(line, _)| line.iter().copied()).collect()
    };
    let files = [path.clone()];
    let mut included: Vec<bool> = authors.iter().map(|who| **who == Who::Human).collect();

    // 1. The human lines.
    if included.iter().any(|yes| *yes) || lines.is_empty() {
        write(&stage(&included))?;
        if !lines.is_empty() {
            ctx.gitai.checkpoint_known_human(worktree, &files, &full_version())?;
        }
    }
    // 2. The lines of each AI session, in the order of their first line.
    let mut sessions: Vec<&Who> = Vec::new();
    for who in authors.iter().copied() {
        if matches!(who, Who::Ai { .. }) && !sessions.contains(&who) {
            sessions.push(who);
        }
    }
    for who in sessions {
        let Who::Ai { agent, model, session } = who else { continue };
        ctx.gitai.checkpoint_before_ai(worktree, &files)?;
        for (slot, author) in included.iter_mut().zip(&authors) {
            *slot |= *author == who;
        }
        write(&stage(&included))?;
        ctx.gitai.checkpoint_ai(worktree, &files, agent, model, session)?;
    }
    if fs::read(&path).ok().as_deref() != Some(item.bytes.as_slice()) {
        bail!(msg::cp::not_equal(&item.file.dest));
    }
    Ok(())
}
