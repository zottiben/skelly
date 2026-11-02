//! Own the engine's process group so cancelling a wrapper also stops its helpers.

use std::io;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, ExitStatus};

use nix::sys::signal::{killpg, Signal};
use nix::unistd::Pid;

pub(crate) struct EngineProcess {
    child: Child,
    reaped: bool,
}

impl EngineProcess {
    pub(crate) fn spawn(command: &mut Command) -> io::Result<Self> {
        Ok(Self {
            child: command.process_group(0).spawn()?,
            reaped: false,
        })
    }

    pub(crate) fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        let status = self.child.try_wait()?;
        self.reaped = status.is_some();
        Ok(status)
    }
}

#[cfg(test)]
/// Write an executable fixture script from a child shell, so this process never holds a write fd
/// on it. On Linux a process that another test thread is spawning keeps a copy of every open fd
/// until it execs, and running a script that any process can still write fails with `ETXTBSY`
/// ("Text file busy").
pub(crate) fn write_script(path: &std::path::Path, body: &str) {
    use std::os::unix::fs::PermissionsExt;
    let status = Command::new("/bin/sh")
        .args(["-c", r#"printf '%s' "$1" > "$2""#, "sh", body])
        .arg(path)
        .status()
        .unwrap();
    assert!(status.success(), "write {}", path.display());
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
}

impl Drop for EngineProcess {
    fn drop(&mut self) {
        // An unreaped leader reserves this PID/PGID even if it has just exited.
        // Never signal after try_wait reaped it: that ID could have been reused.
        if !self.reaped {
            if let Ok(pid) = i32::try_from(self.child.id()) {
                let _ = killpg(Pid::from_raw(pid), Signal::SIGKILL);
            }
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}
