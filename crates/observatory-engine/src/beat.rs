//! "Still running": what the engine says while one bridge command takes a while.
//!
//! The command loop owns the session and runs one command at a time, so while a long command runs (a geocode
//! of a hundred addresses a second apart, a dataset read over the network) nothing else is answered, and
//! the host hears nothing until the command is done. A host that gives up after a fixed wait cannot tell
//! that engine from one that died: it throws the call away at 30 seconds while the engine finishes it, and
//! the caller asks again. So a command that is still running after [`EVERY`] says so, from a thread of its own
//! because the loop is busy: one `progress` event with `phase: "running"` every [`EVERY`] until it is done.
//! Any line from the engine proves it is there; this one is the line a busy engine can always send.
//!
//! It is an optional emitter, like the keyboard's `focus-exit`: the schema's `progress` event already has a
//! free `phase`, a host that does not know the phase ignores the line, and the protocol version does not
//! change. A command that finishes inside [`EVERY`] sends nothing, so the common case adds no line.
//!
//! The state is one value under one lock, and a beat is written while the lock is held, so once
//! [`Beacon::done`] returns no beat of that command is written or in flight: the result is always the
//! last line about it.
//!
//! Followed from the app's own liveness rule for a call in flight (`peer-liveness.ts`, itself from Orca's SSH
//! multiplexer, `src/main/ssh/protocol/ssh-channel-multiplexer.ts`): a call has no timer of its own, and
//! silence from the peer is what ends the wait.

use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::Duration;

use serde_json::{Value, json};

/// How often a running command says so. The host treats a few beats missed as a dead engine, so this is
/// the unit of how soon it hears that one died.
pub const EVERY: Duration = Duration::from_secs(2);

/// What the beating thread is told about.
enum Run {
    /// No command is running: the thread sleeps until there is one.
    Idle,
    /// This command (the host's id and the command's name) is running.
    Running { id: Value, name: String },
    /// The command loop is gone.
    Closed,
}

struct State {
    run: Run,
    /// Rises with every command, so a wait that began for one is never a beat of the next.
    generation: u64,
}

struct Shared {
    state: Mutex<State>,
    wake: Condvar,
}

fn locked(shared: &Shared) -> MutexGuard<'_, State> {
    // A beat that panicked leaves the state as it was; the command loop must still be able to finish.
    shared.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The command loop's end of the beat: one thread for the process, asleep while nothing runs.
pub struct Beacon {
    shared: Arc<Shared>,
}

impl Beacon {
    /// Starts the thread. `say` writes one line to the host. Without a thread (the system refused one) the
    /// engine still works and the host falls back to its own fixed wait.
    pub fn start(every: Duration, say: impl Fn(&Value) + Send + 'static) -> Beacon {
        let shared = Arc::new(Shared { state: Mutex::new(State { run: Run::Idle, generation: 0 }), wake: Condvar::new() });
        let thread = Arc::clone(&shared);
        let _ = std::thread::Builder::new().name("bridge-beat".into()).spawn(move || beat(&thread, every, &say));
        Beacon { shared }
    }

    /// A command is about to run.
    pub fn running(&self, id: &Value, name: &str) {
        let mut state = locked(&self.shared);
        state.run = Run::Running { id: id.clone(), name: name.to_string() };
        state.generation += 1;
        self.shared.wake.notify_all();
    }

    /// The command that was running is done and its answer is about to be written. When this returns no beat
    /// of it is being written, and none will be.
    pub fn done(&self) {
        let mut state = locked(&self.shared);
        state.run = Run::Idle;
        state.generation += 1;
        self.shared.wake.notify_all();
    }
}

impl Drop for Beacon {
    fn drop(&mut self) {
        locked(&self.shared).run = Run::Closed;
        self.shared.wake.notify_all();
    }
}

/// The one line a beat writes: a `progress` event of the command under `id`, in the phase `running`.
pub fn line(id: &Value, name: &str) -> Value {
    json!({
        "event": "bridge.event",
        "protocolVersion": crate::bridge::PROTOCOL_VERSION,
        "payload": {"type": "progress", "id": id, "name": name, "phase": "running"},
    })
}

/// Sleeps while nothing runs; while a command runs, writes a beat each time `every` passes with the same
/// command still running. Ends when the loop's end is dropped.
fn beat(shared: &Shared, every: Duration, say: &impl Fn(&Value)) {
    let mut state = locked(shared);
    loop {
        if matches!(state.run, Run::Closed) {
            return;
        }
        if matches!(state.run, Run::Idle) {
            state = shared.wake.wait(state).unwrap_or_else(|poisoned| poisoned.into_inner());
            continue;
        }
        let waiting_for = state.generation;
        let (next, waited) = shared.wake.wait_timeout(state, every).unwrap_or_else(|poisoned| poisoned.into_inner());
        state = next;
        if waited.timed_out()
            && state.generation == waiting_for
            && let Run::Running { id, name } = &state.run
        {
            say(&line(id, name));
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use std::sync::mpsc::{Receiver, channel};

    const SHORT: Duration = Duration::from_millis(25);
    const WAIT: Duration = Duration::from_secs(10);

    fn beacon_every(every: Duration) -> (Beacon, Receiver<Value>) {
        let (said_tx, said) = channel();
        let beacon = Beacon::start(every, move |v| {
            let _ = said_tx.send(v.clone());
        });
        (beacon, said)
    }

    fn payload(v: &Value) -> (Value, String, String, String) {
        let p = &v["payload"];
        let text = |k: &str| p[k].as_str().unwrap_or("").to_string();
        (p["id"].clone(), text("type"), text("name"), text("phase"))
    }

    fn running(id: Value, name: &str) -> (Value, String, String, String) {
        (id, "progress".into(), name.into(), "running".into())
    }

    #[test]
    fn a_running_command_says_so_again_and_again_until_it_is_done() {
        let (beacon, said) = beacon_every(SHORT);
        beacon.running(&json!(7), "command");
        for _ in 0..3 {
            let v = said.recv_timeout(WAIT).unwrap();
            assert_eq!(v["event"], "bridge.event");
            assert_eq!(v["protocolVersion"], crate::bridge::PROTOCOL_VERSION);
            assert_eq!(payload(&v), running(json!(7), "command"));
        }
        beacon.done();
        // Nothing is written after `done` returns: what was written before it is already in the channel.
        while said.try_recv().is_ok() {}
        assert!(said.recv_timeout(SHORT * 8).is_err(), "no beat after the command is done");
    }

    #[test]
    fn a_command_that_ends_inside_one_beat_says_nothing() {
        let (beacon, said) = beacon_every(Duration::from_millis(400));
        beacon.running(&json!("host:1"), "dataset.query");
        beacon.done();
        assert!(said.recv_timeout(Duration::from_millis(900)).is_err(), "a quick command adds no line");
    }

    #[test]
    fn the_next_command_beats_under_its_own_id_and_never_before_a_full_beat() {
        let (beacon, said) = beacon_every(Duration::from_millis(150));
        beacon.running(&json!(1), "command");
        assert_eq!(payload(&said.recv_timeout(WAIT).unwrap()), running(json!(1), "command"));
        beacon.done();
        while said.try_recv().is_ok() {}
        let started = std::time::Instant::now();
        beacon.running(&json!(2), "dataset.open");
        let first = said.recv_timeout(WAIT).unwrap();
        // The new command's wait began with it: a beat of the old one's clock never comes early.
        assert!(started.elapsed() >= Duration::from_millis(140), "{:?}", started.elapsed());
        assert_eq!(payload(&first), running(json!(2), "dataset.open"));
    }

    #[test]
    fn a_beat_is_a_line_of_the_schemas_progress_event() {
        let schema: Value = serde_json::from_str(crate::bridge::SCHEMA_TEXT).unwrap();
        let progress = &schema["engine"]["events"]["progress"];
        let required: Vec<&str> = progress["required"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
        let beat = line(&json!(3), "command");
        for key in required {
            assert!(beat["payload"].get(key).is_some(), "the schema requires `{key}`: {beat}");
        }
        assert!(progress["properties"].get("phase").is_some(), "the schema's progress event has a phase");
    }

    #[test]
    fn dropping_the_beacon_ends_its_thread_and_its_beats() {
        let (beacon, said) = beacon_every(SHORT);
        beacon.running(&json!(1), "command");
        said.recv_timeout(WAIT).unwrap();
        drop(beacon);
        // The thread's closure holds the sender: the channel disconnects once the thread has returned, and a
        // beat already written is read first.
        loop {
            match said.recv_timeout(WAIT) {
                Ok(_) => continue,
                Err(e) => {
                    assert_eq!(e, std::sync::mpsc::RecvTimeoutError::Disconnected, "the thread ended");
                    break;
                }
            }
        }
    }
}
