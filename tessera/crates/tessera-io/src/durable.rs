//! The durable file-system steps (#695): create a directory, write, fsync and rename a file, fsync a
//! directory, each a thin wrapper a test can record ([`seam::Event`]) and, for some, fail. Moved out
//! of the streaming write engine ([`crate::write`], its crash-consistency contract) so that other
//! writers can share them.
//!
//! A file is durable once its bytes are fsync'd and its directory entry is too: the entry by an
//! fsync of the directory holding it, after the file was created or renamed there. Every directory
//! on the path must itself be durable the same way, hence [`create_dirs_durable`].

use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

use tessera_core::Result;

/// Test-only seams (#695): fault counters, and a recorder of the file-system steps the durability
/// contract is about (a test can't drop the page cache, so it checks their order instead). Each is
/// thread-local and armed through a guard that disarms it on drop, so a test that panics before a
/// seam fires can't leave it armed for the next test on its thread.
#[cfg(test)]
pub(crate) mod seam {
    use std::cell::{Cell, RefCell};
    use std::collections::{BTreeMap, BTreeSet};
    use std::path::Path;
    use std::path::PathBuf;
    use std::thread::LocalKey;

    /// A fault counter: when armed with `n > 0`, the `n`-th next call through it fails.
    pub type Counter = LocalKey<Cell<usize>>;

    thread_local! {
        /// Fail a [`super::write_durable`] once its temp file is durable and before the rename, the
        /// point where a crash leaves both the old file and the new one on disk.
        pub static FAIL_NTH_DURABLE_RENAME: Cell<usize> = const { Cell::new(0) };
        /// Fail a [`super::sync_dir`] (the directory fsync).
        pub static FAIL_NTH_SYNC_DIR: Cell<usize> = const { Cell::new(0) };
        /// Fail the rename of a staged file into `blocks/` as crossing filesystems (`EXDEV`).
        pub static CROSS_DEVICE_NTH_STAGED_RENAME: Cell<usize> = const { Cell::new(0) };
        static LOG: RefCell<Option<Vec<Event>>> = const { RefCell::new(None) };
    }

    /// Keeps a fault counter armed; dropping it disarms the counter.
    #[must_use = "the seam is disarmed when the guard is dropped"]
    pub struct Armed(&'static Counter);

    /// Arm `counter` to fire on its `nth` next call.
    pub fn arm(counter: &'static Counter, nth: usize) -> Armed {
        counter.with(|n| n.set(nth));
        Armed(counter)
    }

    impl Drop for Armed {
        fn drop(&mut self) {
            self.0.with(|n| n.set(0));
        }
    }

    /// Count one call through `counter`: whether it is the call the counter is armed for.
    pub fn fires(counter: &'static Counter) -> bool {
        counter.with(|n| n.replace(n.get().saturating_sub(1)) == 1)
    }

    /// One completed file-system step, as the durability contract sees it.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum Event {
        CreateFile(PathBuf),
        CreateDir(PathBuf),
        Rename {
            from: PathBuf,
            to: PathBuf,
        },
        SyncFile(PathBuf),
        SyncDir(PathBuf),
        /// A journal line is about to be written, committing the fragment at this path.
        JournalLine(PathBuf),
    }

    /// Records this thread's events while it lives.
    #[must_use = "recording stops when the guard is dropped"]
    pub struct Recording;

    /// Start recording this thread's events.
    pub fn record_fs() -> Recording {
        LOG.with(|l| *l.borrow_mut() = Some(Vec::new()));
        Recording
    }

    impl Recording {
        /// The events recorded so far, in order.
        pub fn events(&self) -> Vec<Event> {
            LOG.with(|l| l.borrow().clone().unwrap_or_default())
        }
    }

    impl Drop for Recording {
        fn drop(&mut self) {
            LOG.with(|l| *l.borrow_mut() = None);
        }
    }

    /// Whether this thread is recording.
    pub fn recording() -> bool {
        LOG.with(|l| l.borrow().is_some())
    }

    /// Log `event`, if this thread is recording.
    pub fn record(event: Event) {
        LOG.with(|l| {
            if let Some(log) = l.borrow_mut().as_mut() {
                log.push(event);
            }
        });
    }

    /// What a power loss could still undo, replayed from a recorded event log (#695). An entry
    /// (file or directory) is durable once its directory is fsync'd after the entry was last
    /// created or renamed; an entry that no event created existed before the recording, and is
    /// durable once its directory is fsync'd at all (or, for the recording's `root`, already).
    /// A file's bytes are durable once it is fsync'd after it was created.
    #[derive(Default)]
    pub struct Replay {
        step: usize,
        touched: BTreeMap<PathBuf, usize>,
        dir_synced: BTreeMap<PathBuf, usize>,
        unsynced_bytes: BTreeSet<PathBuf>,
    }

    impl Replay {
        pub fn apply(&mut self, event: &Event) {
            use Event::*;
            self.step += 1;
            match event {
                CreateFile(p) => {
                    self.touched.insert(p.clone(), self.step);
                    self.unsynced_bytes.insert(p.clone());
                }
                CreateDir(p) => {
                    self.touched.insert(p.clone(), self.step);
                }
                Rename { from, to } => {
                    self.touched.insert(from.clone(), self.step);
                    self.touched.insert(to.clone(), self.step);
                    if self.unsynced_bytes.remove(from) {
                        self.unsynced_bytes.insert(to.clone());
                    }
                }
                SyncFile(p) => {
                    self.unsynced_bytes.remove(p);
                }
                SyncDir(d) => {
                    self.dir_synced.insert(d.clone(), self.step);
                }
                JournalLine(_) => {}
            }
        }

        /// What a power loss could still undo of `path` below the pre-existing `root`: its bytes,
        /// and its own entry and every ancestor's.
        pub fn undoable(&self, root: &Path, path: &Path) -> Vec<String> {
            let mut undoable = Vec::new();
            if self.unsynced_bytes.contains(path) {
                undoable.push(format!("the bytes of {}", path.display()));
            }
            for entry in path
                .ancestors()
                .take_while(|a| *a != root && a.starts_with(root))
            {
                let touched = self.touched.get(entry).copied().unwrap_or(0);
                let synced = entry.parent().and_then(|d| self.dir_synced.get(d));
                if synced.is_none_or(|s| *s <= touched) {
                    undoable.push(format!("the entry of {}", entry.display()));
                }
            }
            undoable
        }
    }
}

/// Deletes a file when dropped, unless [`Self::disarm`]ed, and then the directories created for it
/// (a hierarchical block name's subdirectories), deepest first, if they are empty. Best effort: a
/// drop can't report an error, and a file that is already gone is the goal.
#[derive(Debug)]
pub(crate) struct RemoveOnDrop {
    path: Option<PathBuf>,
    pub(crate) created_dirs: Vec<PathBuf>,
}

impl RemoveOnDrop {
    pub(crate) fn new(path: PathBuf) -> Self {
        RemoveOnDrop {
            path: Some(path),
            created_dirs: Vec::new(),
        }
    }

    pub(crate) fn path(&self) -> &Path {
        self.path.as_deref().expect("armed")
    }

    pub(crate) fn disarm(mut self) {
        self.path = None;
    }
}

impl Drop for RemoveOnDrop {
    fn drop(&mut self) {
        if let Some(path) = self.path.take() {
            let _ = fs::remove_file(path);
            for dir in &self.created_dirs {
                // `remove_dir` refuses a non-empty directory, so a sibling's fragment is never touched.
                let _ = fs::remove_dir(dir);
            }
        }
    }
}

/// Create the directory `dir` and every missing ancestor, each fsync'd into its parent so the path
/// survives a crash: the one durable-directory path (#695), used for the stage dir, `blocks/`,
/// `staging/`, a fragment's directories and a quarantine's. Every directory created is recorded
/// in `created` as soon as it exists, before its fsync, deepest first (the order to remove them
/// in), so a caller can undo a partial creation even when the fsync is what failed. A directory
/// that already exists is left as it is: making its own entry durable is the caller's business
/// ([`crate::write::WriteSession::create`] fsyncs the stage dir's parent).
pub(crate) fn create_dirs_durable(dir: &Path, created: &mut Vec<PathBuf>) -> Result<()> {
    let missing: Vec<&Path> = dir
        .ancestors()
        .take_while(|p| !p.as_os_str().is_empty() && !p.exists())
        .collect();
    for p in missing.into_iter().rev() {
        create_dir(p)?;
        created.insert(0, p.to_path_buf());
        sync_dir(parent_dir(p))?;
    }
    Ok(())
}

/// The directory holding `path`'s entry: its parent, or `.` for a bare relative name.
pub(crate) fn parent_dir(path: &Path) -> &Path {
    match path.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir,
        _ => Path::new("."),
    }
}

// The file-system steps the durability contract is made of (see the module docs), each a thin
// wrapper so that a test can record it (`seam::Event`) and, for the directory fsync, fail it.

/// Create (or truncate) the file `path` for writing.
pub(crate) fn create_file(path: &Path) -> std::io::Result<File> {
    let file = File::create(path)?;
    #[cfg(test)]
    seam::record(seam::Event::CreateFile(path.to_path_buf()));
    Ok(file)
}

/// fsync `file`, open at `path`, so its bytes survive a crash (not its directory entry: see
/// [`sync_dir`]).
#[cfg_attr(not(test), allow(unused_variables))]
pub(crate) fn sync_file(file: &File, path: &Path) -> std::io::Result<()> {
    file.sync_all()?;
    #[cfg(test)]
    seam::record(seam::Event::SyncFile(path.to_path_buf()));
    Ok(())
}

/// Create the directory `path` (its parent must exist).
pub(crate) fn create_dir(path: &Path) -> std::io::Result<()> {
    fs::create_dir(path)?;
    #[cfg(test)]
    seam::record(seam::Event::CreateDir(path.to_path_buf()));
    Ok(())
}

/// Rename `from` to `to` (atomic; durable only once both directories are fsync'd).
pub(crate) fn rename(from: &Path, to: &Path) -> std::io::Result<()> {
    fs::rename(from, to)?;
    #[cfg(test)]
    seam::record(seam::Event::Rename {
        from: from.to_path_buf(),
        to: to.to_path_buf(),
    });
    Ok(())
}

/// fsync the directory `dir`, so the entries created, renamed or removed in it survive a crash.
pub(crate) fn sync_dir(dir: &Path) -> std::io::Result<()> {
    #[cfg(test)]
    if seam::fires(&seam::FAIL_NTH_SYNC_DIR) {
        return Err(std::io::Error::other("injected directory fsync failure"));
    }
    File::open(dir)?.sync_all()?;
    #[cfg(test)]
    seam::record(seam::Event::SyncDir(dir.to_path_buf()));
    Ok(())
}

/// Write `bytes` to `path` **durably + atomically**: the one path every whole-file write of a session
/// takes (the header, the journal [`crate::write::WriteSession::recover`] rewrites, a quarantine's journal), and
/// each file of a stored pair ([`crate::stored::write_pair`]). It writes
/// a temp file beside `path` (`<file name>.tmp`, in the same directory, so the rename never crosses
/// filesystems), fsyncs it, renames it over `path`, then fsyncs the directory so the rename itself is
/// on disk. The rename is atomic, so a crash at any point leaves either the old file or the complete
/// new one, never a torn, truncated or missing one. A failure removes the temp file again and leaves
/// `path` as it was; a crash can leave the temp file, which the next write to `path` replaces.
pub(crate) fn write_durable(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut tmp_name = path
        .file_name()
        .expect("a durable write names a file")
        .to_os_string();
    tmp_name.push(".tmp");
    let tmp = RemoveOnDrop::new(path.with_file_name(tmp_name));
    {
        let mut f = create_file(tmp.path())?;
        f.write_all(bytes)?;
        sync_file(&f, tmp.path())?;
    }
    #[cfg(test)]
    if seam::fires(&seam::FAIL_NTH_DURABLE_RENAME) {
        return Err(std::io::Error::other("injected crash before the durable rename").into());
    }
    rename(tmp.path(), path)?;
    tmp.disarm();
    sync_dir(parent_dir(path))?;
    Ok(())
}
