//! Test harness: temporary git repositories with worktrees, the real `git`
//! and the real `git-ai`.
//!
//! Each [`Env`] has its own home directory, and so its own git config, its
//! own git-ai config and its own git-ai service. Every command runs with a
//! clean environment. Reason: inside an agent session, the agent's own git-ai
//! hooks attribute changes made by test commands to that session. With a
//! separate service, nothing outside the test can add a checkpoint.
//!
//! The home directory is in `/tmp` because git-ai puts a Unix socket below
//! it, and a socket path has a length limit.

#![allow(dead_code)]

use std::ffi::OsStr;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;

/// How long a test waits for a detached process or the git-ai service. It
/// is long because the repositories can be on a slow file system
/// (`PROV_TEST_REPOS`), where the whole suite takes ten times longer.
pub const WAIT_SECS: u64 = 120;

pub const AGENT_NAME: &str = "Claude";
pub const AGENT_EMAIL: &str = "noreply@anthropic.com";
pub const HUMAN_EMAIL: &str = "human@example.invalid";
pub const FIXTURE_MODEL: &str = "claude-fable-5-2";

pub struct Out {
    pub ok: bool,
    pub stdout: String,
    pub stderr: String,
}

impl Out {
    #[track_caller]
    pub fn success(self) -> Self {
        assert!(self.ok, "command failed\nstdout: {}\nstderr: {}", self.stdout, self.stderr);
        self
    }

    #[track_caller]
    pub fn failure(self) -> Self {
        assert!(!self.ok, "command succeeded, but a failure was expected\nstdout: {}\nstderr: {}", self.stdout, self.stderr);
        self
    }
}

pub struct Env {
    home: tempfile::TempDir,
    /// Set with `PROV_TEST_REPOS`: the repositories are here, not in the home.
    repos: Option<tempfile::TempDir>,
    path: String,
    /// A process that stands for the agent of each session. `prov` waits
    /// for it to exit before it archives the transcript.
    agents: std::sync::Mutex<std::collections::HashMap<String, std::process::Child>>,
}

/// The directory of a program on `PATH`.
fn dir_of(program: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?).find(|dir| dir.join(program).is_file())
}

impl Env {
    /// `None` if git-ai is not installed: the test is then skipped.
    pub fn new() -> Option<Env> {
        let Some(gitai_dir) = dir_of("git-ai") else {
            // The local checks (ci/run.sh) set this: there, a skip is a failure.
            assert!(std::env::var_os("PROV_TEST_REQUIRE_GITAI").is_none(), "git-ai is not installed, and PROV_TEST_REQUIRE_GITAI is set");
            eprintln!("SKIPPED: git-ai is not installed");
            return None;
        };
        let git_dir = dir_of("git").expect("git is not installed");
        let home = tempfile::Builder::new().prefix("prov-t-").tempdir_in("/tmp").unwrap();
        // The tests run a copy of the binary on the local disk, as an
        // installed binary is. A build directory can be on a slow shared
        // mount, where the start of a process alone exceeds the hook limit.
        let prov_dir = home.path().join("bin");
        std::fs::create_dir_all(&prov_dir).unwrap();
        std::fs::copy(env!("CARGO_BIN_EXE_prov"), prov_dir.join("prov")).unwrap();
        let path = std::env::join_paths([prov_dir.as_path(), gitai_dir.as_path(), git_dir.as_path(), Path::new("/usr/bin"), Path::new("/bin")])
            .unwrap()
            .into_string()
            .unwrap();
        // `PROV_TEST_REPOS=<dir>` puts the repositories and worktrees on
        // another file system, for example a slow shared mount. The home
        // stays in `/tmp`: the git-ai sockets need a local, short path.
        let repos = std::env::var_os("PROV_TEST_REPOS").map(|dir| {
            std::fs::create_dir_all(&dir).unwrap();
            tempfile::Builder::new().prefix("prov-r-").tempdir_in(dir).unwrap()
        });
        let env = Env { home, repos, path, agents: std::sync::Mutex::default() };
        let socket = env.home().join(".git-ai/internal/daemon/trace2.sock");
        // The trace2 target is what `git-ai install` writes to the global git
        // config. Through it, the service sees each commit.
        std::fs::write(
            env.home().join(".gitconfig"),
            format!(
                "[user]\n\tname = Test Human\n\temail = {HUMAN_EMAIL}\n[init]\n\tdefaultBranch = main\n\
                 [trace2]\n\teventNesting = 0\n\teventTarget = af_unix:stream:{}\n",
                socket.display()
            ),
        )
        .unwrap();
        env.run(env.home(), "git-ai", ["bg", "start"], &[], None).success();
        assert!(socket.exists(), "the git-ai service has no trace2 socket at {}", socket.display());
        Some(env)
    }

    pub fn home(&self) -> &Path {
        self.home.path()
    }

    /// The directory of the test repositories and their worktrees.
    pub fn repos(&self) -> &Path {
        self.repos.as_ref().map_or(self.home.path(), |dir| dir.path())
    }

    pub fn run<I, S>(&self, dir: &Path, program: &str, args: I, envs: &[(&str, &str)], stdin: Option<&str>) -> Out
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut cmd = Command::new(program);
        cmd.current_dir(dir)
            .args(args)
            .env_clear()
            .env("HOME", self.home())
            .env("PATH", &self.path)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_EDITOR", "true")
            .env("GIT_MERGE_AUTOEDIT", "no")
            .env("LC_ALL", "C")
            .envs(envs.iter().copied())
            .stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = cmd.spawn().unwrap_or_else(|e| panic!("cannot run {program}: {e}"));
        if let Some(text) = stdin {
            child.stdin.take().unwrap().write_all(text.as_bytes()).unwrap();
        }
        let out = child.wait_with_output().unwrap();
        Out {
            ok: out.status.success(),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        }
    }

    /// Starts `program` with the environment of [`Env::run`], and returns at
    /// once, for a test that stops the process.
    pub fn spawn<I, S>(&self, dir: &Path, program: &str, args: I) -> std::process::Child
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        Command::new(program)
            .current_dir(dir)
            .args(args)
            .env_clear()
            .env("HOME", self.home())
            .env("PATH", &self.path)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("LC_ALL", "C")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap_or_else(|e| panic!("cannot run {program}: {e}"))
    }

    /// `prov` with a controlling terminal, as a human at a keyboard has.
    /// `input` is what the human types. Standard output and standard error
    /// are captured as in [`Env::run`].
    pub fn prov_tty<I, S>(&self, dir: &Path, args: I, input: &str) -> Out
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.prov_tty_env(dir, args, input, &[])
    }

    /// [`Env::prov_tty`] with more environment variables.
    pub fn prov_tty_env<I, S>(&self, dir: &Path, args: I, input: &str, envs: &[(&str, &str)]) -> Out
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        use std::os::fd::FromRawFd;
        use std::os::unix::process::CommandExt;
        let (mut master, mut slave) = (0, 0);
        // SAFETY: openpty writes two file descriptors to valid integers.
        let rc = unsafe { libc::openpty(&raw mut master, &raw mut slave, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut()) };
        assert_eq!(rc, 0, "openpty failed");
        let mut cmd = Command::new("prov");
        cmd.current_dir(dir)
            .args(args)
            .env_clear()
            .env("HOME", self.home())
            .env("PATH", &self.path)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_EDITOR", "true")
            .env("LC_ALL", "C")
            .envs(envs.iter().copied())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // SAFETY: setsid and ioctl are async-signal-safe. The new session
        // takes the pseudo-terminal as its controlling terminal, so the
        // child can open /dev/tty.
        unsafe {
            cmd.pre_exec(move || {
                libc::setsid();
                if libc::ioctl(slave, libc::TIOCSCTTY as _, 0) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let child = cmd.spawn().expect("cannot run prov");
        // SAFETY: both descriptors are open and owned here.
        unsafe { libc::close(slave) };
        let mut terminal = unsafe { std::fs::File::from_raw_fd(master) };
        terminal.write_all(input.as_bytes()).unwrap();
        let out = child.wait_with_output().unwrap();
        drop(terminal);
        Out {
            ok: out.status.success(),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        }
    }

    /// A repository with one human commit and nothing of `prov`: what a
    /// person has before `prov init`.
    pub fn plain_repo(&self, name: &str) -> PathBuf {
        let dir = self.repos().join(name);
        std::fs::create_dir_all(&dir).unwrap();
        self.git(&dir, ["init", "-q", "-b", "main", "."]);
        std::fs::write(dir.join("README.md"), format!("# {name}\n")).unwrap();
        self.git(&dir, ["add", "-A"]);
        self.git(&dir, ["commit", "-q", "-m", "Start"]);
        dir
    }

    /// What `git-ai install-hooks` writes for Claude Code: the checkpoint
    /// hooks in the user's settings. The hooks never run in a test.
    pub fn install_gitai_integration(&self) {
        let dir = self.home().join(".claude");
        std::fs::create_dir_all(&dir).unwrap();
        let hook = serde_json::json!([{"matcher": "*", "hooks": [{"type": "command", "command": "git-ai checkpoint claude --hook-input stdin"}]}]);
        std::fs::write(dir.join("settings.json"), serde_json::json!({"hooks": {"PreToolUse": hook, "PostToolUse": hook}}).to_string()).unwrap();
    }

    /// Stops the git-ai service of this environment and waits until its
    /// process is gone. `git-ai bg shutdown` returns before that, and a
    /// `git-ai bg start` in that interval does nothing.
    pub fn stop_service(&self) {
        let pid_file = self.home().join(".git-ai/internal/daemon/daemon.pid.json");
        let pid = std::fs::read_to_string(&pid_file)
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .and_then(|v| v["pid"].as_u64());
        self.run(self.home(), "git-ai", ["bg", "shutdown"], &[], None).success();
        let Some(pid) = pid else { panic!("no pid in {}", pid_file.display()) };
        let begin = std::time::Instant::now();
        // A zombie has exited: where nothing reaps it (a container with no
        // init process), it stays in /proc with the state `Z`.
        let running = |pid: u64| {
            std::fs::read_to_string(format!("/proc/{pid}/stat"))
                .is_ok_and(|stat| stat.rsplit_once(')').is_some_and(|(_, rest)| !rest.trim_start().starts_with('Z')))
        };
        while running(pid) {
            assert!(begin.elapsed() < std::time::Duration::from_secs(WAIT_SECS), "the git-ai service {pid} did not exit");
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }

    /// git as the human. Panics if git fails.
    #[track_caller]
    pub fn git<I, S>(&self, dir: &Path, args: I) -> String
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.run(dir, "git", args, &[], None).success().stdout.trim_end().to_string()
    }

    /// git as the human; the caller looks at the result.
    pub fn try_git<I, S>(&self, dir: &Path, args: I) -> Out
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.run(dir, "git", args, &[], None)
    }

    /// git as Claude Code runs it: the agent identity, no signing, and the
    /// agent's environment variables.
    pub fn agent_git<I, S>(&self, dir: &Path, session: &str, args: I) -> Out
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let envs = [
            ("GIT_AUTHOR_NAME", AGENT_NAME),
            ("GIT_AUTHOR_EMAIL", AGENT_EMAIL),
            ("GIT_COMMITTER_NAME", AGENT_NAME),
            ("GIT_COMMITTER_EMAIL", AGENT_EMAIL),
            ("GIT_CONFIG_COUNT", "1"),
            ("GIT_CONFIG_KEY_0", "commit.gpgsign"),
            ("GIT_CONFIG_VALUE_0", "false"),
            ("CLAUDECODE", "1"),
            ("CLAUDE_CODE_SESSION_ID", session),
        ];
        self.run(dir, "git", args, &envs, None)
    }

    pub fn prov<I, S>(&self, dir: &Path, args: I) -> Out
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.run(dir, "prov", args, &[], None)
    }

    /// A repository with one human commit that holds `.provenance.toml`, the
    /// hook shims and the git options that `prov init` sets.
    pub fn repo(&self, name: &str) -> PathBuf {
        let dir = self.repos().join(name);
        std::fs::create_dir_all(&dir).unwrap();
        self.git(&dir, ["init", "-q", "-b", "main", "."]);
        std::fs::write(
            dir.join(".provenance.toml"),
            "required_version = \"=0.1.0\"\nagents = [\"claude\"]\nlogs_dir = \"llm-logs\"\n\
             protected = [\".provenance.toml\", \".claude/**\", \".github/workflows/**\", \"CLAUDE.md\", \"AGENTS.md\"]\n",
        )
        .unwrap();
        std::fs::write(dir.join(".gitignore"), "/llm-logs/\n").unwrap();
        std::fs::write(dir.join("README.md"), format!("# {name}\n")).unwrap();
        self.git(&dir, ["add", "-A"]);
        self.git(&dir, ["commit", "-q", "-m", "Set up"]);
        self.install_shims(&dir);
        // Every branch that is not an agent branch is a human branch, so the
        // merge commit is forced for the whole repository.
        self.git(&dir, ["config", "merge.ff", "false"]);
        self.git(&dir, ["config", "pull.ff", "only"]);
        dir
    }

    pub fn hooks_dir(&self, repo: &Path) -> PathBuf {
        PathBuf::from(self.git(repo, ["rev-parse", "--path-format=absolute", "--git-path", "hooks"]))
    }

    pub fn install_shims(&self, repo: &Path) {
        use std::os::unix::fs::PermissionsExt;
        let hooks = self.hooks_dir(repo);
        std::fs::create_dir_all(&hooks).unwrap();
        for name in ["pre-commit", "prepare-commit-msg", "post-commit", "post-merge"] {
            let path = hooks.join(name);
            std::fs::write(&path, format!("#!/bin/sh\nexec prov hook git {name} \"$@\"\n")).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    /// Removes the shims, as in a clone where `prov init` did not run, or on a forge.
    pub fn remove_shims(&self, repo: &Path) {
        for name in ["pre-commit", "prepare-commit-msg", "post-commit", "post-merge"] {
            let _ = std::fs::remove_file(self.hooks_dir(repo).join(name));
        }
    }

    /// An agent worktree on the new branch `agent/claude/<name>`, from `main`.
    pub fn worktree(&self, repo: &Path, name: &str) -> PathBuf {
        self.worktree_from(repo, name, "main")
    }

    /// An agent worktree on the new branch `agent/claude/<name>`, from `start`.
    pub fn worktree_from(&self, repo: &Path, name: &str, start: &str) -> PathBuf {
        let dir = self.repos().join(format!("{}.worktrees", repo.file_name().unwrap().to_str().unwrap())).join(name);
        self.git(repo, ["worktree", "add", "-q", "-b", &branch(name), dir.to_str().unwrap(), start]);
        dir
    }

    /// Where Claude Code would keep the transcript of a session.
    pub fn transcript_path(&self, session: &str) -> PathBuf {
        self.home().join(".claude/projects/test").join(format!("{session}.jsonl"))
    }

    /// Writes the recorded transcript of a session, with one subagent file.
    pub fn write_transcript(&self, session: &str, cwd: &Path) -> PathBuf {
        let path = self.transcript_path(session);
        let side = path.parent().unwrap().join(session).join("subagents");
        std::fs::create_dir_all(&side).unwrap();
        std::fs::write(&path, fill(include_str!("../fixtures/claude-code/transcript.jsonl"), session, cwd, &path)).unwrap();
        std::fs::write(side.join("agent-a1.jsonl"), include_str!("../fixtures/claude-code/subagent.jsonl")).unwrap();
        path
    }

    /// Runs `prov hook claude-code <event>` with the recorded payload of the event.
    pub fn hook(&self, cwd: &Path, event: &str, session: &str) -> Out {
        let out = self.hook_from(cwd, event, session, self.agent_pid(session));
        if event == "SessionEnd" {
            // The agent exits right after its SessionEnd hook.
            self.kill_agent(session);
        }
        out
    }

    /// The ID of the process that stands for the agent of a session. The
    /// first call starts it.
    pub fn agent_pid(&self, session: &str) -> u32 {
        let mut agents = self.agents.lock().unwrap();
        agents
            .entry(session.to_string())
            .or_insert_with(|| Command::new("sleep").arg("100000").stdin(Stdio::null()).spawn().expect("cannot run sleep"))
            .id()
    }

    /// The agent process of a session exits, with no event: a crash, or the
    /// end of a session whose `SessionEnd` hook already ran.
    pub fn kill_agent(&self, session: &str) {
        if let Some(mut child) = self.agents.lock().unwrap().remove(session) {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    /// The same, from an agent process with the ID `agent_pid`.
    pub fn hook_from(&self, cwd: &Path, event: &str, session: &str, agent_pid: u32) -> Out {
        let template = match event {
            "SessionStart" => include_str!("../fixtures/claude-code/SessionStart.json"),
            "UserPromptSubmit" => include_str!("../fixtures/claude-code/UserPromptSubmit.json"),
            "Stop" => include_str!("../fixtures/claude-code/Stop.json"),
            "SessionEnd" => include_str!("../fixtures/claude-code/SessionEnd.json"),
            other => panic!("no recorded payload for {other}"),
        };
        let payload = fill(template, session, cwd, &self.transcript_path(session));
        let pid = agent_pid.to_string();
        let envs = [("CLAUDE_PID", pid.as_str()), ("CLAUDECODE", "1"), ("CLAUDE_CODE_SESSION_ID", session)];
        self.run(cwd, "prov", ["hook", "claude-code", event], &envs, Some(&payload))
    }

    /// Starts a session: the transcript and the `SessionStart` event.
    pub fn start_session(&self, worktree: &Path, session: &str) -> Out {
        self.write_transcript(session, worktree);
        self.hook(worktree, "SessionStart", session)
    }

    /// Ends a session and waits for the archive step.
    #[track_caller]
    pub fn end_session(&self, worktree: &Path, session: &str) -> Value {
        self.hook(worktree, "SessionEnd", session).success();
        self.wait_state(worktree, session, "archived")
    }

    pub fn record(&self, repo: &Path, session: &str) -> Option<Value> {
        let common = self.git(repo, ["rev-parse", "--path-format=absolute", "--git-common-dir"]);
        let path = Path::new(&common).join("provenance/sessions").join(format!("{session}.json"));
        std::fs::read(path).ok().map(|bytes| serde_json::from_slice(&bytes).unwrap())
    }

    #[track_caller]
    pub fn state(&self, repo: &Path, session: &str) -> String {
        self.record(repo, session).expect("no session record")["state"].as_str().unwrap().to_string()
    }

    #[track_caller]
    pub fn wait_state(&self, repo: &Path, session: &str, state: &str) -> Value {
        let start = Instant::now();
        loop {
            let record = self.record(repo, session).expect("no session record");
            if record["state"] == state {
                return record;
            }
            assert!(start.elapsed() < Duration::from_secs(WAIT_SECS), "session {session} did not become {state}: {record}");
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    pub fn agent_checkpoint(&self, worktree: &Path, payload: &Value) {
        self.run(worktree, "git-ai", ["checkpoint", "agent-v1", "--hook-input", "stdin"], &[], Some(&payload.to_string()))
            .success();
    }

    /// A file edit by the agent's tool, with the checkpoints that the agent's
    /// own git-ai hooks make around it. Those hooks do not know the model.
    pub fn agent_edit(&self, worktree: &Path, file: &str, content: &str, session: &str) {
        let path = worktree.join(file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        self.agent_checkpoint(
            worktree,
            &serde_json::json!({"type": "human", "repo_working_dir": worktree, "will_edit_filepaths": [path]}),
        );
        std::fs::write(&path, content).unwrap();
        self.agent_checkpoint(
            worktree,
            &serde_json::json!({"type": "ai_agent", "repo_working_dir": worktree, "edited_filepaths": [path],
                "agent_name": "claude", "model": "unknown", "conversation_id": session}),
        );
    }

    /// A file that a process wrote with no checkpoint at all.
    #[allow(clippy::unused_self)] // a method like the other edits, for readable tests
    pub fn plain_write(&self, worktree: &Path, file: &str, content: &str) {
        let path = worktree.join(file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    /// A file that git-ai knows a human wrote: the human edited it in the
    /// agent worktree. Its lines are doubtful lines in an agent commit.
    pub fn human_edit(&self, worktree: &Path, file: &str, content: &str) {
        self.plain_write(worktree, file, content);
        let path = worktree.join(file);
        self.run(
            worktree,
            "git-ai",
            [OsStr::new("checkpoint"), OsStr::new("known_human"), OsStr::new("--editor"), OsStr::new("test"),
             OsStr::new("--editor-version"), OsStr::new("0"), OsStr::new("--"), path.as_os_str()],
            &[],
            None,
        )
        .success();
    }

    /// Stages everything and commits as the agent.
    pub fn agent_commit(&self, worktree: &Path, session: &str, message: &str) -> Out {
        self.git(worktree, ["add", "-A"]);
        self.agent_git(worktree, session, ["commit", "-q", "-m", message])
    }

    /// Waits until git-ai has written the notes of new commits.
    #[track_caller]
    pub fn await_notes(&self, repo: &Path) {
        // Under load, `git-ai await` once gave up after 6 seconds with all
        // notes written ("could not finish (0 metrics, 0 notes remaining)").
        let mut last = None;
        for _ in 0..3 {
            let out = self.run(repo, "git-ai", ["await", "--timeout", "60"], &[], None);
            if out.ok {
                return;
            }
            last = Some(out);
        }
        last.unwrap().success();
    }

    /// `git-ai diff <commit> --json`, as a list of
    /// `(path, tool, session ID, model)` for AI ranges and `(path, "human", "", "")`
    /// or `(path, "untracked", "", "")` for the others.
    #[track_caller]
    pub fn authorship(&self, repo: &Path, commit: &str) -> Vec<(String, String, String, String)> {
        self.await_notes(repo);
        let out = self.run(repo, "git-ai", ["diff", commit, "--json"], &[], None).success();
        let json: Value = serde_json::from_str(&out.stdout).unwrap();
        let mut rows = Vec::new();
        for hunk in json["hunks"].as_array().unwrap() {
            if hunk["hunk_kind"] != "addition" {
                continue;
            }
            let path = hunk["file_path"].as_str().unwrap().to_string();
            let row = if let Some(key) = hunk["session_id"].as_str() {
                let agent = &json["sessions"][key]["agent_id"];
                let field = |name: &str| agent[name].as_str().unwrap_or("").to_string();
                (path, field("tool"), field("id"), field("model"))
            } else if hunk["human_id"].is_string() {
                (path, "human".into(), String::new(), String::new())
            } else {
                (path, "untracked".into(), String::new(), String::new())
            };
            rows.push(row);
        }
        rows.sort();
        rows
    }

    /// The trailers of a commit, as `Key: value` lines.
    pub fn trailers(&self, repo: &Path, commit: &str) -> Vec<String> {
        self.git(repo, ["log", "-1", "--format=%(trailers:only,unfold)", commit])
            .lines()
            .filter(|l| !l.is_empty())
            .map(String::from)
            .collect()
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        // `PROV_TEST_KEEP=1` keeps the home and the repositories of a failed test.
        if std::thread::panicking() && std::env::var_os("PROV_TEST_KEEP").is_some() {
            eprintln!("PROV_TEST_KEEP: kept {} and {}", self.home().display(), self.repos().display());
            self.home.disable_cleanup(true);
            if let Some(repos) = &mut self.repos {
                repos.disable_cleanup(true);
            }
        }
        for (_, mut child) in self.agents.lock().unwrap().drain() {
            let _ = child.kill();
            let _ = child.wait();
        }
        let _ = self.run(self.home(), "git-ai", ["bg", "shutdown"], &[], None);
    }
}

pub fn branch(name: &str) -> String {
    format!("agent/claude/{name}")
}

fn fill(template: &str, session: &str, cwd: &Path, transcript: &Path) -> String {
    template
        .replace("{{SESSION}}", session)
        .replace("{{CWD}}", cwd.to_str().unwrap())
        .replace("{{TRANSCRIPT}}", transcript.to_str().unwrap())
}

/// SHA-256 of the decompressed bytes of a gzip file, as hex. This is how the
/// spec says to check an archive.
pub fn sha256_of_gunzipped(path: &Path) -> String {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let mut tar = Vec::new();
    flate2::read::GzDecoder::new(std::fs::File::open(path).unwrap()).read_to_end(&mut tar).unwrap();
    Sha256::digest(&tar).iter().fold(String::new(), |mut s, b| {
        use std::fmt::Write as _;
        let _ = write!(s, "{b:02x}");
        s
    })
}

/// The entry names of a `.tar.gz` file, in archive order.
pub fn tar_names(path: &Path) -> Vec<String> {
    let gz = flate2::read::GzDecoder::new(std::fs::File::open(path).unwrap());
    tar::Archive::new(gz).entries().unwrap().map(|e| e.unwrap().path().unwrap().to_string_lossy().into_owned()).collect()
}
