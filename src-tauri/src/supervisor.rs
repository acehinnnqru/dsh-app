//! Child supervision for one profile.
//!
//! The shell's whole coupling to dsh is: spawn it, read the readiness line from
//! stdout, and shut it down with SIGTERM before escalating. Everything else the
//! runtime does is opaque to us, which is what keeps the shell version-agnostic.
//!
//! The executable is the user's own `dsh`. The shell passes **no** Node flags:
//! those belong to whatever that executable wraps, and a shell that starts
//! second-guessing them is back to being a runtime installer (spec §5, D8). It
//! does name the home, so that what the window lists and what the child writes
//! cannot be two different directories.
//!
//! @see docs/design-tauri-wrapper.md D5, D6
//! @see docs/spec-external-dsh.md §5

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Readiness timeout: how long the launcher may take to print its URL line.
const READY_TIMEOUT: Duration = Duration::from_secs(60);
/// SIGTERM grace, matching the harness `PROCESS_SHUTDOWN_TIMEOUT_MS`.
const TERM_GRACE: Duration = Duration::from_secs(5);
/// How many stderr lines to retain for diagnostics.
const STDERR_TAIL: usize = 40;

/// Observable state of one supervised profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Phase {
    Starting,
    Ready { url: String },
    Exited { code: Option<i32>, signal: Option<i32> },
    Failed { message: String },
}

impl Phase {
    /// Stable string for the UI.
    pub fn label(&self) -> &'static str {
        match self {
            Phase::Starting => "starting",
            Phase::Ready { .. } => "ready",
            Phase::Exited { .. } => "exited",
            Phase::Failed { .. } => "failed",
        }
    }
}

/// Everything the UI needs to render one profile row.
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub phase: Phase,
    pub stderr_tail: Vec<String>,
}

/// A live child plus its reported state.
pub struct SupervisedChild {
    child: Child,
    phase: Phase,
    stderr_tail: Arc<Mutex<Vec<String>>>,
}

/// Extract the authenticated URL from one stdout line.
///
/// Deliberately tolerant: upstream appends a ` (LAN: …)` suffix, and the line
/// is a log statement rather than a committed API.
pub fn parse_ready_line(line: &str) -> Option<String> {
    let rest = line.split_once("dsh web:")?.1.trim_start();
    let url = rest.split_whitespace().next()?;
    if url.starts_with("http://") || url.starts_with("https://") {
        Some(url.to_owned())
    } else {
        None
    }
}

/// Build the child environment: never inherit `NODE_OPTIONS` or package-manager
/// state (the harness Electron shell strips the same names), and name the home
/// explicitly.
///
/// `DSH_HOME` **is** set, to the home this shell resolved, and any inherited
/// `DSH_HOME` is dropped first. Both halves are about agreement: the harness
/// gives `DSH_HOME` precedence over `~/.dsh`, so a value exported in the
/// terminal the app was launched from would otherwise point the child at a
/// different home than the profile list the window is showing; and the shell may
/// itself have taken its home *from* `DSH_HOME`, in which case the child has to
/// be told that answer rather than recomputing one.
fn child_environment(home: &Path) -> BTreeMap<String, String> {
    let mut env = BTreeMap::new();
    for (name, value) in std::env::vars() {
        if name == "NODE_OPTIONS" || name.starts_with("DSH_") {
            continue;
        }
        let lower = name.to_ascii_lowercase();
        if lower.starts_with("npm_") || lower.starts_with("pnpm_") || lower.starts_with("corepack_") {
            continue;
        }
        env.insert(name, value);
    }
    // After the filter, so an inherited `DSH_HOME` can never win over ours.
    env.insert("DSH_HOME".to_owned(), home.display().to_string());
    env
}

/// What one launch needs to know about the machine.
pub struct Launch {
    /// The user's `dsh` executable, exactly as they named it.
    pub dsh: PathBuf,
    /// Working directory. The harness does not write to its installation, and it
    /// boots from any directory, so this exists only to give the child a stable
    /// one rather than whatever the app inherited.
    pub cwd: PathBuf,
    /// The harness home the child is told to use (`DSH_HOME`).
    pub home: PathBuf,
}

impl SupervisedChild {
    /// Spawn the launcher and wait for its readiness line.
    ///
    /// The argv shape is the whole launcher contract: launcher flags
    /// (`--profile`) precede the first token the launcher does not know, and
    /// everything after that belongs to the booted app (`--no-open`, `--port`).
    /// `--no-open` must be given, or dsh also opens the user's default browser.
    pub fn start(profile: &str, launch: Launch) -> Result<Self, String> {
        let Launch { dsh, cwd, home } = launch;
        let args = [
            "--profile".to_owned(),
            profile.to_owned(),
            "--no-open".to_owned(),
            "--port".to_owned(),
            "0".to_owned(),
        ];

        let mut child = Command::new(&dsh)
            .args(&args)
            .current_dir(&cwd)
            .env_clear()
            .envs(child_environment(&home))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| format!("cannot spawn {}: {error}", dsh.display()))?;

        let stdout = child.stdout.take().ok_or("child stdout was not captured")?;
        let stderr = child.stderr.take().ok_or("child stderr was not captured")?;

        // stderr: keep a bounded tail for diagnostics.
        let tail = Arc::new(Mutex::new(Vec::new()));
        {
            let tail = Arc::clone(&tail);
            let tag = profile.to_owned();
            std::thread::spawn(move || {
                let reader = BufReader::new(stderr);
                for line in reader.lines().map_while(Result::ok) {
                    eprintln!("[dsh:{tag}] {line}");
                    if let Ok(mut guard) = tail.lock() {
                        guard.push(line);
                        if guard.len() > STDERR_TAIL {
                            guard.remove(0);
                        }
                    }
                }
            });
        }

        // stdout: forward and watch for the readiness line.
        let (sender, receiver) = mpsc::channel::<String>();
        let tag = profile.to_owned();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            let mut line = String::new();
            loop {
                line.clear();
                match reader.read_line(&mut line) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
                let text = line.trim_end().to_owned();
                eprintln!("[dsh:{tag}] {text}");
                if parse_ready_line(&text).is_some() {
                    let _ = sender.send(text);
                }
            }
        });

        let mut supervised = SupervisedChild {
            child,
            phase: Phase::Starting,
            stderr_tail: tail,
        };

        match receiver.recv_timeout(READY_TIMEOUT) {
            Ok(line) => {
                let url = parse_ready_line(&line).unwrap_or_default();
                supervised.phase = Phase::Ready { url };
            }
            Err(RecvTimeoutError::Timeout) => {
                supervised.phase = Phase::Failed {
                    message: format!("no readiness line within {}s", READY_TIMEOUT.as_secs()),
                };
            }
            Err(RecvTimeoutError::Disconnected) => {
                supervised.phase = Phase::Failed {
                    message: "the launcher exited before announcing its URL".to_owned(),
                };
            }
        }
        Ok(supervised)
    }

    /// Poll the child so a crash becomes visible without an explicit check.
    pub fn refresh(&mut self) {
        if matches!(self.phase, Phase::Failed { .. }) {
            return;
        }
        match self.child.try_wait() {
            Ok(Some(status)) => {
                use std::os::unix::process::ExitStatusExt;
                self.phase = Phase::Exited {
                    code: status.code(),
                    signal: status.signal(),
                };
            }
            Ok(None) => {}
            Err(error) => {
                self.phase = Phase::Failed {
                    message: format!("cannot poll the launcher: {error}"),
                };
            }
        }
    }

    /// Whether the child is still running.
    pub fn is_running(&mut self) -> bool {
        self.refresh();
        matches!(self.phase, Phase::Starting | Phase::Ready { .. })
    }

    /// Current UI snapshot.
    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            phase: self.phase.clone(),
            stderr_tail: self.stderr_tail.lock().map(|guard| guard.clone()).unwrap_or_default(),
        }
    }

    /// SIGTERM, wait for the grace period, then SIGKILL.
    pub fn shutdown(&mut self) {
        if !self.is_running() {
            return;
        }
        unsafe {
            libc_kill(self.child.id() as i32, 15);
        }
        let deadline = Instant::now() + TERM_GRACE;
        while Instant::now() < deadline {
            self.refresh();
            if !matches!(self.phase, Phase::Starting | Phase::Ready { .. }) {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        eprintln!("[dsh-app] launcher ignored SIGTERM for {TERM_GRACE:?}; sending SIGKILL");
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Send a signal without pulling in a `libc` dependency.
///
/// # Safety
/// The caller must pass a live process id owned by this process.
unsafe fn libc_kill(pid: i32, signal: i32) {
    extern "C" {
        fn kill(pid: i32, sig: i32) -> i32;
    }
    kill(pid, signal);
}

impl Drop for SupervisedChild {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_readiness_line_is_parsed_leniently() {
        assert_eq!(
            parse_ready_line("dsh web: http://127.0.0.1:51234/?token=abc"),
            Some("http://127.0.0.1:51234/?token=abc".to_owned())
        );
        // Upstream appends a LAN suffix; the first token is the loopback URL.
        assert_eq!(
            parse_ready_line("dsh web: http://127.0.0.1:51234/?token=abc (LAN: http://10.0.0.2:51234/)"),
            Some("http://127.0.0.1:51234/?token=abc".to_owned())
        );
        assert_eq!(parse_ready_line("dsh web: not-a-url"), None);
        assert_eq!(parse_ready_line("booting"), None);
    }

    /// The whole supervision path against a stub: the argv the launcher gets, the
    /// readiness line, and a SIGTERM that actually stops it.
    #[test]
    fn a_child_that_announces_a_url_becomes_ready_and_then_stops() {
        let dir = std::env::temp_dir().join(format!("dsh-app-supervisor-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("the scratch directory is creatable");
        let stub = dir.join("dsh");
        std::fs::write(
            &stub,
            "#!/bin/sh\necho 'dsh web: http://127.0.0.1:9/?token=stub'\nsleep 60\n",
        )
        .expect("the stub is writable");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        }

        let mut child = SupervisedChild::start(
            "web",
            Launch {
                dsh: stub,
                cwd: dir.clone(),
                home: dir.join("home"),
            },
        )
        .expect("the stub is spawnable");

        match &child.phase {
            Phase::Ready { url } => assert_eq!(url, "http://127.0.0.1:9/?token=stub"),
            other => panic!("expected ready, got {other:?}"),
        }
        assert!(child.is_running());
        child.shutdown();
        assert!(!child.is_running());

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The child must never inherit the variables the harness Electron shell also
    /// strips, and must be told exactly one home: the one the shell resolved.
    #[test]
    fn the_environment_is_filtered_and_names_our_home() {
        let env = child_environment(Path::new("/tmp/some-home"));
        assert_eq!(env.get("DSH_HOME").map(String::as_str), Some("/tmp/some-home"));
        assert!(!env.contains_key("NODE_OPTIONS"));
        assert_eq!(env.keys().filter(|name| name.starts_with("DSH_")).count(), 1);
        assert!(env.keys().all(|name| !name.to_ascii_lowercase().starts_with("npm_")));
    }
}
