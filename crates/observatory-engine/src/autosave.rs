//! The working copy: the project the engine holds, kept on disk so a quit does not lose it (Syngle-Ecosystem #671).
//!
//! The engine holds the open project in memory and nothing wrote it down, so closing the app lost every map, layer,
//! style, dataset and view the person had made. With `--autosave <file>` the engine
//!
//! * **restores** the file at start, before it says `ready`, so the page's first project document is already the work the
//!   person left. A file that cannot be read is moved aside to `<file>.unreadable` and the engine starts as it always did;
//!   the work is never overwritten;
//! * **writes** the project after it changes, once the changes go quiet ([`IDLE`]) and at the latest [`MOST`] after the
//!   first one, so an agent that runs for minutes still leaves a copy behind;
//! * **writes** it once more when it is told to quit or its pipe closes.
//!
//! It is the Deck's own recovery rule (`DeckWorkspaceService`: a snapshot on a quiet moment, kept in the app's own folder,
//! offered at the next start), applied to the one project an engine holds. It is not a save: it does not name the project
//! and does not clear the unsaved mark. Beside the project it keeps two facts the project file does not hold, in
//! `<file>.state`: the file the person saved the project to, and whether it has changes that file lacks, so after a
//! restart "Save" still writes to the same file and "Unsaved changes" is still true when it was.
//!
//! A write is a temporary file renamed over the old one, so a kill in the middle leaves the last whole copy. A copy is
//! written only when it differs from the last one, because the revision rises on every command, read-only ones included.
//! A project that takes long to write is written less often: the next copy waits [`BACK_OFF`] times as long as the last
//! one took, so keeping a copy never costs the person more than a tenth of the engine's time.
//!
//! The time and the files are the loop's; this module holds the decisions, so the schedule can be tested without a clock.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use rustgis_engine::Project;
use serde_json::{Value, json};

/// Quiet time after the last change before a copy is written.
pub const IDLE: Duration = Duration::from_millis(1500);

/// The longest a change waits for a copy while changes keep coming.
pub const MOST: Duration = Duration::from_secs(15);

/// A copy that took `t` to write holds the next one back by `t * BACK_OFF`.
pub const BACK_OFF: u32 = 9;

/// What the working copy held at start.
pub enum Restored {
    /// No working copy: a first start.
    Absent,
    /// The project the person left.
    Loaded(Kept),
    /// The file was there and was not a project this engine reads; it has been moved aside.
    Unreadable(String),
}

/// A working copy that was read: the project's text (checked to be one this engine reads, opened through the engine's own
/// `project.openText` so a restored project is set up exactly as an opened one is) and the facts kept beside it.
pub struct Kept {
    pub text: String,
    /// The file the person saved the project to, when they did.
    pub path: Option<PathBuf>,
    /// The project has changes that file does not hold (or was never saved).
    pub dirty: bool,
}

pub struct Autosave {
    path: PathBuf,
    /// The revision last looked at: a different one means the project may have changed.
    seen: u64,
    /// When the project first changed after the last copy, and when it last changed.
    first: Option<Instant>,
    last: Option<Instant>,
    /// What the files hold now, so an identical project is not written again.
    written: Option<u64>,
    /// When the last copy was written and how long it took: the next one waits for ten times that.
    wrote: Option<(Instant, Duration)>,
}

impl Autosave {
    pub fn new(path: PathBuf) -> Self {
        Self { path, seen: 0, first: None, last: None, written: None, wrote: None }
    }

    fn state_path(&self) -> PathBuf {
        let mut state = self.path.clone().into_os_string();
        state.push(".state");
        PathBuf::from(state)
    }

    /// Reads the working copy, if there is one.
    pub fn restore(&mut self) -> Restored {
        let text = match std::fs::read_to_string(&self.path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Restored::Absent,
            Err(e) => return self.set_aside(format!("{}: {e}", self.path.display())),
        };
        if let Err(why) = Project::from_json(&text) {
            return self.set_aside(why);
        }
        // A copy with no state beside it (or one that cannot be read) is a project nobody saved to a file: unsaved.
        let state = std::fs::read_to_string(self.state_path()).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok());
        let path = state.as_ref().and_then(|s| s["path"].as_str()).filter(|p| !p.is_empty()).map(PathBuf::from);
        let dirty = state.as_ref().and_then(|s| s["dirty"].as_bool()).unwrap_or(true);
        self.written = Some(digest(&text, &state_text(path.as_deref(), dirty)));
        Restored::Loaded(Kept { text, path, dirty })
    }

    /// Moves a working copy that cannot be read out of the way, so the next copy does not replace it.
    fn set_aside(&mut self, why: String) -> Restored {
        let mut aside = self.path.clone().into_os_string();
        aside.push(".unreadable");
        let _ = std::fs::rename(&self.path, PathBuf::from(aside));
        let _ = std::fs::remove_file(self.state_path());
        self.written = None;
        Restored::Unreadable(why)
    }

    /// The engine read the copy and the session still refused it: it is set aside like one that cannot be read.
    pub fn reject(&mut self, why: String) {
        let _ = self.set_aside(why);
    }

    /// The revision the loop starts at: what was restored or opened is not a change to write down.
    pub fn baseline(&mut self, revision: u64) {
        self.seen = revision;
    }

    /// The loop saw the project at `revision` at `now`.
    pub fn observe(&mut self, revision: u64, now: Instant) {
        if revision == self.seen {
            return;
        }
        self.seen = revision;
        self.first.get_or_insert(now);
        self.last = Some(now);
    }

    /// How long until a copy is due: `None` when nothing is waiting, zero when it is due now.
    pub fn wait(&self, now: Instant) -> Option<Duration> {
        let (first, last) = (self.first?, self.last?);
        let mut due = (last + IDLE).min(first + MOST);
        if let Some((at, took)) = self.wrote {
            due = due.max(at + took * BACK_OFF);
        }
        Some(due.saturating_duration_since(now))
    }

    /// A change is waiting for its copy.
    pub fn pending(&self) -> bool {
        self.first.is_some()
    }

    /// Writes the copy now (the wait is over, or the engine is going). A project identical to the files' is not written. A
    /// failed write is tried again after another quiet moment, and says why.
    pub fn write(&mut self, project: &Project, path: Option<&Path>, dirty: bool, now: Instant) -> Result<(), String> {
        let text = serde_json::to_string(project).map_err(|e| e.to_string())?;
        let state = state_text(path, dirty);
        let digest = digest(&text, &state);
        if self.written == Some(digest) {
            self.first = None;
            self.last = None;
            return Ok(());
        }
        // The project first and its state second: a kill between them leaves the new project with the old state, which
        // reads as at worst "saved to that file" for a project that has changes it lacks, never the reverse.
        let began = Instant::now();
        let done = write_whole(&self.path, text.as_bytes()).and_then(|()| write_whole(&self.state_path(), state.as_bytes()));
        match done {
            Ok(()) => {
                self.written = Some(digest);
                self.first = None;
                self.last = None;
                self.wrote = Some((Instant::now(), began.elapsed()));
                Ok(())
            }
            Err(e) => {
                self.first = Some(now);
                self.last = Some(now);
                Err(e)
            }
        }
    }
}

fn state_text(path: Option<&Path>, dirty: bool) -> String {
    json!({"path": path.map(|p| p.display().to_string()), "dirty": dirty}).to_string()
}

fn digest(text: &str, state: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    text.hash(&mut hasher);
    state.hash(&mut hasher);
    hasher.finish()
}

/// The file whole or not at all: the bytes go to a neighbour and take the file's place in one rename.
fn write_whole(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let fail = |e: std::io::Error| format!("could not write {}: {e}", path.display());
    if let Some(folder) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(folder).map_err(fail)?;
    }
    let mut part = path.to_path_buf().into_os_string();
    part.push(".part");
    let part = PathBuf::from(part);
    std::fs::write(&part, bytes).map_err(fail)?;
    std::fs::rename(&part, path).map_err(|e| {
        let _ = std::fs::remove_file(&part);
        fail(e)
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    fn folder(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("syngle-obs-autosave-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn named(name: &str) -> Project {
        Project::new(name)
    }

    fn copy_of(restored: Restored) -> Kept {
        match restored {
            Restored::Loaded(copy) => copy,
            Restored::Absent => panic!("there was no copy"),
            Restored::Unreadable(why) => panic!("the copy was unreadable: {why}"),
        }
    }

    #[test]
    fn nothing_is_waiting_until_the_project_changes() {
        let mut autosave = Autosave::new(PathBuf::from("x.rgis"));
        let t = Instant::now();
        autosave.baseline(5);
        autosave.observe(5, t);
        assert_eq!(autosave.wait(t), None);
        assert!(!autosave.pending());
    }

    #[test]
    fn a_change_is_written_once_the_changes_go_quiet() {
        let mut autosave = Autosave::new(PathBuf::from("x.rgis"));
        let t = Instant::now();
        autosave.baseline(1);
        autosave.observe(2, t);
        assert_eq!(autosave.wait(t), Some(IDLE));
        // Another change a second later pushes the copy back by a quiet moment.
        let later = t + Duration::from_secs(1);
        autosave.observe(3, later);
        assert_eq!(autosave.wait(later), Some(IDLE));
        assert_eq!(autosave.wait(later + IDLE), Some(Duration::ZERO));
    }

    #[test]
    fn changes_that_never_go_quiet_still_get_a_copy() {
        let mut autosave = Autosave::new(PathBuf::from("x.rgis"));
        let t = Instant::now();
        autosave.baseline(1);
        autosave.observe(2, t);
        // A change every second for twenty seconds: the quiet moment never comes, the longest wait does.
        let mut revision = 2;
        for second in 1..20 {
            revision += 1;
            autosave.observe(revision, t + Duration::from_secs(second));
        }
        assert_eq!(autosave.wait(t + MOST), Some(Duration::ZERO));
        assert_eq!(autosave.wait(t + MOST - Duration::from_secs(1)), Some(Duration::from_secs(1)));
    }

    #[test]
    fn a_missing_file_is_a_first_start() {
        let dir = folder("absent");
        let mut autosave = Autosave::new(dir.join("project.rgis"));
        assert!(matches!(autosave.restore(), Restored::Absent));
    }

    #[test]
    fn what_was_written_comes_back_with_its_file_and_unsaved_mark_and_is_not_written_again() {
        let dir = folder("roundtrip");
        let path = dir.join("maps").join("project.rgis");
        let saved_to = dir.join("Sydney.rgis");
        let mut first = Autosave::new(path.clone());
        let t = Instant::now();
        first.baseline(1);
        first.observe(2, t);
        first.write(&named("Sydney"), Some(&saved_to), true, t).unwrap();
        assert!(!first.pending());
        assert!(path.exists());
        assert!(!dir.join("maps").join("project.rgis.part").exists());

        let mut second = Autosave::new(path.clone());
        let copy = copy_of(second.restore());
        assert_eq!(Project::from_json(&copy.text).unwrap().name, "Sydney");
        assert_eq!(copy.path.as_deref(), Some(saved_to.as_path()));
        assert!(copy.dirty);
        // The same project in the same state changes nothing on disk and leaves nothing waiting.
        second.observe(9, t);
        assert!(second.pending());
        let before = std::fs::metadata(&path).unwrap().modified().unwrap();
        second.write(&named("Sydney"), Some(&saved_to), true, t).unwrap();
        assert!(!second.pending());
        assert_eq!(std::fs::metadata(&path).unwrap().modified().unwrap(), before);
    }

    #[test]
    fn saving_changes_only_the_state_so_the_copy_follows_the_save() {
        let dir = folder("saved");
        let path = dir.join("project.rgis");
        let mut autosave = Autosave::new(path.clone());
        let t = Instant::now();
        autosave.observe(2, t);
        autosave.write(&named("Sydney"), None, true, t).unwrap();
        // The person saves: the project is the same, the file and the mark are not.
        autosave.observe(3, t);
        autosave.write(&named("Sydney"), Some(Path::new("Sydney.rgis")), false, t).unwrap();
        let copy = copy_of(Autosave::new(path).restore());
        assert_eq!(copy.path.as_deref(), Some(Path::new("Sydney.rgis")));
        assert!(!copy.dirty);
    }

    #[test]
    fn a_copy_with_no_state_beside_it_is_unsaved() {
        let dir = folder("nostate");
        let path = dir.join("project.rgis");
        let mut autosave = Autosave::new(path.clone());
        autosave.write(&named("Sydney"), None, false, Instant::now()).unwrap();
        std::fs::remove_file(dir.join("project.rgis.state")).unwrap();
        let copy = copy_of(Autosave::new(path).restore());
        assert!(copy.path.is_none());
        assert!(copy.dirty);
    }

    #[test]
    fn a_copy_that_cannot_be_read_is_moved_aside_and_never_overwritten() {
        let dir = folder("broken");
        let path = dir.join("project.rgis");
        std::fs::write(&path, "{ this is not a project").unwrap();
        let mut autosave = Autosave::new(path.clone());
        assert!(matches!(autosave.restore(), Restored::Unreadable(_)));
        assert!(!path.exists());
        assert_eq!(std::fs::read_to_string(dir.join("project.rgis.unreadable")).unwrap(), "{ this is not a project");
        // The engine then starts fresh and its first copy lands in the file's place.
        autosave.write(&named("Fresh"), None, true, Instant::now()).unwrap();
        assert!(path.exists());
    }

    #[test]
    fn a_project_from_a_newer_engine_is_set_aside_too() {
        let dir = folder("newer");
        let path = dir.join("project.rgis");
        let mut project = serde_json::to_value(named("Later")).unwrap();
        project["format"] = json!(9999);
        std::fs::write(&path, project.to_string()).unwrap();
        let mut autosave = Autosave::new(path.clone());
        let Restored::Unreadable(why) = autosave.restore() else { panic!("a newer project was read") };
        assert!(why.contains("newer"), "{why}");
        assert!(dir.join("project.rgis.unreadable").exists());
    }

    #[test]
    fn a_failed_write_waits_for_another_quiet_moment() {
        let dir = folder("failing");
        // The folder where the file goes is a file, so nothing can be written there.
        let blocker = dir.join("maps");
        std::fs::write(&blocker, "in the way").unwrap();
        let mut autosave = Autosave::new(blocker.join("project.rgis"));
        let t = Instant::now();
        autosave.baseline(1);
        autosave.observe(2, t);
        assert!(autosave.write(&named("Lost"), None, true, t).is_err());
        assert!(autosave.pending());
        assert_eq!(autosave.wait(t), Some(IDLE));
    }

    #[test]
    fn a_slow_write_holds_the_next_one_back() {
        let mut autosave = Autosave::new(PathBuf::from("x.rgis"));
        let t = Instant::now();
        // A copy that took two seconds: the next one is not due for eighteen, however quiet the changes are.
        autosave.wrote = Some((t, Duration::from_secs(2)));
        autosave.observe(2, t);
        assert_eq!(autosave.wait(t), Some(Duration::from_secs(18)));
        assert_eq!(autosave.wait(t + Duration::from_secs(18)), Some(Duration::ZERO));
    }
}
