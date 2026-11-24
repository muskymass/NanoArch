//! Collaboration: two people work on the same project and their copies are merged, with every disagreement resolved by a named person
//! (tracker item 94). Everything stays on the machines involved: the exchange is one TCP connection between the two computers (this
//! machine only, or the local network), and nothing is sent to any service.
//!
//! **Merge.** A three-way merge of the whole model. The base is the saved state both people started from (their hashes must be equal,
//! otherwise the exchange is refused by name). Per element and per property, recursively into nested objects (the project element
//! stores the brief, the BOQ and every input as nested objects):
//! - changed on one side only: that change wins;
//! - changed identically on both: kept once;
//! - added on one side: added; deleted on one side and untouched on the other: deleted;
//! - changed differently on both sides, or deleted on one side and changed on the other: a **conflict**. Arrays and numbers are one
//!   value, so two different edits of one array are a conflict, never a guessed blend.
//!
//! Conflicts have no default winner. The merge produces no model until every conflict has a choice (`choices`: conflict key → the name
//! of the replica whose value is kept). The two sides are ordered by replica name, so both computers compute the identical result from
//! the same choices. After merging, every element is re-validated (host present, typed schema); a merge that would leave an invalid
//! element yields `issues` and no model.
//!
//! **Transport.** `Host` listens for one peer, `connect` joins it; both swap snapshots. The session code (6 characters or more, chosen
//! by the users) stops a wrong computer from joining by mistake. It travels unencrypted, as does the project: use it on a trusted
//! network, or on this machine only. Nothing here encrypts, authenticates identity or merges text inside a single value.

use crate::command::Command;
use crate::model::{Element, Id, Model};
use crate::typed::validate_element;
use crate::{Document, Origin};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Longest wait for a peer, seconds.
pub const MAX_WAIT_S: u64 = 600;
/// Largest message accepted from a peer, bytes.
const MAX_FRAME: usize = 128 * 1024 * 1024;
/// Wrong codes tolerated before the host closes.
const MAX_BAD_ATTEMPTS: usize = 5;
const MIN_CODE_LEN: usize = 6;

/// One side of an exchange: who, what they both started from, and what they have now.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    pub replica: String,
    /// `Model::state_hash` of the common base, as text (a 64-bit number does not survive every JSON reader).
    pub base_hash: String,
    pub model: Model,
}

impl Snapshot {
    pub fn new(replica: &str, base: &Model, model: &Model) -> Result<Snapshot, String> {
        check_replica(replica)?;
        Ok(Snapshot { replica: replica.trim().to_string(), base_hash: base.state_hash().to_string(), model: model.clone() })
    }
}

fn check_replica(name: &str) -> Result<(), String> {
    if name.trim().is_empty() {
        return Err("collab: the replica name (who you are in this session) is required".into());
    }
    if name.trim().chars().count() > 64 {
        return Err("collab: the replica name is longer than 64 characters".into());
    }
    Ok(())
}

/// A disagreement that needs a person's choice.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Conflict {
    /// `<element id>|<path>`: the key to put in `choices`.
    pub key: String,
    pub element: String,
    /// The element's type, e.g. `Wall`.
    pub kind: String,
    /// `element` (deleted on one side, changed on the other) or the property path (`props.thickness`, `locked`, `props.meta.title`).
    pub path: String,
    pub base: Option<Value>,
    /// What each replica has (`null` = absent / deleted there), keyed by replica name.
    pub values: BTreeMap<String, Option<Value>>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct MergeStats {
    /// Elements only the first / second replica (by name order) added.
    pub added_by_first: usize,
    pub added_by_second: usize,
    pub deleted: usize,
    /// Elements both replicas changed, in different properties, merged without a conflict.
    pub merged_both: usize,
    pub conflicts: usize,
    pub resolved: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct MergeResult {
    /// The two replica names in the order used (alphabetical).
    pub replicas: [String; 2],
    pub conflicts: Vec<Conflict>,
    /// Why a conflict-free merge is still refused (an element would be invalid).
    pub issues: Vec<String>,
    /// The merged model; only when there is no unresolved conflict and no issue.
    #[serde(skip)]
    pub model: Option<Model>,
    pub stats: MergeStats,
}

struct Ctx<'a> {
    names: &'a [String; 2],
    choices: &'a BTreeMap<String, String>,
    id: String,
    kind: String,
    conflicts: Vec<Conflict>,
    used: BTreeSet<String>,
}

impl Ctx<'_> {
    /// The value to keep at `path` when both replicas changed it differently.
    fn conflict(&mut self, path: &str, base: Option<&Value>, x: Option<&Value>, y: Option<&Value>) -> Option<Value> {
        let key = format!("{}|{path}", self.id);
        if let Some(rep) = self.choices.get(&key) {
            self.used.insert(key);
            return if *rep == self.names[0] { x.cloned() } else { y.cloned() };
        }
        let mut values = BTreeMap::new();
        values.insert(self.names[0].clone(), x.cloned());
        values.insert(self.names[1].clone(), y.cloned());
        self.conflicts.push(Conflict { key, element: self.id.clone(), kind: self.kind.clone(), path: path.to_string(), base: base.cloned(), values });
        x.cloned()
    }
}

/// Three-way merge of one value, `None` meaning absent.
fn merge_value(ctx: &mut Ctx, path: &str, b: Option<&Value>, x: Option<&Value>, y: Option<&Value>) -> Option<Value> {
    if x == y {
        return x.cloned();
    }
    if x == b {
        return y.cloned();
    }
    if y == b {
        return x.cloned();
    }
    if let (Some(Value::Object(xo)), Some(Value::Object(yo))) = (x, y) {
        let empty = Map::new();
        let bo = match b {
            Some(Value::Object(m)) => m,
            _ => &empty,
        };
        let keys: BTreeSet<&String> = xo.keys().chain(yo.keys()).chain(bo.keys()).collect();
        let mut out = Map::new();
        for k in keys {
            let p = if path.is_empty() { k.clone() } else { format!("{path}.{k}") };
            if let Some(v) = merge_value(ctx, &p, bo.get(k), xo.get(k), yo.get(k)) {
                out.insert(k.clone(), v);
            }
        }
        return Some(Value::Object(out));
    }
    ctx.conflict(path, b, x, y)
}

/// The mergeable part of an element: everything except its id and its revision counter.
fn body(el: &Element) -> Value {
    let mut v = serde_json::to_value(el).expect("an element serializes");
    if let Value::Object(m) = &mut v {
        m.remove("id");
        m.remove("rev");
    }
    v
}

fn rebuild(id: Id, mut v: Value, rev: u64) -> Result<Element, String> {
    let m = v.as_object_mut().ok_or("collab: a merged element is not an object")?;
    m.insert("id".into(), Value::String(id.to_string()));
    m.insert("rev".into(), Value::from(rev));
    serde_json::from_value(v).map_err(|e| format!("collab: element {id} cannot be rebuilt after the merge: {e}"))
}

/// Merges `a` and `b` (any order: the result does not depend on it) over their common `base`. `choices` maps a conflict key to the
/// name of the replica whose value is kept.
pub fn merge(base: &Model, a: (&str, &Model), b: (&str, &Model), choices: &BTreeMap<String, String>) -> Result<MergeResult, String> {
    check_replica(a.0)?;
    check_replica(b.0)?;
    let (a, b) = if a.0.trim() <= b.0.trim() { (a, b) } else { (b, a) };
    if a.0.trim() == b.0.trim() {
        return Err("collab: the two replicas have the same name; each person needs a different one".into());
    }
    let names = [a.0.trim().to_string(), b.0.trim().to_string()];
    for (k, rep) in choices {
        if *rep != names[0] && *rep != names[1] {
            return Err(format!("collab: the choice for '{k}' names '{rep}', which is not one of the replicas ({} / {})", names[0], names[1]));
        }
    }
    let (xm, ym) = (a.1, b.1);
    let ids: BTreeSet<Id> = base.elements.keys().chain(xm.elements.keys()).chain(ym.elements.keys()).copied().collect();
    let mut ctx = Ctx { names: &names, choices, id: String::new(), kind: String::new(), conflicts: Vec::new(), used: BTreeSet::new() };
    let mut stats = MergeStats::default();
    let mut out = Model::default();
    for id in ids {
        let (eb, ex, ey) = (base.get(&id), xm.get(&id), ym.get(&id));
        ctx.id = id.to_string();
        ctx.kind = ex.or(ey).or(eb).map(|e| e.kind.clone()).unwrap_or_default();
        match (eb, ex, ey) {
            (_, None, None) => {
                if eb.is_some() {
                    stats.deleted += 1;
                }
            }
            (None, Some(e), None) => {
                stats.added_by_first += 1;
                out.elements.insert(id, e.clone());
            }
            (None, None, Some(e)) => {
                stats.added_by_second += 1;
                out.elements.insert(id, e.clone());
            }
            (Some(b0), Some(x), None) | (Some(b0), None, Some(x)) => {
                // One side deleted it. Unchanged on the other side: it is gone. Changed: somebody must choose.
                if x == b0 {
                    stats.deleted += 1;
                    continue;
                }
                let first_has = ex.is_some();
                let (vx, vy) = if first_has { (Some(body(x)), None) } else { (None, Some(body(x)))};
                let kept = ctx.conflict("element", Some(&body(b0)), vx.as_ref(), vy.as_ref());
                if kept.is_some() {
                    out.elements.insert(id, x.clone());
                } else {
                    stats.deleted += 1;
                }
            }
            (_, Some(x), Some(y)) => {
                let before = ctx.conflicts.len();
                let merged = merge_value(&mut ctx, "", eb.map(body).as_ref(), Some(&body(x)), Some(&body(y))).ok_or("collab: a merged element vanished")?;
                let rev = x.rev.max(y.rev);
                let mut el = rebuild(id, merged.clone(), rev)?;
                if body(x) != merged && body(y) != merged {
                    el.rev = rev + 1;
                }
                if eb.is_some_and(|b0| body(x) != body(b0) && body(y) != body(b0)) && body(x) != body(y) && ctx.conflicts.len() == before {
                    stats.merged_both += 1;
                }
                out.elements.insert(id, el);
            }
        }
    }
    stats.conflicts = ctx.conflicts.len();
    stats.resolved = ctx.used.len();
    if let Some(stale) = choices.keys().find(|k| !ctx.used.contains(*k) && !ctx.conflicts.iter().any(|c| &c.key == *k)) {
        return Err(format!("collab: the choice for '{stale}' matches no conflict between these two copies"));
    }
    let conflicts = std::mem::take(&mut ctx.conflicts);
    let mut issues = Vec::new();
    if conflicts.is_empty() {
        for el in out.elements.values() {
            if let Some(h) = el.host_id
                && !out.elements.contains_key(&h)
            {
                issues.push(format!("{} {} is hosted by element {h}, which the merge removed; keep that element or remove this one", el.kind, el.id));
            }
        }
        if issues.is_empty() {
            for el in out.elements.values() {
                let changed = xm.get(&el.id) != Some(el) && ym.get(&el.id) != Some(el);
                if changed && let Err(e) = validate_element(&out, el) {
                    issues.push(format!("{} {} would be invalid after the merge: {e}", el.kind, el.id));
                }
            }
        }
    }
    let model = (conflicts.is_empty() && issues.is_empty()).then_some(out);
    Ok(MergeResult { replicas: names, conflicts, issues, model, stats })
}

/// The commands that turn `from` into `to`: creations (hosts first), whole-element restores, deletions (dependents first).
pub fn diff_commands(from: &Model, to: &Model) -> Vec<Command> {
    let mut create: Vec<&Element> = to.elements.values().filter(|e| !from.elements.contains_key(&e.id)).collect();
    let mut cmds = Vec::new();
    let mut placed: BTreeSet<Id> = from.elements.keys().copied().collect();
    while !create.is_empty() {
        let (ready, rest): (Vec<&Element>, Vec<&Element>) = create.into_iter().partition(|e| e.host_id.is_none_or(|h| placed.contains(&h)));
        if ready.is_empty() {
            // A host that never appears: the command fails by name when applied.
            cmds.extend(rest.into_iter().map(|e| Command::Create(e.clone())));
            break;
        }
        for e in ready {
            placed.insert(e.id);
            cmds.push(Command::Create(e.clone()));
        }
        create = rest;
    }
    for (id, e) in &to.elements {
        if from.elements.get(id).is_some_and(|f| f != e) {
            cmds.push(Command::Restore(e.clone()));
        }
    }
    let mut gone: Vec<&Element> = from.elements.values().filter(|e| !to.elements.contains_key(&e.id)).collect();
    while !gone.is_empty() {
        let hosts: BTreeSet<Id> = gone.iter().filter_map(|e| e.host_id).collect();
        let (blocked, ready): (Vec<&Element>, Vec<&Element>) = gone.into_iter().partition(|e| hosts.contains(&e.id));
        if ready.is_empty() {
            // A hosting cycle cannot occur in a valid model; the delete fails by name if it does.
            cmds.extend(blocked.into_iter().map(|e| Command::Delete(e.id)));
            break;
        }
        cmds.extend(ready.into_iter().map(|e| Command::Delete(e.id)));
        gone = blocked;
    }
    cmds
}

/// Applies a merged model to the open document as ONE undoable step. Returns the number of commands. Refuses when the document is not
/// the copy that was exchanged (`expected_hash`): edits made after the exchange would otherwise be overwritten.
pub fn apply_merged(doc: &mut Document, merged: &Model, expected_hash: u64, label: &str) -> Result<usize, String> {
    if doc.model.state_hash() != expected_hash {
        return Err("collab: the project changed after the exchange with your partner; exchange again before merging".into());
    }
    let cmds = diff_commands(&doc.model, merged);
    let n = cmds.len();
    if n == 0 {
        return Ok(0);
    }
    doc.commit(label, Origin::User, cmds).map_err(|e| format!("collab: the merge could not be applied: {e}"))?;
    if doc.model != *merged {
        return Err("collab: the document does not equal the merged model after applying it".into());
    }
    Ok(n)
}

// ---------------------------------------------------------------------------------------------------------------------------------
// Transport
// ---------------------------------------------------------------------------------------------------------------------------------

#[derive(Serialize, Deserialize)]
struct Hello {
    code: String,
    snapshot: Snapshot,
}

#[derive(Serialize, Deserialize)]
struct Reply {
    error: Option<String>,
    snapshot: Option<Snapshot>,
}

fn write_frame(s: &mut TcpStream, v: &impl Serialize) -> Result<(), String> {
    let bytes = serde_json::to_vec(v).map_err(|e| format!("collab: cannot encode the message: {e}"))?;
    if bytes.len() > MAX_FRAME {
        return Err("collab: the project is too large to send (over 128 MB)".into());
    }
    let len = u32::try_from(bytes.len()).map_err(|_| "collab: the message is too large".to_string())?;
    s.write_all(&len.to_be_bytes()).and_then(|()| s.write_all(&bytes)).and_then(|()| s.flush()).map_err(|e| format!("collab: the connection failed while sending: {e}"))
}

fn read_frame<T: for<'de> Deserialize<'de>>(s: &mut TcpStream) -> Result<T, String> {
    let mut len = [0u8; 4];
    s.read_exact(&mut len).map_err(|e| format!("collab: the connection failed while receiving: {e}"))?;
    let n = u32::from_be_bytes(len) as usize;
    if n > MAX_FRAME {
        return Err("collab: the partner announced a message over 128 MB; refused".into());
    }
    let mut buf = vec![0u8; n];
    s.read_exact(&mut buf).map_err(|e| format!("collab: the connection failed while receiving: {e}"))?;
    serde_json::from_slice(&buf).map_err(|e| format!("collab: the partner's message is not valid: {e}"))
}

fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |d, (x, y)| d | (x ^ y)) == 0
}

fn check_code(code: &str) -> Result<(), String> {
    if code.chars().count() < MIN_CODE_LEN {
        return Err(format!("collab: the session code needs at least {MIN_CODE_LEN} characters"));
    }
    Ok(())
}

fn check_wait(wait: Duration) -> Result<(), String> {
    if wait.is_zero() || wait > Duration::from_secs(MAX_WAIT_S) {
        return Err(format!("collab: the wait must be between 1 second and {MAX_WAIT_S} seconds"));
    }
    Ok(())
}

fn compatible(mine: &Snapshot, theirs: &Snapshot) -> Result<(), String> {
    if mine.replica == theirs.replica {
        return Err(format!("collab: both sides are named '{}'; each person needs a different name", mine.replica));
    }
    if mine.base_hash != theirs.base_hash {
        return Err("collab: the two copies did not start from the same saved state, so they cannot be merged; both people must open the same saved file first".into());
    }
    Ok(())
}

/// Where the host stands.
#[derive(Debug, Clone, PartialEq)]
pub enum HostState {
    Waiting,
    Done(Box<Snapshot>),
    Failed(String),
}

/// Waits for one partner. Dropping it (or `stop`) closes the listener.
pub struct Host {
    addr: SocketAddr,
    stop: Arc<AtomicBool>,
    state: Arc<Mutex<HostState>>,
    worker: Option<JoinHandle<()>>,
}

impl Host {
    /// `local` listens on this machine only; otherwise on every interface. `port` 0 lets the OS choose one.
    pub fn start(code: &str, local: bool, port: u16, wait: Duration, mine: Snapshot) -> Result<Host, String> {
        check_code(code)?;
        check_wait(wait)?;
        let ip: IpAddr = if local { Ipv4Addr::LOCALHOST.into() } else { Ipv4Addr::UNSPECIFIED.into() };
        let listener = TcpListener::bind((ip, port)).map_err(|e| format!("collab: cannot listen on port {port}: {e}"))?;
        listener.set_nonblocking(true).map_err(|e| format!("collab: cannot configure the listener: {e}"))?;
        let addr = listener.local_addr().map_err(|e| format!("collab: cannot read the listening address: {e}"))?;
        let stop = Arc::new(AtomicBool::new(false));
        let state = Arc::new(Mutex::new(HostState::Waiting));
        let deadline = Instant::now() + wait;
        let (stop2, state2, code) = (stop.clone(), state.clone(), code.to_string());
        let worker = std::thread::spawn(move || {
            let finish = |s: HostState| {
                *state2.lock().unwrap_or_else(|p| p.into_inner()) = s;
            };
            let mut bad = 0usize;
            while !stop2.load(Ordering::Acquire) {
                if Instant::now() >= deadline {
                    finish(HostState::Failed("collab: no partner joined before the wait ended".into()));
                    return;
                }
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let _ = stream.set_nonblocking(false);
                        let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
                        let _ = stream.set_write_timeout(Some(Duration::from_secs(30)));
                        match read_frame::<Hello>(&mut stream) {
                            Ok(h) if same(h.code.as_bytes(), code.as_bytes()) => match compatible(&mine, &h.snapshot) {
                                Ok(()) => {
                                    let sent = write_frame(&mut stream, &Reply { error: None, snapshot: Some(mine.clone()) });
                                    let _ = stream.shutdown(Shutdown::Both);
                                    finish(match sent {
                                        Ok(()) => HostState::Done(Box::new(h.snapshot)),
                                        Err(e) => HostState::Failed(e),
                                    });
                                    return;
                                }
                                Err(e) => {
                                    let _ = write_frame(&mut stream, &Reply { error: Some(e.clone()), snapshot: None });
                                    let _ = stream.shutdown(Shutdown::Both);
                                    finish(HostState::Failed(e));
                                    return;
                                }
                            },
                            Ok(_) => {
                                bad += 1;
                                let _ = write_frame(&mut stream, &Reply { error: Some("collab: wrong session code".into()), snapshot: None });
                                let _ = stream.shutdown(Shutdown::Both);
                                if bad >= MAX_BAD_ATTEMPTS {
                                    finish(HostState::Failed(format!("collab: {MAX_BAD_ATTEMPTS} wrong session codes were tried; the host closed")));
                                    return;
                                }
                            }
                            Err(_) => {
                                let _ = stream.shutdown(Shutdown::Both);
                            }
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(20)),
                    Err(_) => std::thread::sleep(Duration::from_millis(100)),
                }
            }
        });
        Ok(Host { addr, stop, state, worker: Some(worker) })
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    pub fn state(&self) -> HostState {
        self.state.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }

    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(w) = self.worker.take() {
            let _ = w.join();
        }
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Joins a host at `addr` (`ip:port`) and returns the partner's snapshot.
pub fn connect(addr: &str, code: &str, mine: &Snapshot, wait: Duration) -> Result<Snapshot, String> {
    check_code(code)?;
    check_wait(wait)?;
    let sock: SocketAddr = addr.trim().parse().map_err(|_| format!("collab: '{addr}' is not an address like 192.168.1.20:7070"))?;
    let mut s = TcpStream::connect_timeout(&sock, wait).map_err(|e| format!("collab: cannot reach {sock}: {e}"))?;
    s.set_read_timeout(Some(wait.max(Duration::from_secs(30)))).map_err(|e| format!("collab: {e}"))?;
    s.set_write_timeout(Some(Duration::from_secs(30))).map_err(|e| format!("collab: {e}"))?;
    write_frame(&mut s, &Hello { code: code.to_string(), snapshot: mine.clone() })?;
    let r: Reply = read_frame(&mut s)?;
    if let Some(e) = r.error {
        return Err(e);
    }
    let theirs = r.snapshot.ok_or("collab: the host sent no project")?;
    compatible(mine, &theirs)?;
    Ok(theirs)
}
