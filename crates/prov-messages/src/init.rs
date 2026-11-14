//! `prov init`. The result of each action (`written`, `kept`, `set`,
//! `moved`) and its target are values of the JSON output, so they stay in
//! the code.

use std::fmt::{Debug, Display};

/// Also in `prov doctor`.
pub fn relative_hooks_path(value: impl Display) -> String {
    format!(
        "core.hooksPath is the relative path `{value}`, which is inside each worktree. Use an absolute path \
         outside every worktree, or unset it: git config --unset core.hooksPath"
    )
}

/// Also in `prov doctor`.
pub fn hooks_dir_in_worktree(dir: impl Display, worktree: impl Display) -> String {
    format!("the hooks directory {dir} is inside the worktree {worktree}. Use a directory outside every worktree.")
}

pub fn agent_env(var: impl Display) -> String {
    format!("`prov init` is for a human. {var} is set: an agent started this command.")
}

pub fn on_agent_branch(branch: impl Display) -> String {
    format!("`prov init` runs on a human branch, not on the agent branch `{branch}`")
}

pub fn config_exists(file: impl Display, agents: impl Debug) -> String {
    format!("{file} exists with agents = {agents:?}. Edit the file, or replace it with --force.")
}

pub fn config_invalid(error: impl Display) -> String {
    format!("{error}. Correct the file, or replace it with --force.")
}

pub const REPLACED_FILE: &str = "replaced the existing file";

pub fn version_warning(error: impl Display) -> String {
    format!("warning: {error}. The merge gate and `prov check` will refuse.")
}

pub fn no_adapter(agent: impl Display, file: impl Display) -> String {
    format!("no adapter for the agent `{agent}` in {file}")
}

pub fn no_identity(agent: impl Display) -> String {
    format!("no identity for `{agent}`")
}

pub fn settings_conflict(file: impl Display, conflicts: impl Display) -> String {
    format!("{file} sets other values: {conflicts}. Replace them with --force.")
}

pub const MERGED_SETTINGS: &str = "merged into the existing file; all other keys stay";

pub fn hook_and_local_exist(path: impl Display, local: impl Display) -> String {
    format!("{path} is not the shim of prov, and {local} exists too. Remove one, or replace the hook with --force.")
}

pub fn replaced_hook(name: impl Display) -> String {
    format!("replaced the existing hook; {name}.local stays")
}

pub fn cannot_move(path: impl Display) -> String {
    format!("cannot move {path}")
}

pub fn moved_hook(name: impl Display) -> String {
    format!("the existing hook is now {name}.local; prov runs it after its own work")
}

pub fn cannot_make_executable(path: impl Display) -> String {
    format!("cannot make {path} executable")
}

pub fn added_line(line: impl Display) -> String {
    format!("added {line}")
}

/// One action. The result is padded to 8 characters.
pub fn action(result: impl Display, target: impl Display) -> String {
    format!("{result:8} {target}")
}

pub fn action_detail(result: impl Display, target: impl Display, detail: impl Display) -> String {
    format!("{result:8} {target} ({detail})")
}

pub const DRY_RUN: &str = "Dry run: nothing changed.";

pub fn commit_these(files: impl Display) -> String {
    format!("Commit these files on this branch: {files}")
}

pub const THEN_DOCTOR: &str = "Then check the setup: prov doctor";
