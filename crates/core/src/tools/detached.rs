//! Detached remote runs (ADR-0009, decision 6; #44). The container command
//! runs on the host under `setsid`/`nohup` in a run directory that holds its
//! output (`out.log`), the wrapper's pid (`pid`) and, once it ends, its exit
//! code (`status`). Short ssh polls copy new output into the local stage log
//! and read the status, so a dropped connection costs a retry, not the run.

use std::fs::OpenOptions;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::thread::sleep;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::{Remote, argv, sh_quote};
use crate::error::{Error, Result, io_error};

/// ssh options for the calls of a detached run: besides batch mode, a dead
/// connection fails the call within about a minute instead of hanging.
const SSH_OPTS: [&str; 8] = [
    "-o",
    "BatchMode=yes",
    "-o",
    "ConnectTimeout=30",
    "-o",
    "ServerAliveInterval=15",
    "-o",
    "ServerAliveCountMax=4",
];

/// ssh's exit code for its own failures, as opposed to the remote command's.
const SSH_FAILED: i32 = 255;

/// Runs the remote command line of `cmd` (the ssh call `Tool::command`
/// built) detached on `remote`'s host, appending its output to `log`.
pub(super) fn run(tool: &str, remote: &Remote, cmd: &Command, log: &Path) -> Result<()> {
    static SEQ: AtomicU32 = AtomicU32::new(0);
    let line = cmd
        .get_args()
        .last()
        .expect("the ssh call ends with the remote line");
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let id = run_id(
        tool,
        std::process::id(),
        secs,
        SEQ.fetch_add(1, Ordering::Relaxed),
    );
    let engine = Engine {
        ssh_program: "ssh",
        host: &remote.host,
        policy: RetryPolicy::from_remote(remote),
        poll_every: Duration::from_secs(remote.poll_secs.into()),
    };
    engine.run(
        tool,
        log,
        &line.to_string_lossy(),
        &run_dir(&remote.root, &id),
    )
}

/// A run id unique per process and call, of `[A-Za-z0-9_-]` only.
pub(super) fn run_id(tool: &str, pid: u32, unix_secs: u64, seq: u32) -> String {
    let safe: String = tool
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!("{safe}-{pid}-{unix_secs}-{seq}")
}

/// `<root>/.shoreline-runs/<id>`, beside the mirrors and never inside one.
pub(super) fn run_dir(root: &Path, id: &str) -> PathBuf {
    root.join(".shoreline-runs").join(id)
}

/// `script` as a remote command line: the login shell (zsh on the GPU host)
/// sees one quoted word for `sh`, so it expands nothing.
fn remote_sh(script: &str) -> String {
    format!("sh -c {}", sh_quote(script))
}

/// The detached `sh` script: records its pid, runs the container line (as
/// `Tool::command` quoted it, so `$(id -u)` still expands), then writes the
/// exit code to `status` atomically.
pub(super) fn wrapper_script(container_line: &str, dir: &str) -> String {
    format!(
        "echo $$ >{dir}/pid; {container_line} >>{dir}/out.log 2>&1 </dev/null; \
         echo $? >{dir}/status.tmp && mv {dir}/status.tmp {dir}/status"
    )
}

/// Creates the run directory and `out.log`, then starts the wrapper outside
/// the ssh session: `setsid` where the host has it, `nohup` either way. All
/// three streams are redirected, so ssh returns at once.
pub(super) fn launch_line(container_line: &str, dir: &str) -> String {
    let wrapper = sh_quote(&wrapper_script(container_line, dir));
    format!(
        "mkdir -p {dir} && : >{dir}/out.log || exit 1; s=; command -v setsid >/dev/null 2>&1 && \
         s=setsid; $s nohup sh -c {wrapper} </dev/null >/dev/null 2>&1 &"
    )
}

/// Prints `SHORELINE-STATUS <state>`, then `out.log` from byte `offset`. The
/// state is read before the log, so an exit code means the log is complete.
/// States: an exit code; `running`; `dead` when the wrapper is gone without
/// an exit code; `missing` when nothing was started or the directory is gone.
pub(super) fn poll_line(dir: &str, offset: u64) -> String {
    format!(
        "cd {dir} 2>/dev/null || {{ echo 'SHORELINE-STATUS missing'; exit 0; }}; \
         if [ -e status ]; then s=$(cat status); \
         elif [ -s pid ] && kill -0 \"$(cat pid)\" 2>/dev/null; then s=running; \
         elif [ -e status ]; then s=$(cat status); \
         elif [ -s pid ]; then s=dead; \
         elif [ -e out.log ]; then s=running; \
         else s=missing; fi; \
         echo \"SHORELINE-STATUS $s\"; tail -c +{} out.log 2>/dev/null; true",
        offset + 1
    )
}

pub(super) fn cleanup_line(dir: &str) -> String {
    format!("rm -rf {dir}")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PollState {
    Running,
    Exited(i32),
    /// The wrapper ended without writing an exit code (killed, host restart).
    Dead,
    Missing,
}

/// The state header of a poll's output and the log bytes after it; `None`
/// for output without a well-formed header.
pub(super) fn parse_poll(out: &[u8]) -> Option<(PollState, &[u8])> {
    let (head, rest) = match out.iter().position(|&b| b == b'\n') {
        Some(i) => (&out[..i], &out[i + 1..]),
        None => (out, &[][..]),
    };
    let word = std::str::from_utf8(head.strip_prefix(b"SHORELINE-STATUS ")?).ok()?;
    let state = match word {
        "running" => PollState::Running,
        "dead" => PollState::Dead,
        "missing" => PollState::Missing,
        code => PollState::Exited(code.parse().ok()?),
    };
    Some((state, rest))
}

/// What to do after a poll answered `state`. `launched` says the launch is
/// known to have happened: its ssh call succeeded or a poll saw the command.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Next {
    Wait,
    Relaunch,
    Finished(i32),
    Fail(&'static str),
}

pub(super) fn next(launched: bool, state: PollState) -> Next {
    match state {
        PollState::Running => Next::Wait,
        PollState::Exited(code) => Next::Finished(code),
        // A launch whose ssh call failed before it created anything.
        PollState::Missing if !launched => Next::Relaunch,
        PollState::Missing => Next::Fail("the remote run directory is gone"),
        PollState::Dead => Next::Fail(
            "the remote command ended without an exit code (killed, or the host restarted)",
        ),
    }
}

/// Reconnect backoff: `initial * 2^retry`, capped at `max`.
#[derive(Clone, Copy, Debug)]
pub(super) struct RetryPolicy {
    pub attempts: u32,
    pub initial: Duration,
    pub max: Duration,
}

impl RetryPolicy {
    pub(super) fn from_remote(remote: &Remote) -> Self {
        Self {
            attempts: remote.reconnect_attempts,
            initial: Duration::from_secs(remote.reconnect_delay_secs.into()),
            max: Duration::from_secs(remote.reconnect_max_delay_secs.into()),
        }
    }

    pub(super) fn delay(&self, retry: u32) -> Duration {
        2u32.checked_pow(retry)
            .and_then(|f| self.initial.checked_mul(f))
            .map_or(self.max, |d| d.min(self.max))
    }
}

/// More consecutive failures than the policy allows.
pub(super) fn exhausted(failures: u32, attempts: u32) -> bool {
    failures > attempts
}

/// Appends `bytes` to the local stage log.
fn append_log(log: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    if bytes.is_empty() {
        return Ok(());
    }
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(log)
        .map_err(io_error(log))?;
    file.write_all(bytes).map_err(io_error(log))
}

/// The launch, poll and cleanup calls of one detached run. `ssh_program` is
/// `ssh` except in tests, which pass a shim.
pub(super) struct Engine<'a> {
    pub ssh_program: &'a str,
    pub host: &'a str,
    pub policy: RetryPolicy,
    pub poll_every: Duration,
}

impl Engine<'_> {
    pub(super) fn run(
        &self,
        tool: &str,
        log: &Path,
        container_line: &str,
        dir: &Path,
    ) -> Result<()> {
        let d = dir.to_string_lossy();
        let lost = || Error::RemoteLost {
            tool: tool.to_owned(),
            host: self.host.to_owned(),
            run_dir: dir.to_owned(),
            attempts: self.policy.attempts,
            log: log.to_owned(),
        };
        let mut launched = self.launch(tool, log, container_line, &d)?;
        let mut launch_failures = u32::from(!launched);
        let mut wait = if launched {
            self.poll_every
        } else {
            self.policy.delay(0)
        };
        let (mut offset, mut failures) = (0u64, 0u32);
        loop {
            sleep(wait);
            let Some((state, bytes)) = self.poll(log, &d, offset)? else {
                failures += 1;
                if exhausted(failures, self.policy.attempts) {
                    return Err(lost());
                }
                wait = self.policy.delay(failures - 1);
                continue;
            };
            failures = 0;
            append_log(log, &bytes)?;
            // Only bytes received count, so a retry asks for the same range.
            offset += bytes.len() as u64;
            match next(launched, state) {
                Next::Wait => {
                    launched = true;
                    wait = self.poll_every;
                }
                Next::Relaunch => {
                    launched = self.launch(tool, log, container_line, &d)?;
                    wait = self.poll_every;
                    if !launched {
                        launch_failures += 1;
                        if exhausted(launch_failures, self.policy.attempts) {
                            return Err(lost());
                        }
                        wait = self.policy.delay(launch_failures - 1);
                    }
                }
                Next::Finished(code) => {
                    self.cleanup(log, &d)?;
                    if code == 0 {
                        return Ok(());
                    }
                    return Err(Error::RemoteToolFailed {
                        tool: tool.to_owned(),
                        code,
                        log: log.to_owned(),
                    });
                }
                Next::Fail(why) => {
                    return Err(Error::RemoteRun {
                        tool: tool.to_owned(),
                        host: self.host.to_owned(),
                        message: format!("{why} ({d})"),
                        log: log.to_owned(),
                    });
                }
            }
        }
    }

    fn ssh(&self, line: String) -> Command {
        let mut cmd = Command::new(self.ssh_program);
        cmd.args(SSH_OPTS)
            .args([self.host, "--"])
            .arg(line)
            .stdin(Stdio::null());
        cmd
    }

    fn spawn_error(&self, source: std::io::Error) -> Error {
        Error::Spawn {
            program: self.ssh_program.to_owned(),
            source,
        }
    }

    /// Starts the wrapper, its ssh output going to the log. `Ok(false)` when
    /// ssh itself failed, so whether the command started is unknown; another
    /// non-zero exit is the remote shell failing, an error.
    fn launch(&self, tool: &str, log: &Path, container_line: &str, dir: &str) -> Result<bool> {
        let mut cmd = self.ssh(remote_sh(&launch_line(container_line, dir)));
        append_log(log, format!("$ {}\n", argv(&cmd).join(" ")).as_bytes())?;
        let out = OpenOptions::new()
            .append(true)
            .open(log)
            .map_err(io_error(log))?;
        let err = out.try_clone().map_err(io_error(log))?;
        let status = cmd
            .stdout(out)
            .stderr(err)
            .status()
            .map_err(|e| self.spawn_error(e))?;
        match status.code() {
            Some(0) => Ok(true),
            Some(code) if code != SSH_FAILED => Err(Error::ToolFailed {
                tool: tool.to_owned(),
                status,
                log: log.to_owned(),
            }),
            _ => Ok(false),
        }
    }

    /// One poll; `None` when the call failed, with a note in the log.
    fn poll(&self, log: &Path, dir: &str, offset: u64) -> Result<Option<(PollState, Vec<u8>)>> {
        let out = self
            .ssh(remote_sh(&poll_line(dir, offset)))
            .output()
            .map_err(|e| self.spawn_error(e))?;
        if out.status.success()
            && let Some((state, bytes)) = parse_poll(&out.stdout)
        {
            return Ok(Some((state, bytes.to_vec())));
        }
        let note = format!(
            "\nshoreline: polling {} failed ({})\n",
            self.host, out.status
        );
        append_log(log, note.as_bytes())?;
        append_log(log, &out.stderr)?;
        Ok(None)
    }

    /// Removes the run directory; a failure is noted in the log, not an error.
    fn cleanup(&self, log: &Path, dir: &str) -> Result<()> {
        let ok = self
            .ssh(remote_sh(&cleanup_line(dir)))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success());
        if ok {
            return Ok(());
        }
        append_log(
            log,
            format!("\nshoreline: could not remove {dir} on {}\n", self.host).as_bytes(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_ids_are_shell_safe_and_unique_per_call() {
        assert_eq!(run_id("col map", 1, 2, 3), "col_map-1-2-3");
        assert_eq!(run_id("a/b'c$", 3, 4, 0), "a_b_c_-3-4-0");
        assert_ne!(run_id("x", 1, 2, 0), run_id("x", 1, 2, 1));
    }

    #[test]
    fn run_dir_is_beside_the_mirrors() {
        assert_eq!(
            run_dir(Path::new("/r"), "x"),
            PathBuf::from("/r/.shoreline-runs/x")
        );
    }

    #[test]
    fn wrapper_records_pid_output_and_status() {
        assert_eq!(
            wrapper_script("'echo' 'hi'", "/d"),
            "echo $$ >/d/pid; 'echo' 'hi' >>/d/out.log 2>&1 </dev/null; \
             echo $? >/d/status.tmp && mv /d/status.tmp /d/status"
        );
    }

    #[test]
    fn launch_line_detaches_the_wrapper() {
        assert_eq!(
            launch_line("echo hi", "/d"),
            "mkdir -p /d && : >/d/out.log || exit 1; s=; command -v setsid >/dev/null 2>&1 && \
             s=setsid; $s nohup sh -c 'echo $$ >/d/pid; echo hi >>/d/out.log 2>&1 </dev/null; \
             echo $? >/d/status.tmp && mv /d/status.tmp /d/status' </dev/null >/dev/null 2>&1 &"
        );
    }

    #[test]
    fn launch_line_keeps_single_quotes_and_shell_words() {
        // `it's` as Tool::command quotes it, and docker's remote `--user`.
        let line = r"'printf' 'it'\''s' --user $(id -u):$(id -g)";
        let launch = launch_line(line, "/d");
        let wrapper = sh_quote(&wrapper_script(line, "/d"));
        assert!(
            launch.contains(&format!("nohup sh -c {wrapper} ")),
            "{launch}"
        );
        assert_eq!(
            wrapper,
            r"'echo $$ >/d/pid; '\''printf'\'' '\''it'\''\'\'''\''s'\'' --user $(id -u):$(id -g) >>/d/out.log 2>&1 </dev/null; echo $? >/d/status.tmp && mv /d/status.tmp /d/status'"
        );
        // The remote line wraps the launch script once more for the login shell.
        assert_eq!(remote_sh(&launch), format!("sh -c {}", sh_quote(&launch)));
    }

    #[test]
    fn poll_line_reads_the_state_before_the_log() {
        assert_eq!(
            poll_line("/d", 5),
            "cd /d 2>/dev/null || { echo 'SHORELINE-STATUS missing'; exit 0; }; \
             if [ -e status ]; then s=$(cat status); \
             elif [ -s pid ] && kill -0 \"$(cat pid)\" 2>/dev/null; then s=running; \
             elif [ -e status ]; then s=$(cat status); \
             elif [ -s pid ]; then s=dead; \
             elif [ -e out.log ]; then s=running; \
             else s=missing; fi; \
             echo \"SHORELINE-STATUS $s\"; tail -c +6 out.log 2>/dev/null; true"
        );
        assert!(poll_line("/d", 0).contains("tail -c +1 "));
    }

    #[test]
    fn cleanup_removes_the_run_dir() {
        assert_eq!(
            cleanup_line("/r/.shoreline-runs/x"),
            "rm -rf /r/.shoreline-runs/x"
        );
        assert_eq!(remote_sh("rm -rf /d"), "sh -c 'rm -rf /d'");
    }

    #[test]
    fn poll_output_parses() {
        let cases: [(&[u8], PollState, &[u8]); 7] = [
            (
                b"SHORELINE-STATUS running\nhello",
                PollState::Running,
                b"hello",
            ),
            (b"SHORELINE-STATUS running", PollState::Running, b""),
            (b"SHORELINE-STATUS 7\nlog\n", PollState::Exited(7), b"log\n"),
            (b"SHORELINE-STATUS -1\n", PollState::Exited(-1), b""),
            (b"SHORELINE-STATUS dead\n", PollState::Dead, b""),
            (b"SHORELINE-STATUS missing\n", PollState::Missing, b""),
            (
                b"SHORELINE-STATUS 0\n\xff\x00\r\n",
                PollState::Exited(0),
                b"\xff\x00\r\n",
            ),
        ];
        for (out, state, rest) in cases {
            assert_eq!(parse_poll(out), Some((state, rest)), "{out:?}");
        }
        for bad in [
            &b""[..],
            b"garbage\n",
            b"SHORELINE-STATUS \n",
            b"SHORELINE-STATUS 1x\n",
            b"x\nSHORELINE-STATUS 0\n",
        ] {
            assert_eq!(parse_poll(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn decisions_after_a_poll() {
        for launched in [false, true] {
            assert_eq!(next(launched, PollState::Running), Next::Wait);
            assert_eq!(next(launched, PollState::Exited(3)), Next::Finished(3));
            assert!(
                matches!(next(launched, PollState::Dead), Next::Fail(m) if m.contains("exit code"))
            );
        }
        // Relaunch only when nothing was started, so a launch that ran is not repeated.
        assert_eq!(next(false, PollState::Missing), Next::Relaunch);
        assert!(matches!(next(true, PollState::Missing), Next::Fail(m) if m.contains("gone")));
    }

    #[test]
    fn backoff_doubles_up_to_the_cap() {
        let p = RetryPolicy {
            attempts: 3,
            initial: Duration::from_millis(10),
            max: Duration::from_millis(100),
        };
        let ms: Vec<_> = (0..6).map(|n| p.delay(n).as_millis()).collect();
        assert_eq!(ms, [10, 20, 40, 80, 100, 100]);
        assert_eq!(p.delay(200), p.max);
        assert_eq!(p.delay(u32::MAX), p.max);
    }

    #[test]
    fn policy_from_remote_uses_seconds() {
        let remote: Remote = toml::from_str("host = \"h\"\nroot = \"/r\"").unwrap();
        let p = RetryPolicy::from_remote(&remote);
        assert_eq!(
            (p.attempts, p.initial, p.max),
            (10, Duration::from_secs(5), Duration::from_secs(60))
        );
    }

    #[test]
    fn gives_up_after_more_failures_than_attempts() {
        assert!(!exhausted(0, 3));
        assert!(!exhausted(3, 3));
        assert!(exhausted(4, 3));
    }

    /// Needs `sh`; run with the integration profile.
    mod integration {
        use super::*;
        use std::os::unix::fs::PermissionsExt;
        use std::time::Instant;

        /// A fake `ssh` that runs the remote line locally under zsh (the GPU
        /// host's login shell) or sh. Calls are numbered from 1 (the launch) in
        /// `count` beside it; a number listed in `drop` exits 255 without
        /// running the line, one in `drop_after` runs it and then exits 255.
        fn shim(dir: &Path, drop: &[u32], drop_after: &[u32]) -> String {
            let list = |n: &[u32]| n.iter().map(|n| format!("{n}\n")).collect::<String>();
            std::fs::write(dir.join("drop"), list(drop)).unwrap();
            std::fs::write(dir.join("drop_after"), list(drop_after)).unwrap();
            let path = dir.join("ssh");
            std::fs::write(
                &path,
                r#"#!/bin/sh
d=$(dirname "$0")
n=$(($(cat "$d/count" 2>/dev/null || echo 0) + 1))
echo "$n" >"$d/count"
while [ "$#" -gt 0 ] && [ "$1" != "--" ]; do shift; done
shift
sh=sh
command -v zsh >/dev/null 2>&1 && sh=zsh
grep -qx "$n" "$d/drop" && exit 255
if grep -qx "$n" "$d/drop_after"; then "$sh" -c "$1"; exit 255; fi
exec "$sh" -c "$1"
"#,
            )
            .unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o111, 0o111, "shim must be executable");
            path.to_str().unwrap().to_owned()
        }

        struct Setup {
            _tmp: tempfile::TempDir,
            base: PathBuf,
            ssh: String,
            log: PathBuf,
            dir: PathBuf,
        }

        fn setup(drop: &[u32], drop_after: &[u32]) -> Setup {
            let tmp = tempfile::tempdir().unwrap();
            let base = tmp.path().canonicalize().unwrap();
            let ssh = shim(&base, drop, drop_after);
            let (log, dir) = (
                base.join("step.log"),
                run_dir(&base.join("remote"), "t-1-2-0"),
            );
            Setup {
                _tmp: tmp,
                base,
                ssh,
                log,
                dir,
            }
        }

        fn engine(ssh: &str, attempts: u32) -> Engine<'_> {
            Engine {
                ssh_program: ssh,
                host: "gpu-host",
                policy: RetryPolicy {
                    attempts,
                    initial: Duration::from_millis(1),
                    max: Duration::from_millis(1),
                },
                poll_every: Duration::from_millis(100),
            }
        }

        /// `words` single-quoted, as `Tool::command` builds a remote line.
        fn line(words: &[&str]) -> String {
            words
                .iter()
                .map(|w| sh_quote(w))
                .collect::<Vec<_>>()
                .join(" ")
        }

        fn read(path: &Path) -> String {
            std::fs::read_to_string(path).unwrap()
        }

        /// Lines of `log` for which `f` holds.
        fn count(log: &Path, f: impl Fn(&str) -> bool) -> usize {
            read(log).lines().filter(|l| f(l)).count()
        }

        /// Launch calls logged, one `$ ` line each.
        fn launches(log: &Path) -> usize {
            count(log, |l| l.starts_with("$ "))
        }

        #[test]
        fn survives_dropped_polls_and_log_has_each_line_once() {
            // Calls 3 and 4 fail before running; call 5 runs, then loses its output.
            let s = setup(&[3, 4], &[5]);
            let script = "echo one; sleep 1; echo two; sleep 1; echo three";
            engine(&s.ssh, 3)
                .run("t", &s.log, &line(&["sh", "-c", script]), &s.dir)
                .unwrap();
            let log = read(&s.log);
            for word in ["one", "two", "three"] {
                assert_eq!(count(&s.log, |l| l == word), 1, "{word} in {log}");
            }
            assert_eq!(log.matches("polling gpu-host failed").count(), 3, "{log}");
            assert_eq!(launches(&s.log), 1, "{log}");
            assert!(!s.dir.exists(), "run dir left behind");
        }

        #[test]
        fn gives_up_with_remote_lost_after_reconnect_attempts() {
            let s = setup(&(2..=100).collect::<Vec<_>>(), &[]);
            let start = Instant::now();
            let err = engine(&s.ssh, 3)
                .run("t", &s.log, &line(&["true"]), &s.dir)
                .unwrap_err();
            assert!(start.elapsed() < Duration::from_secs(10));
            assert!(
                matches!(err, Error::RemoteLost { attempts: 3, .. }),
                "{err}"
            );
            let msg = err.to_string();
            assert!(
                msg.contains("gpu-host") && msg.contains(&*s.dir.to_string_lossy()),
                "{msg}"
            );
            assert_eq!(
                read(&s.base.join("count")).trim(),
                "5",
                "launch and four polls"
            );
            assert!(s.dir.exists(), "run dir is left for inspection");
        }

        #[test]
        fn propagates_nonzero_exit() {
            let s = setup(&[], &[]);
            let cmd = line(&["sh", "-c", "echo bad; exit 7"]);
            let err = engine(&s.ssh, 3)
                .run("t", &s.log, &cmd, &s.dir)
                .unwrap_err();
            assert!(
                matches!(err, Error::RemoteToolFailed { code: 7, .. }),
                "{err}"
            );
            assert!(err.to_string().contains("exit 7"), "{err}");
            assert!(read(&s.log).contains("bad\n"));
            assert!(!s.dir.exists());
        }

        #[test]
        fn relaunch_decision_after_failed_launch_does_not_double_launch() {
            // The launch runs, then ssh reports the connection lost.
            let s = setup(&[], &[1]);
            let marker = s.base.join("marker");
            let script = format!("echo x >>{}; sleep 0.3", marker.display());
            engine(&s.ssh, 3)
                .run("t", &s.log, &line(&["sh", "-c", &script]), &s.dir)
                .unwrap();
            assert_eq!(read(&marker), "x\n");
            assert_eq!(launches(&s.log), 1, "one launch logged");
        }

        #[test]
        fn relaunch_after_a_launch_that_never_ran() {
            let s = setup(&[1], &[]);
            let marker = s.base.join("marker");
            let script = format!("echo x >>{}", marker.display());
            engine(&s.ssh, 3)
                .run("t", &s.log, &line(&["sh", "-c", &script]), &s.dir)
                .unwrap();
            assert_eq!(read(&marker), "x\n");
            assert_eq!(launches(&s.log), 2, "launched twice");
        }

        #[test]
        fn detach_keeps_quotes_and_shell_words() {
            let s = setup(&[], &[]);
            // Docker's remote `--user` is left unquoted for the remote shell.
            let cmd = format!("{} $(id -u)", line(&["printf", "%s|%s\\n", "it's"]));
            engine(&s.ssh, 3).run("t", &s.log, &cmd, &s.dir).unwrap();
            let uid = rustix::process::geteuid().as_raw();
            assert!(
                read(&s.log).contains(&format!("it's|{uid}\n")),
                "{}",
                read(&s.log)
            );
        }

        #[test]
        fn detach_reports_a_killed_wrapper_as_dead() {
            let s = setup(&[], &[]);
            let cmd = line(&["sh", "-c", "echo started; kill -9 $PPID"]);
            let err = engine(&s.ssh, 3)
                .run("t", &s.log, &cmd, &s.dir)
                .unwrap_err();
            assert!(
                matches!(&err, Error::RemoteRun { message, .. } if message.contains("without an exit code")),
                "{err}"
            );
            assert!(read(&s.log).contains("started"));
        }
    }
}
