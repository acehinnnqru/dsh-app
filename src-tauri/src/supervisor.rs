//! Child supervision for one preset.
//!
//! The shell's whole coupling to dsh is: spawn it, read the readiness line from
//! stdout, and shut it down with SIGTERM before escalating. Everything else the
//! runtime does is opaque to us, which is what keeps the shell version-agnostic.
//!
//! @see docs/design-tauri-wrapper.md D5, D6

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::config::ResolvedNode;
use crate::preset::Preset;

/// Readiness timeout: how long the launcher may take to print its URL line.
const READY_TIMEOUT: Duration = Duration::from_secs(60);
/// SIGTERM grace, matching the harness `PROCESS_SHUTDOWN_TIMEOUT_MS`.
const TERM_GRACE: Duration = Duration::from_secs(5);
/// How many stderr lines to retain for diagnostics.
const STDERR_TAIL: usize = 40;

/// Observable state of one supervised preset.
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

/// Everything the UI needs to render one preset row.
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub preset_id: String,
    pub label: String,
    pub profile: String,
    pub phase: Phase,
    pub stderr_tail: Vec<String>,
    /// Patch overlays this child was started with, in application order.
    pub patch_files: Vec<PathBuf>,
    /// The Node executable in use, and where it came from.
    pub node: PathBuf,
    pub node_source: &'static str,
}

/// A live child plus its reported state.
pub struct SupervisedChild {
    preset: Preset,
    /// The runtime this child was launched with, and where it came from.
    node: ResolvedNode,
    child: Child,
    phase: Phase,
    stderr_tail: Arc<Mutex<Vec<String>>>,
    patch_files: Vec<PathBuf>,
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
/// state (the harness Electron shell strips the same names).
fn child_environment(home: &PathBuf, preset: &Preset) -> BTreeMap<String, String> {
    let mut env = BTreeMap::new();
    // `dsh plugin` runs `pnpm` from PATH in the profile directory, so the bundle
    // has to make its own pnpm the first one found — on a machine without pnpm
    // the command otherwise fails with exit 127. It is unconditional because
    // deciding by existence would make the environment depend on what else
    // happens to be installed.
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let bundled_bin = dir.join("../Resources/bin");
            if bundled_bin.is_dir() {
                let existing = std::env::var("PATH").unwrap_or_default();
                env.insert(
                    "PATH".to_owned(),
                    format!("{}:{existing}", bundled_bin.display()),
                );
            }
        }
    }
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
    env.insert("DSH_HOME".to_owned(), home.display().to_string());
    for (name, value) in &preset.env {
        env.insert(name.clone(), value.clone());
    }
    env
}

impl SupervisedChild {
    /// Spawn the launcher and wait for its readiness line.
    ///
    /// `entry` is the closure's `lib/bin.js`; `node` the executable that runs it.
    /// `patch_files` become repeatable `--patch` overlays, which sit at the top
    /// of the patch stack in argv order, so preset layers win over bundle and
    /// profile layers without touching the profile's own patch file.
    pub fn start(
        preset: Preset,
        node: ResolvedNode,
        entry: PathBuf,
        cwd: PathBuf,
        home: PathBuf,
        patch_files: Vec<PathBuf>,
        node_args: Vec<String>,
    ) -> Result<Self, String> {
        // Node's own flags precede the script; passing them as argv entries
        // rather than through NODE_OPTIONS keeps the child environment clean
        // (that variable is stripped from inherited state for the same reason).
        let mut args: Vec<String> = node_args;
        args.push(entry.display().to_string());
        // Launcher flags must precede the first token the launcher does not
        // know, because everything from there on belongs to the booted app.
        args.push("--profile".to_owned());
        args.push(preset.profile_name().to_owned());
        for patch in &patch_files {
            args.push("--patch".to_owned());
            args.push(patch.display().to_string());
        }
        // App flags follow the launcher's.
        args.push("--no-open".to_owned());
        args.push("--port".to_owned());
        args.push("0".to_owned());

        let mut child = Command::new(&node.path)
            .args(&args)
            .current_dir(&cwd)
            .env_clear()
            .envs(child_environment(&home, &preset))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| format!("cannot spawn {}: {error}", node.path.display()))?;

        let stdout = child.stdout.take().ok_or("child stdout was not captured")?;
        let stderr = child.stderr.take().ok_or("child stderr was not captured")?;

        // stderr: keep a bounded tail for diagnostics.
        let tail = Arc::new(Mutex::new(Vec::new()));
        {
            let tail = Arc::clone(&tail);
            std::thread::spawn(move || {
                let reader = BufReader::new(stderr);
                for line in reader.lines().map_while(Result::ok) {
                    eprintln!("[dsh] {line}");
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
                eprintln!("[dsh] {text}");
                if parse_ready_line(&text).is_some() {
                    let _ = sender.send(text);
                }
            }
        });

        let mut supervised = SupervisedChild {
            preset,
            node,
            child,
            phase: Phase::Starting,
            stderr_tail: tail,
            patch_files,
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

    /// The preset this child runs.
    pub fn preset(&self) -> &Preset {
        &self.preset
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
            preset_id: self.preset.id.clone(),
            label: self
                .preset
                .label
                .clone()
                .unwrap_or_else(|| self.preset.id.clone()),
            profile: self.preset.profile_name().to_owned(),
            phase: self.phase.clone(),
            stderr_tail: self.stderr_tail.lock().map(|guard| guard.clone()).unwrap_or_default(),
            patch_files: self.patch_files.clone(),
            node: self.node.path.clone(),
            node_source: self.node.source.label(),
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
