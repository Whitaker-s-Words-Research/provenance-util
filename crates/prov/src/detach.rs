//! Starting a process that outlives the hook that starts it.

use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};

/// Makes `cmd` a detached process: its own process session, no standard
/// streams, and no other file descriptor of the parent.
///
/// Reason for the last part: a hook inherits file descriptors from the
/// process that runs it. During `git commit`, git talks to a long-running
/// filter, for example `git-lfs filter-process`, through pipes, and waits at
/// the end until the filter exits. A detached process that keeps a copy of
/// such a pipe holds the commit open until it exits itself. The deferred
/// copy check waits for the note of that commit, so both waited 60 seconds.
pub fn detach(cmd: &mut Command) {
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    // SAFETY: setsid, close_range, getrlimit and fcntl are async-signal-safe
    // and touch no memory of the parent.
    unsafe {
        cmd.pre_exec(|| {
            libc::setsid();
            cloexec_from(3);
            Ok(())
        });
    }
}

/// Marks every file descriptor from `first` on close-on-exec. The pipe that
/// the standard library uses to report a failed exec is close-on-exec
/// already, so it keeps working.
fn cloexec_from(first: i32) {
    #[cfg(target_os = "linux")]
    {
        // SAFETY: a plain system call with integer arguments.
        let rc = unsafe {
            libc::syscall(libc::SYS_close_range, first.cast_unsigned(), libc::c_uint::MAX, libc::CLOSE_RANGE_CLOEXEC)
        };
        if rc == 0 {
            return;
        }
    }
    // Older kernels and other systems: one call for each possible descriptor,
    // up to the limit of the process (at most 65,536).
    let mut limit = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
    // SAFETY: getrlimit writes to a valid struct.
    let max = if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &raw mut limit) } == 0 {
        i32::try_from(limit.rlim_cur.min(65_536)).unwrap_or(65_536)
    } else {
        4_096
    };
    for fd in first..max {
        // SAFETY: fcntl on a descriptor that may not exist returns an error.
        unsafe {
            libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
        }
    }
}
