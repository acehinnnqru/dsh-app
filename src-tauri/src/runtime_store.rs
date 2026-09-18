//! The runtime store: which dsh runtime this installation serves.
//!
//! The shell is thin and changes rarely; dsh changes often. So the runtime lives
//! in the user's own directory and is replaced there, while the copy inside the
//! app bundle is only the first-run seed (design doc D10, U2/U3).
//!
//! ```text
//! <home>/dsh-app/runtimes/
//!   current -> 0.1.5-rc.1     the active version
//!   previous -> 0.1.4/        the one rollback returns to
//!   0.1.5-rc.1/               a closure: runtime.json + package.json + node_modules
//!   store.json                which seed was installed, and the active version
//! ```
//!
//! Everything here must be crash-safe: the process can die between any two
//! statements, so the switch is a single atomic rename and the truth is always
//! recoverable from the directories that exist.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::RuntimeManifest;

/// Directory holding the runtimes, under the shell's own home.
pub const RUNTIMES_DIR: &str = "dsh-app/runtimes";

/// Bookkeeping for the store; the directories themselves are the real state.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StoreState {
    /// The seed identity last installed (so an app upgrade can tell).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<String>,
    /// The active version directory name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active: Option<String>,
    /// The version rollback returns to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous: Option<String>,
    /// The channel document this installation checks for updates. Set by the
    /// packaging step (a release cask and a master cask differ only here).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel_url: Option<String>,
}

/// What the store currently holds.
#[derive(Debug, Clone, Serialize)]
pub struct RuntimeStatus {
    /// Directory name of the active version.
    pub active: Option<String>,
    /// dsh version of the active runtime, read from its manifest.
    pub active_dsh_version: Option<String>,
    /// Directory name available for rollback.
    pub previous: Option<String>,
    /// Every version directory present.
    pub installed: Vec<String>,
    /// The seed this installation came from.
    pub seed: Option<String>,
}

/// The store rooted at one home.
pub struct RuntimeStore {
    root: PathBuf,
}

impl RuntimeStore {
    /// Open (creating nothing) the store under `home`.
    pub fn new(home: &Path) -> Self {
        Self { root: home.join(RUNTIMES_DIR) }
    }

    /// The store directory.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The `current` link, whether or not it exists.
    pub fn current_link(&self) -> PathBuf {
        self.root.join("current")
    }

    /// The `previous` link, whether or not it exists.
    pub fn previous_link(&self) -> PathBuf {
        self.root.join("previous")
    }

    /// The active runtime directory, when the `current` link resolves.
    ///
    /// Read from the link rather than from `store.json` on purpose: a stale
    /// bookkeeping file must never be able to point the shell at nothing.
    pub fn active_dir(&self) -> Option<PathBuf> {
        let link = self.current_link();
        let target = std::fs::read_link(&link).ok()?;
        let resolved = if target.is_absolute() { target } else { self.root.join(target) };
        resolved.is_dir().then_some(resolved)
    }

    /// Read the bookkeeping, or a default when it is absent or unreadable.
    pub fn state(&self) -> StoreState {
        let path = self.root.join("store.json");
        std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    /// A description of the store for the UI and diagnostics.
    pub fn status(&self) -> RuntimeStatus {
        let state = self.state();
        let active = self
            .active_dir()
            .and_then(|dir| dir.file_name().map(|name| name.to_string_lossy().into_owned()));
        let active_dsh_version = self
            .active_dir()
            .and_then(|dir| RuntimeManifest::load(&dir).ok())
            .map(|manifest| manifest.dsh_version);
        RuntimeStatus {
            active,
            active_dsh_version,
            previous: state.previous,
            installed: self.installed_versions(),
            seed: state.seed,
        }
    }

    /// Version directory names, sorted.
    pub fn installed_versions(&self) -> Vec<String> {
        let mut names: Vec<String> = match std::fs::read_dir(&self.root) {
            Ok(entries) => entries
                .filter_map(|entry| entry.ok())
                // `file_type` is the link-aware answer: `current` and `previous`
                // are symlinks and must not be listed as installed versions.
                .filter(|entry| entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false))
                .filter(|entry| !entry.file_name().to_string_lossy().starts_with('.'))
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .collect(),
            Err(_) => Vec::new(),
        };
        names.sort();
        names
    }

    /// Install a runtime from `source` as `id`, without activating it.
    ///
    /// Returns `false` when that id is already installed, so an app upgrade that
    /// carries the same seed does not re-copy hundreds of megabytes.
    pub fn install(&self, id: &str, source: &Path) -> Result<bool, String> {
        if !source.join("runtime.json").is_file() {
            return Err(format!("{} is not a runtime directory (no runtime.json)", source.display()));
        }
        let target = self.root.join(id);
        if target.is_dir() {
            return Ok(false);
        }
        std::fs::create_dir_all(&self.root)
            .map_err(|error| format!("cannot create {}: {error}", self.root.display()))?;

        // Copy into a staging name first, so an interrupted copy never leaves a
        // half-populated directory that looks installed.
        let staging = self.root.join(format!(".staging-{id}"));
        let _ = std::fs::remove_dir_all(&staging);
        copy_tree(source, &staging)?;
        // The manifest validates what we just copied before it can become active.
        RuntimeManifest::load(&staging)?;
        std::fs::rename(&staging, &target)
            .map_err(|error| format!("cannot publish {}: {error}", target.display()))?;
        Ok(true)
    }

    /// Point `current` at `id`, keeping the previous target for rollback.
    ///
    /// The switch is one `rename` over the link, which is atomic: a crash leaves
    /// either the old target or the new one, never neither.
    ///
    /// This is the raw switch and performs **no** validation beyond the manifest.
    /// Callers that are putting a *new* runtime into service must gate it on
    /// `crate::smoke::smoke` first — that check is the only proof a runtime can
    /// be launched by this shell (design doc D8.2 U1).
    pub fn activate(&self, id: &str) -> Result<(), String> {
        let target = self.root.join(id);
        if !target.is_dir() {
            return Err(format!("runtime {id} is not installed"));
        }
        RuntimeManifest::load(&target)?;

        let mut state = self.state();
        let outgoing = self
            .active_dir()
            .and_then(|dir| dir.file_name().map(|name| name.to_string_lossy().into_owned()));

        let staging_link = self.root.join(".current.next");
        let _ = std::fs::remove_file(&staging_link);
        std::os::unix::fs::symlink(id, &staging_link)
            .map_err(|error| format!("cannot create the current link: {error}"))?;
        std::fs::rename(&staging_link, self.current_link())
            .map_err(|error| format!("cannot switch the current link: {error}"))?;

        // The outgoing version becomes the rollback target; an explicit previous
        // is only overwritten once the new one is in place.
        if let Some(outgoing) = outgoing.filter(|name| name != id) {
            let staging_previous = self.root.join(".previous.next");
            let _ = std::fs::remove_file(&staging_previous);
            if std::os::unix::fs::symlink(&outgoing, &staging_previous).is_ok() {
                let _ = std::fs::rename(&staging_previous, self.previous_link());
            }
            state.previous = Some(outgoing);
        }
        state.active = Some(id.to_owned());
        self.write_state(&state)?;
        Ok(())
    }

    /// Activate whatever `previous` points at, and make the current one previous.
    pub fn rollback(&self) -> Result<String, String> {
        let link = self.previous_link();
        let target = std::fs::read_link(&link).map_err(|_| "there is no previous runtime to roll back to".to_owned())?;
        let resolved = if target.is_absolute() { target } else { self.root.join(target) };
        let id = resolved
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .ok_or_else(|| "the previous link does not name a version".to_owned())?;
        if !resolved.is_dir() {
            return Err(format!("the previous runtime {id} is no longer on disk"));
        }
        self.activate(&id)?;
        Ok(id)
    }

    /// Record which seed this installation came from.
    pub fn record_seed(&self, seed: &str) -> Result<(), String> {
        let mut state = self.state();
        state.seed = Some(seed.to_owned());
        self.write_state(&state)
    }

    /// Record the channel document to check for updates.
    pub fn set_channel_url(&self, url: &str) -> Result<(), String> {
        let mut state = self.state();
        state.channel_url = Some(url.to_owned());
        self.write_state(&state)
    }

    /// The channel document to check, if one has been configured.
    pub fn channel_url(&self) -> Option<String> {
        self.state().channel_url
    }

    /// Write the bookkeeping, creating the directory when needed.
    fn write_state(&self, state: &StoreState) -> Result<(), String> {
        std::fs::create_dir_all(&self.root)
            .map_err(|error| format!("cannot create {}: {error}", self.root.display()))?;
        let path = self.root.join("store.json");
        let text = format!(
            "{}\n",
            serde_json::to_string_pretty(state).map_err(|error| error.to_string())?
        );
        std::fs::write(&path, text).map_err(|error| format!("cannot write {}: {error}", path.display()))
    }
}

/// Recursively copy `from` into `to`.
///
/// A closure is tens of thousands of files, so this is the slow part of an
/// update and must leave nothing behind on failure — the caller stages into a
/// temporary name and only then publishes.
pub fn copy_tree(from: &Path, to: &Path) -> Result<(), String> {
    std::fs::create_dir_all(to).map_err(|error| format!("cannot create {}: {error}", to.display()))?;
    let entries = std::fs::read_dir(from).map_err(|error| format!("cannot read {}: {error}", from.display()))?;
    for entry in entries.filter_map(Result::ok) {
        let source = entry.path();
        let target = to.join(entry.file_name());
        let file_type = entry
            .file_type()
            .map_err(|error| format!("cannot stat {}: {error}", source.display()))?;
        if file_type.is_dir() {
            copy_tree(&source, &target)?;
            continue;
        }
        // Symlinks are copied as symlinks: the closure materializes them itself,
        // and dereferencing here would duplicate entire subtrees.
        if file_type.is_symlink() {
            let destination = std::fs::read_link(&source)
                .map_err(|error| format!("cannot read link {}: {error}", source.display()))?;
            std::os::unix::fs::symlink(&destination, &target)
                .map_err(|error| format!("cannot link {}: {error}", target.display()))?;
            continue;
        }
        std::fs::copy(&source, &target)
            .map_err(|error| format!("cannot copy {}: {error}", target.display()))?;
        let permissions = std::fs::metadata(&source)
            .map_err(|error| format!("cannot stat {}: {error}", source.display()))?
            .permissions();
        let _ = std::fs::set_permissions(&target, permissions);
    }
    Ok(())
}
