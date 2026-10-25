//! The recording buffer: the commands a person (or an agent) ran while recording, each with the
//! document, sheet and selection it ran in.
//!
//! Only top-level calls are kept. A command that runs other commands (Custom Views › Show, a
//! script) is one step, so playing the recording back does not run the inner commands twice. The
//! buffer is separate from the journal, so trimming the journal never loses recorded steps, and
//! secret parameters are left out as they are in the journal ([`crate::cmd::journal_params`]).

use serde::Serialize;
use serde_json::Value as Json;

use crate::Selection;

/// The most steps one recording keeps; later ones are counted in [`Recorder::dropped`].
pub const RECORD_LIMIT: usize = 100_000;
/// The most bytes (steps serialized as JSON) one recording keeps. A step keeps its full
/// parameters so it can be played back, and a `file.open` holds the whole file as base64, so
/// without a size cap a recording of a few opens of a large workbook keeps tens of MB alive.
pub const RECORD_MAX_BYTES: usize = 64 * 1024 * 1024;

/// Commands never recorded: recovery actions, not steps of the person's work. A recording that
/// replayed Stop Action would stop the run it is a step of.
pub const NOT_RECORDED: [&str; 1] = ["automate.stopAction"];

/// One recorded command and where it ran.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordedStep {
    /// Uid of the document that was active when the command ran (0 when none was open).
    pub doc: u64,
    /// That document's active sheet (index and name) just before the command ran.
    pub sheet: usize,
    pub sheet_name: String,
    /// That document's selection just before the command ran.
    pub selection: Selection,
    /// The command id and its parameters, without secrets.
    pub id: String,
    pub params: Json,
}

/// Numbers each recording, so an undo group's mark from an earlier one isn't read as this one's.
static NEXT_RECORDING: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// The steps recorded since recording started.
#[derive(Clone, Debug)]
pub struct Recorder {
    pub steps: Vec<RecordedStep>,
    /// Steps not kept because the recording reached [`RECORD_LIMIT`] or [`RECORD_MAX_BYTES`].
    /// Once a step is dropped, every later one is too, so `steps` is always the beginning of
    /// what ran, never a sequence with gaps.
    pub dropped: usize,
    /// The size of `steps` serialized as JSON.
    bytes: usize,
    /// Which recording this is, and the number the next kept step gets: each step's number is in
    /// `seqs`, beside it, so a mark stays right however many steps rollbacks take out.
    generation: u64,
    next_seq: u64,
    seqs: Vec<u64>,
}

impl Default for Recorder {
    fn default() -> Self {
        let generation = NEXT_RECORDING.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Recorder { steps: Vec::new(), dropped: 0, bytes: 0, generation, next_seq: 0, seqs: Vec::new() }
    }
}

impl Recorder {
    /// Adds a step, or counts it as dropped once the buffer is full.
    pub fn push(&mut self, step: RecordedStep) {
        // Once a step has been dropped, or the step count is full, the rest are counted without
        // being serialized (a `file.open` step holds a whole file).
        if self.dropped > 0 || self.steps.len() >= RECORD_LIMIT {
            self.dropped = self.dropped.saturating_add(1);
            return;
        }
        let size = serde_json::to_vec(&step).map_or(0, |v| v.len());
        if self.bytes.saturating_add(size) <= RECORD_MAX_BYTES {
            self.bytes = self.bytes.saturating_add(size);
            self.steps.push(step);
            self.seqs.push(self.next_seq);
            self.next_seq = self.next_seq.saturating_add(1);
        } else {
            self.dropped = self.dropped.saturating_add(1);
        }
    }

    /// Where the recording is now, for [`Recorder::cut_back`]: this recording and the number its
    /// next step gets.
    pub(crate) fn mark(&self) -> (u64, u64) {
        (self.generation, self.next_seq)
    }

    /// Takes back the steps recorded in the document with uid `doc` since `mark` (an undo group
    /// on it was rolled back, so playing them back would make edits it no longer has). Steps in
    /// other documents stay: their edits do too. A mark from another recording (the recording
    /// started again inside the group) means every step of this one came after it. Steps the
    /// buffer dropped stay counted.
    pub(crate) fn cut_back(&mut self, (generation, seq): (u64, u64), doc: u64) {
        let from = if generation == self.generation { seq } else { 0 };
        let gone = |i: usize, step: &RecordedStep| step.doc == doc && self.seqs.get(i).is_some_and(|&n| n >= from);
        if !self.steps.iter().enumerate().any(|(i, s)| gone(i, s)) {
            return;
        }
        let (mut steps, mut seqs) = (Vec::with_capacity(self.steps.len()), Vec::with_capacity(self.seqs.len()));
        for (i, step) in self.steps.iter().enumerate() {
            if !gone(i, step) {
                steps.push(step.clone());
                seqs.push(self.seqs.get(i).copied().unwrap_or(0));
            }
        }
        (self.steps, self.seqs) = (steps, seqs);
        self.bytes = self.steps.iter().map(|s| serde_json::to_vec(s).map_or(0, |v| v.len())).sum();
    }
}
