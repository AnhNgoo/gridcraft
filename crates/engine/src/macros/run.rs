//! Running a macro.
//!
//! 1. **Gate**: every step's command goes through [`macro_policy`]. Denied commands stop the macro
//!    before anything runs; commands that touch files must be listed in the macro's permissions;
//!    a macro from a file someone else made, or one that touches files, runs only once the person
//!    approves it (once per macro content, for the session).
//! 2. **Inputs**: each declared input is bound to a range (given, its default, or the selection)
//!    and checked against its shape and column types, with messages that name the cells.
//! 3. **Conflicts**: unless the macro may overwrite, each step is checked as it runs. A step that
//!    writes over data outside the inputs (cells that held data when the run started, sheets and
//!    objects it deletes) stops the run, which asks (`ask`) or fails (the other rules), and the
//!    run rolls back. Shifting the data to make room (`shiftDown`, `shiftRight`) needs to know
//!    where the macro writes before it runs, which this version doesn't work out, so those rules
//!    stop the run as `fail` does.
//! 4. **Run**: one undo step (`Session::grouped`); an error at any step rolls everything back and
//!    names the step.
//!
//! The steps run on a small machine with an explicit frame stack, so the debugger can stop
//! between any two steps and pick up again on a later frame.

use std::collections::BTreeSet;
use std::sync::Arc;

use gridcraft_core::{CellRef, RangeRef, Value};
use gridcraft_model::{Cell, CompareOp, InputKind, Macro, OnConflict, RepeatOf, Sheet, Step, StepKind, Workbook};
use serde::Serialize;
use serde_json::{Value as Json, json};

use super::policy::{Policy, macro_policy};
use super::resolve::{self, Env, Place, ResolveError};
use crate::{EngineError, Result, Session, Trust, UiRequest};

/// The most steps one run executes (counting each pass through a Repeat).
pub const MAX_EXECUTED_STEPS: u64 = 100_000;
/// How long a run may take, in milliseconds. The web build has no clock; there only the step
/// limit applies.
pub const DEFAULT_TIME_LIMIT_MS: f64 = 60_000.0;
/// The most cells an input may have for its content to be checked.
pub const MAX_INPUT_CELLS: u64 = 1_000_000;
/// The most ranges a list of cells gives per sheet (the counts stay exact).
const MAX_LISTED_RANGES: usize = 500;

/// An input bound to a range: name, where, whether its first row is a header.
pub type Bound = (String, Place, bool);

/// Where and why a step failed.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct StepError {
    /// The step's path: `3`, `2.1` inside a Repeat, `4.then.1` inside an If.
    pub step: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    pub message: String,
    /// The step ran past the edge of the sheet.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub edge: bool,
    /// The step would write over data and the conflict rule is ask: overwriting answers it.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub confirm: bool,
}

impl StepError {
    fn new(step: &str, command: Option<&str>, message: impl Into<String>) -> StepError {
        StepError { step: step.to_string(), command: command.map(str::to_string), message: message.into(), edge: false, confirm: false }
    }

    fn resolve(step: &str, command: Option<&str>, e: ResolveError) -> StepError {
        let edge = matches!(e, ResolveError::Edge(_));
        StepError { edge, ..StepError::new(step, command, e.message()) }
    }

    /// `Step 3 (cell.set): …`
    pub fn describe(&self) -> String {
        match &self.command {
            Some(c) => format!("Step {} ({c}): {}", self.step, self.message),
            None => format!("Step {}: {}", self.step, self.message),
        }
    }

    pub fn into_engine(self, name: &str) -> EngineError {
        let text = format!("Macro '{name}' stopped. {} Nothing was changed.", self.describe());
        if self.confirm { EngineError::Confirm(text) } else { EngineError::Other(text) }
    }
}

// ---------------------------------------------------------------- gate

/// Checks every enabled step against the policy: denied commands and undeclared file access are
/// errors; otherwise returns what the person must approve (empty when nothing).
pub fn gate(m: &Macro, trust: Trust) -> Result<Vec<String>> {
    let mut asks = BTreeSet::new();
    let mut err = None;
    walk_enabled(&m.steps, "", &mut |path, id, params| {
        if err.is_some() {
            return;
        }
        match macro_policy(id, params, trust) {
            Policy::Allow => {}
            Policy::Ask(why) => {
                asks.insert(why);
            }
            Policy::NeedsPermission(what) => {
                if m.permissions.iter().any(|p| p == id) {
                    asks.insert(format!("runs {id}, which {what}"));
                } else {
                    err = Some(format!("Step {path} ({id}) {what}. Add {id} to the macro's permissions to allow it."));
                }
            }
            Policy::Deny(why) => err = Some(format!("Step {path} ({id}) can't run in a macro: {why}.")),
        }
    });
    match err {
        Some(e) => Err(EngineError::Other(format!("Macro '{}' can't run. {e}", m.name))),
        None => Ok(asks.into_iter().collect()),
    }
}

/// Calls `f(path, id, params)` for each enabled command step (skipping disabled steps' bodies).
fn walk_enabled(steps: &[Step], prefix: &str, f: &mut impl FnMut(&str, &str, &Json)) {
    for (i, st) in steps.iter().enumerate() {
        if !st.enabled {
            continue;
        }
        let path = child_path(prefix, i);
        match &st.kind {
            StepKind::Command { id, params } => f(&path, id, params),
            StepKind::Repeat { body, .. } => walk_enabled(body, &path, f),
            StepKind::If { then, otherwise, .. } => {
                walk_enabled(then, &format!("{path}.then"), f);
                walk_enabled(otherwise, &format!("{path}.else"), f);
            }
            StepKind::Stop { .. } => {}
        }
    }
}

fn child_path(prefix: &str, i: usize) -> String {
    if prefix.is_empty() { (i + 1).to_string() } else { format!("{prefix}.{}", i + 1) }
}

/// Approves the macro for this session when `approve` is set, or asks (an
/// [`EngineError::Approve`]) when it needs approval it doesn't have.
pub fn approve(s: &mut Session, m: &Macro, doc: u64, asks: &[String], approve: bool) -> Result<()> {
    if asks.is_empty() {
        return Ok(());
    }
    let key = (doc, super::fingerprint(m));
    if s.macro_approvals.contains(&key) {
        return Ok(());
    }
    if approve {
        s.macro_approvals.insert(key);
        return Ok(());
    }
    Err(EngineError::Approve(format!("Run macro '{}'? It {}.", m.name, join_and(asks))))
}

fn join_and(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [init @ .., last] => format!("{} and {last}", init.join(", ")),
    }
}

// ---------------------------------------------------------------- inputs

/// A macro ready to run: its inputs bound and checked, its output anchor resolved.
#[derive(Clone, Debug)]
pub struct Prepared {
    pub mac: Macro,
    pub doc: u64,
    pub trust: Trust,
    pub inputs: Vec<Bound>,
    pub output: Option<Place>,
}

impl Prepared {
    pub fn inputs_json(&self, wb: &Workbook) -> Json {
        let env = Env { wb, active_sheet: wb.active_sheet, inputs: &[], output: None, item: None };
        Json::Object(self.inputs.iter().map(|(n, p, _)| (n.clone(), json!(resolve::a1(*p, &env)))).collect())
    }
}

/// Binds the macro's inputs on the active document (`given`: `{"Input": "B2:D9"}`, or a reference
/// for the first input) and checks them. Errors list every problem found.
pub fn prepare(s: &Session, m: &Macro, given: Option<&Json>) -> Result<Prepared> {
    let d = s.doc()?;
    let wb = &d.wb;
    let mut inputs: Vec<Bound> = Vec::new();
    let mut problems = Vec::new();
    for (i, input) in m.inputs.iter().enumerate() {
        let named = given.and_then(Json::as_object).and_then(|o| o.iter().find(|(k, _)| k.eq_ignore_ascii_case(&input.name)).map(|(_, v)| v));
        let text = named.and_then(Json::as_str).or_else(|| if i == 0 { given.and_then(Json::as_str) } else { None }).or(input.default_ref.as_deref());
        let place = match text {
            Some(t) => {
                let env = Env { wb, active_sheet: wb.active_sheet, inputs: &inputs, output: None, item: None };
                match resolve::resolve_ref(t, &env) {
                    Ok(p) => p,
                    Err(e) => {
                        problems.push(format!("{}: {}", input.name, e.message()));
                        continue;
                    }
                }
            }
            None if i == 0 => Place { sheet: wb.active_sheet, range: d.selection.current() },
            None => {
                problems.push(format!("{0} needs a range: pass inputs: {{\"{0}\": \"A1:B5\"}}.", input.name));
                continue;
            }
        };
        let place = trim(wb, place);
        problems.extend(check_input(wb, input, place));
        inputs.push((input.name.clone(), place, input.header));
    }
    if !problems.is_empty() {
        return Err(EngineError::Other(format!("Macro '{}' can't run on these inputs. {}", m.name, problems.join(" "))));
    }
    let output = match &m.output.anchor {
        Some(a) if !a.trim().is_empty() => {
            let env = Env { wb, active_sheet: wb.active_sheet, inputs: &inputs, output: None, item: None };
            Some(
                resolve::resolve_ref(a, &env)
                    .map_err(|e| EngineError::Other(format!("Macro '{}': its output anchor {}", m.name, lower_first(e.message()))))?,
            )
        }
        _ => None,
    };
    Ok(Prepared { mac: m.clone(), doc: d.uid, trust: d.trust, inputs, output })
}

fn lower_first(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_lowercase().chain(c).collect(),
        None => String::new(),
    }
}

/// Whole rows or columns shrink to the part of the sheet in use.
fn trim(wb: &Workbook, p: Place) -> Place {
    if !(p.range.is_full_cols() || p.range.is_full_rows()) {
        return p;
    }
    let used = wb.sheet(p.sheet).and_then(Sheet::used_range);
    let range = used.and_then(|u| p.range.intersection(&u)).unwrap_or(RangeRef::cell(p.range.start));
    Place { sheet: p.sheet, range }
}

/// What is wrong with `place` as `input`, one sentence each.
fn check_input(wb: &Workbook, input: &gridcraft_model::NamedInput, place: Place) -> Vec<String> {
    let mut out = Vec::new();
    let r = place.range;
    let name = &input.name;
    let at = r.a1();
    if input.kind == InputKind::Cell && !r.is_single() {
        out.push(format!("{name} must be a single cell; {at} is {} cells.", r.count()));
        return out;
    }
    let header = u32::from(input.header && r.height() > 0);
    let rows = r.height().saturating_sub(header);
    let between = |what: &str, n: u32, [min, max]: [u32; 2]| -> Option<String> {
        let plural = |k: u32| if k == 1 { what.to_string() } else { format!("{what}s") };
        if max > 0 && min == max && n != min {
            Some(format!("exactly {min} {}", plural(min)))
        } else if n < min {
            Some(format!("at least {min} {}", plural(min)))
        } else if max > 0 && n > max {
            Some(format!("at most {max} {}", plural(max)))
        } else {
            None
        }
    };
    if let Some(want) = between("data row", rows, input.rows) {
        out.push(format!("{name} needs {want}; {at} has {rows}."));
    }
    if let Some(want) = between("column", r.width(), input.cols) {
        out.push(format!("{name} needs {want}; {at} has {}.", r.width()));
    }
    if r.count() > MAX_INPUT_CELLS {
        out.push(format!("{name} is too large to check ({} cells; at most {}).", r.count(), MAX_INPUT_CELLS));
        return out;
    }
    let Some(sh) = wb.sheet(place.sheet) else { return out };
    let mut blanks = Vec::new();
    for (ci, col) in (r.start.col..=r.end.col).enumerate() {
        let ty = input.col_types.get(ci).copied().unwrap_or_default();
        let mut wrong = Vec::new();
        for row in r.start.row + header..=r.end.row {
            let c = CellRef::new(row, col);
            let v = sh.value(c);
            if v.is_empty() || v.as_text() == Some("") {
                if input.non_empty {
                    blanks.push(c);
                }
                continue;
            }
            let date = || wb.styles.get(sh.style_id(c)).num_fmt.as_str().to_string();
            let ok = match ty {
                gridcraft_model::ColType::Any => true,
                gridcraft_model::ColType::Number => v.is_number(),
                gridcraft_model::ColType::Text => v.is_text(),
                gridcraft_model::ColType::Bool => matches!(v, Value::Bool(_)),
                gridcraft_model::ColType::Date => v.is_number() && crate::display::number_format(&date()).is_date(),
            };
            if !ok {
                wrong.push((c, holds(&v, ty)));
            }
        }
        if !wrong.is_empty() {
            let head = if input.header {
                Some(crate::display::cell_text(wb, sh, CellRef::new(r.start.row, col))).filter(|h| !h.trim().is_empty())
            } else {
                None
            };
            let column = match head {
                Some(h) => format!("{name} column {} ({})", ci + 1, h.trim()),
                None => format!("{name} column {}", ci + 1),
            };
            let listed: Vec<String> = wrong.iter().take(3).map(|(c, what)| format!("{} holds {what}", c.a1())).collect();
            let more = if wrong.len() > 3 { format!(" and {} more cells don't", wrong.len() - 3) } else { String::new() };
            out.push(format!("{column} must hold {}; {}{more}.", ty.noun(), listed.join(", ")));
        }
    }
    if let Some(first) = blanks.first() {
        let more = if blanks.len() > 1 { format!(" (and {} more)", blanks.len() - 1) } else { String::new() };
        out.push(format!("{name} must have no blank cells; {} is blank{more}.", first.a1()));
    }
    out
}

/// What a cell holds, for "C7 holds text".
fn holds(v: &Value, wanted: gridcraft_model::ColType) -> &'static str {
    match v {
        Value::Number(_) if wanted == gridcraft_model::ColType::Date => "a number that isn't a date",
        Value::Number(_) => "a number",
        Value::Text(_) => "text",
        Value::Bool(_) => "TRUE or FALSE",
        Value::Error(_) => "an error",
        Value::Array(_) => "an array",
        Value::Empty => "nothing",
    }
}

// ---------------------------------------------------------------- the machine

/// A stopwatch for the time limit. `Instant::now` traps on wasm, so there it measures nothing.
#[derive(Clone, Copy, Debug)]
pub struct Stopwatch {
    #[cfg(not(target_arch = "wasm32"))]
    start: std::time::Instant,
}

impl Stopwatch {
    pub fn start() -> Stopwatch {
        #[cfg(not(target_arch = "wasm32"))]
        #[allow(clippy::disallowed_methods)] // never on wasm (see above)
        let w = Stopwatch { start: std::time::Instant::now() };
        #[cfg(target_arch = "wasm32")]
        let w = Stopwatch {};
        w
    }

    /// Milliseconds since the start, or `None` where there is no clock.
    fn elapsed_ms(&self) -> Option<f64> {
        #[cfg(not(target_arch = "wasm32"))]
        let ms = Some(self.start.elapsed().as_secs_f64() * 1000.0);
        #[cfg(target_arch = "wasm32")]
        let ms = None;
        ms
    }
}

#[derive(Clone, Debug)]
struct Frame {
    steps: Arc<Vec<Step>>,
    next: usize,
    /// Path of the step that opened this frame (`""` at the top, `3` for a Repeat's body,
    /// `4.then` for an If's branch).
    prefix: String,
    repeat: Option<Loop>,
}

#[derive(Clone, Copy, Debug)]
struct Loop {
    of: RepeatOf,
    over: Place,
    /// The current item (0-based) and how many there are.
    at: u32,
    count: u32,
}

impl Loop {
    fn item(&self) -> Place {
        let r = self.over.range;
        let range = match self.of {
            RepeatOf::Rows => {
                let row = r.start.row.saturating_add(self.at);
                RangeRef::new(CellRef::new(row, r.start.col), CellRef::new(row, r.end.col))
            }
            RepeatOf::Columns => {
                let col = r.start.col.saturating_add(self.at);
                RangeRef::new(CellRef::new(r.start.row, col), CellRef::new(r.end.row, col))
            }
        };
        Place { sheet: self.over.sheet, range }
    }
}

/// A macro run in progress: which step is next, and what has run.
#[derive(Clone, Debug)]
pub struct Machine {
    pub name: String,
    pub doc: u64,
    pub inputs: Vec<Bound>,
    pub output: Option<Place>,
    permissions: Vec<String>,
    trust: Trust,
    frames: Vec<Frame>,
    /// Steps executed so far (each pass through a Repeat counts).
    pub executed: u64,
    clock: Stopwatch,
    pub time_limit_ms: f64,
    /// A Stop step ended the run: its message, and whether it asked for a rollback.
    pub stopped: Option<(String, bool)>,
    /// The last step's resolved parameters (for the debugger).
    pub last_params: Option<Json>,
    /// Set unless the macro may overwrite: each step is checked for writes over data as it runs.
    guard: Option<Guard>,
}

/// Checks each step of a run for writes over data the workbook held when the run started.
#[derive(Clone, Debug)]
struct Guard {
    /// The workbook when the run started, and for each sheet now, its index there (`None` for a
    /// sheet the run added).
    start: Arc<Workbook>,
    origin: Vec<Option<usize>>,
    /// The conflict rule is ask.
    ask: bool,
}

impl Guard {
    fn new(start: Arc<Workbook>, ask: bool) -> Guard {
        Guard { origin: (0..start.sheets.len()).map(Some).collect(), start, ask }
    }

    /// What one step wrote over that held data when the run started: cells outside the inputs,
    /// sheets and objects it deleted ("B1 on Sheet1; 1 object"), or `""`.
    fn check(&mut self, prev: &Workbook, after: &Workbook, inputs: &[Bound]) -> String {
        let (pairs, _) = pair_sheets(prev, after);
        let mut origin = vec![None; after.sheets.len()];
        let mut parts = Vec::new();
        let mut objects = 0usize;
        for (pi, ai) in pairs {
            // A sheet the run added holds nothing of the person's.
            let Some(si) = self.origin.get(pi).copied().flatten() else { continue };
            let Some(ss) = self.start.sheets.get(si) else { continue };
            let Some(ai) = ai else {
                parts.push(format!("the sheet {}", ss.name));
                continue;
            };
            if let Some(o) = origin.get_mut(ai) {
                *o = Some(si);
            }
            let (Some(ps), Some(sa)) = (prev.sheets.get(pi), after.sheets.get(ai)) else { continue };
            if Arc::ptr_eq(ps, sa) {
                continue;
            }
            let in_input = |c: CellRef| inputs.iter().any(|(_, p, _)| p.sheet == si && p.range.contains(c));
            let mut hit: Vec<CellRef> = ps
                .cells
                .diff(&sa.cells)
                .into_iter()
                .filter(|&c| !in_input(c) && !recalculated_only(ps.cell(c), sa.cell(c)))
                .filter(|&c| ss.cell(c).is_some_and(has_content) && content_differs(ss.cell(c), sa.cell(c)))
                .collect();
            if !hit.is_empty() {
                let mut listed = ranges(&mut hit);
                if listed.len() > 3 {
                    listed.truncate(3);
                    listed.push("…".into());
                }
                parts.push(format!("{} on {}", listed.join(", "), sa.name));
            }
            objects += deleted(&ss.charts, &ps.charts, &sa.charts, |o| o.id)
                + deleted(&ss.images, &ps.images, &sa.images, |o| o.id)
                + deleted(&ss.shapes, &ps.shapes, &sa.shapes, |o| o.id);
        }
        if objects > 0 {
            parts.push(format!("{objects} object{}", if objects == 1 { "" } else { "s" }));
        }
        self.origin = origin;
        parts.join("; ")
    }
}

/// Objects of `prev` that `after` no longer has and that were there at the start.
fn deleted<T>(start: &[T], prev: &[T], after: &[T], id: impl Fn(&T) -> u32) -> usize {
    let kept: BTreeSet<u32> = after.iter().map(&id).collect();
    let had: BTreeSet<u32> = start.iter().map(&id).collect();
    prev.iter().map(&id).filter(|i| !kept.contains(i) && had.contains(i)).count()
}

impl Machine {
    pub fn new(p: &Prepared) -> Machine {
        let mut m = Machine {
            name: p.mac.name.clone(),
            doc: p.doc,
            inputs: p.inputs.clone(),
            output: p.output,
            permissions: p.mac.permissions.clone(),
            trust: p.trust,
            frames: vec![Frame { steps: Arc::new(p.mac.steps.clone()), next: 0, prefix: String::new(), repeat: None }],
            executed: 0,
            clock: Stopwatch::start(),
            time_limit_ms: DEFAULT_TIME_LIMIT_MS,
            stopped: None,
            last_params: None,
            guard: None,
        };
        m.settle();
        m
    }

    /// Moves to the next enabled step: past disabled steps, finished frames and loop passes.
    fn settle(&mut self) {
        while let Some(f) = self.frames.last_mut() {
            if let Some(st) = f.steps.get(f.next) {
                if st.enabled {
                    return;
                }
                f.next += 1;
                continue;
            }
            if let Some(l) = f.repeat.as_mut()
                && l.at.saturating_add(1) < l.count
            {
                l.at += 1;
                f.next = 0;
                continue;
            }
            self.frames.pop();
        }
    }

    pub fn is_done(&self) -> bool {
        self.frames.is_empty()
    }

    /// The step that runs next, with its path.
    pub fn current(&self) -> Option<(String, &Step)> {
        let f = self.frames.last()?;
        f.steps.get(f.next).map(|st| (child_path(&f.prefix, f.next), st))
    }

    /// The innermost Repeat's pass (1-based) and count.
    pub fn iteration(&self) -> Option<(u32, u32)> {
        self.frames.iter().rev().find_map(|f| f.repeat).map(|l| (l.at + 1, l.count))
    }

    fn item(&self) -> Option<(RepeatOf, Place)> {
        self.frames.iter().rev().find_map(|f| f.repeat).map(|l| (l.of, l.item()))
    }

    /// Runs every remaining step.
    pub fn run_to_end(&mut self, s: &mut Session) -> std::result::Result<(), StepError> {
        while !self.is_done() {
            self.step(s)?;
        }
        Ok(())
    }

    /// Runs the current step and moves to the next one.
    pub fn step(&mut self, s: &mut Session) -> std::result::Result<(), StepError> {
        let Some((path, st)) = self.current().map(|(p, st)| (p, st.clone())) else { return Ok(()) };
        if let Some(f) = self.frames.last_mut() {
            f.next += 1;
        }
        let r = self.exec(s, &path, &st);
        if r.is_err() {
            self.frames.clear();
        }
        self.settle();
        r
    }

    fn exec(&mut self, s: &mut Session, path: &str, st: &Step) -> std::result::Result<(), StepError> {
        self.executed += 1;
        let at = |msg: String| StepError::new(path, None, msg);
        if self.executed > MAX_EXECUTED_STEPS {
            return Err(at(format!("The macro ran more than {MAX_EXECUTED_STEPS} steps and was stopped.")));
        }
        if self.clock.elapsed_ms().is_some_and(|ms| ms > self.time_limit_ms) {
            return Err(at(format!("The macro ran longer than {} seconds and was stopped.", (self.time_limit_ms / 1000.0).round())));
        }
        // Steps run on the macro's workbook, whichever one is in front.
        let Some(i) = s.docs.iter().position(|d| d.uid == self.doc) else { return Err(at("The workbook was closed.".into())) };
        s.set_active(i);
        match &st.kind {
            StepKind::Command { id, params } => self.command(s, path, id, params),
            StepKind::Repeat { over, of, max, body } => {
                let place = {
                    let wb = &s.doc().map_err(|e| at(e.to_string()))?.wb;
                    let env = self.env(wb);
                    let p = resolve::resolve_ref(over, &env).map_err(|e| StepError::resolve(path, None, e))?;
                    trim(wb, p)
                };
                let items = match of {
                    RepeatOf::Rows => place.range.height(),
                    RepeatOf::Columns => place.range.width(),
                };
                let cap = (*max).min(MAX_EXECUTED_STEPS as u32);
                if items > cap {
                    let what = if *of == RepeatOf::Rows { "rows" } else { "columns" };
                    return Err(at(format!("Repeat over {} has {items} {what}, more than its limit of {cap}.", place.range.a1())));
                }
                if items > 0 && body.iter().any(|b| b.enabled) {
                    let repeat = Some(Loop { of: *of, over: place, at: 0, count: items });
                    self.frames.push(Frame { steps: Arc::new(body.clone()), next: 0, prefix: path.to_string(), repeat });
                }
                Ok(())
            }
            StepKind::If { cell, op, value, then, otherwise } => {
                let v = {
                    let wb = &s.doc().map_err(|e| at(e.to_string()))?.wb;
                    let env = self.env(wb);
                    let p = resolve::resolve_ref(cell, &env).map_err(|e| StepError::resolve(path, None, e))?;
                    if !p.range.is_single() {
                        return Err(at(format!("If looks at one cell; {} is {} cells.", p.range.a1(), p.range.count())));
                    }
                    wb.sheet(p.sheet).map(|sh| sh.value(p.range.start)).unwrap_or_default()
                };
                let (branch, name) = if compare(&v, *op, value) { (then, "then") } else { (otherwise, "else") };
                if branch.iter().any(|b| b.enabled) {
                    self.frames.push(Frame { steps: Arc::new(branch.clone()), next: 0, prefix: format!("{path}.{name}"), repeat: None });
                }
                Ok(())
            }
            StepKind::Stop { message, rollback } => {
                self.stopped = Some((message.clone(), *rollback));
                self.frames.clear();
                Ok(())
            }
        }
    }

    fn env<'a>(&'a self, wb: &'a Workbook) -> Env<'a> {
        Env { wb, active_sheet: wb.active_sheet, inputs: &self.inputs, output: self.output, item: self.item() }
    }

    fn command(&mut self, s: &mut Session, path: &str, id: &str, params: &Json) -> std::result::Result<(), StepError> {
        let fail = |msg: String| StepError::new(path, Some(id), msg);
        // The policy sees the parameters the command will get, after tokens became A1 text.
        let resolved = {
            let wb = &s.doc().map_err(|e| fail(e.to_string()))?.wb;
            resolve::resolve_params(params, &self.env(wb)).map_err(|e| StepError::resolve(path, Some(id), e))?
        };
        match (macro_policy(id, params, self.trust), macro_policy(id, &resolved, self.trust)) {
            (_, Policy::Deny(why)) | (Policy::Deny(why), _) => return Err(fail(format!("{id} can't run in a macro: {why}."))),
            (written, Policy::NeedsPermission(what)) => {
                if !self.permissions.iter().any(|p| p == id) {
                    return Err(fail(format!("{id} {what}. Add it to the macro's permissions to allow it.")));
                }
                // The gate asked the person about this step only if it saw the file access.
                if !matches!(written, Policy::NeedsPermission(_)) {
                    return Err(fail(format!("{id} {what} only once its parameters are filled in, so it wasn't approved.")));
                }
            }
            (_, Policy::Allow | Policy::Ask(_)) => {}
        }
        self.last_params = Some(resolved.clone());
        let prev = self.guard.as_ref().and_then(|_| s.docs.iter().find(|d| d.uid == self.doc)).map(|d| d.wb.clone());
        let queued = s.ui_requests.len();
        let r = s.execute(id, resolved);
        // Macros never open dialogs or web pages, and never start editing a cell; messages for
        // the person stay queued.
        let mut blocked = None;
        let asked: Vec<UiRequest> = s.ui_requests.drain(queued.min(s.ui_requests.len())..).collect();
        for q in asked {
            match q {
                UiRequest::Dialog(name, _) => blocked = Some(format!("{id} would open the {name} dialog; give the step the parameters it needs.")),
                UiRequest::OpenUrl(_) => blocked = Some(format!("{id} would open a web page, which macros don't do.")),
                UiRequest::EditCell(_) | UiRequest::InsertName(_) => {}
                m @ UiRequest::Message(_) => s.ui_requests.push(m),
            }
        }
        r.map_err(|e| fail(e.to_string()))?;
        if let Some(b) = blocked {
            return Err(fail(b));
        }
        if let (Some(g), Some(prev)) = (self.guard.as_mut(), prev) {
            let after = s.docs.iter().find(|d| d.uid == self.doc).map(|d| d.wb.clone()).ok_or_else(|| fail("The workbook was closed.".into()))?;
            let what = g.check(&prev, &after, &self.inputs);
            if !what.is_empty() {
                let msg = format!("It would write over data ({what}).");
                return Err(StepError { confirm: g.ask, ..fail(msg) });
            }
        }
        Ok(())
    }
}

/// If's comparison of a cell's value with a step's `value`, the way Excel's `=`, `<`… compare.
fn compare(v: &Value, op: CompareOp, want: &Json) -> bool {
    let empty = v.is_empty() || v.as_text() == Some("");
    let want_text = || match want {
        Json::String(t) => t.clone(),
        Json::Null => String::new(),
        other => other.to_string(),
    };
    let ord = || -> Option<std::cmp::Ordering> {
        let w = match want {
            Json::Null => Value::Empty,
            Json::Bool(b) => Value::Bool(*b),
            Json::Number(n) => Value::number(n.as_f64()?),
            // "5" compares as a number with a number in the cell.
            Json::String(t) => match (v, t.trim().parse::<f64>()) {
                (Value::Number(_), Ok(n)) => Value::number(n),
                _ => Value::text(t.as_str()),
            },
            _ => return None,
        };
        match (v, &w) {
            (Value::Error(e), Value::Text(t)) => {
                Some(if e.as_str().eq_ignore_ascii_case(t.trim()) { std::cmp::Ordering::Equal } else { std::cmp::Ordering::Less })
            }
            (Value::Error(_), _) | (_, Value::Error(_)) | (Value::Array(_), _) => None,
            _ => Some(gridcraft_core::value::compare(v, &w)),
        }
    };
    use std::cmp::Ordering::*;
    match op {
        CompareOp::Empty => empty,
        CompareOp::NotEmpty => !empty,
        CompareOp::Contains => v.display().to_lowercase().contains(&want_text().to_lowercase()),
        CompareOp::Eq => ord() == Some(Equal),
        CompareOp::Ne => ord() != Some(Equal),
        CompareOp::Gt => ord() == Some(Greater),
        CompareOp::Ge => matches!(ord(), Some(Greater | Equal)),
        CompareOp::Lt => ord() == Some(Less),
        CompareOp::Le => matches!(ord(), Some(Less | Equal)),
    }
}

// ---------------------------------------------------------------- comparing workbooks

/// Each sheet of `before` with its index in `after` (`None` when deleted), and the sheets of
/// `after` that are new. Sheets are matched by name; what's left is matched in order (a renamed
/// sheet).
fn pair_sheets(before: &Workbook, after: &Workbook) -> (Vec<(usize, Option<usize>)>, Vec<usize>) {
    let mut pairs: Vec<(usize, Option<usize>)> = before.sheets.iter().enumerate().map(|(bi, sh)| (bi, after.sheet_index(&sh.name))).collect();
    let matched: BTreeSet<usize> = pairs.iter().filter_map(|(_, a)| *a).collect();
    let mut spare = (0..after.sheets.len()).filter(|a| !matched.contains(a));
    for (_, a) in pairs.iter_mut().filter(|(_, a)| a.is_none()) {
        *a = spare.next();
    }
    (pairs, spare.collect())
}

/// Only a formula's cached value changed (a recalculation, not a write).
fn recalculated_only(b: Option<&Cell>, a: Option<&Cell>) -> bool {
    match (b, a) {
        (Some(b), Some(a)) => match (&b.formula, &a.formula) {
            (Some(x), Some(y)) => x.text == y.text && x.array == y.array && b.style == a.style,
            _ => false,
        },
        _ => false,
    }
}

fn has_content(c: &Cell) -> bool {
    c.formula.is_some() || !(c.value.is_empty() || c.value.as_text() == Some(""))
}

fn content_differs(b: Option<&Cell>, a: Option<&Cell>) -> bool {
    match (b, a) {
        (Some(b), Some(a)) => match (&b.formula, &a.formula) {
            (Some(x), Some(y)) => x.text != y.text,
            (None, None) => b.value != a.value,
            _ => true,
        },
        (Some(b), None) => has_content(b),
        _ => false,
    }
}

/// Cells as A1 ranges: runs along each row, then runs with the same columns on following rows.
pub(crate) fn ranges(cells: &mut [CellRef]) -> Vec<String> {
    cells.sort_unstable_by_key(|c| (c.row, c.col));
    let mut runs: Vec<RangeRef> = Vec::new();
    for c in cells.iter() {
        match runs.last_mut() {
            Some(r) if r.end.row == c.row && r.end.col.checked_add(1) == Some(c.col) => r.end.col = c.col,
            _ => runs.push(RangeRef::cell(*c)),
        }
    }
    let mut merged: Vec<RangeRef> = Vec::new();
    for r in runs {
        // The block above with the same columns that ends on the row before.
        match merged
            .iter_mut()
            .rev()
            .take(64)
            .find(|m| m.start.col == r.start.col && m.end.col == r.end.col && m.end.row.checked_add(1) == Some(r.start.row))
        {
            Some(m) => m.end.row = r.end.row,
            None => merged.push(r),
        }
    }
    merged.iter().take(MAX_LISTED_RANGES).map(RangeRef::a1).collect()
}

// ---------------------------------------------------------------- run

/// A macro that passed its checks and is about to run.
pub struct Begun {
    pub mac: Macro,
    pub prepared: Prepared,
    pub rule: OnConflict,
    pub time_limit_ms: f64,
    pub inputs: Json,
}

/// The checks before a run, on the active workbook: the macro exists, the policy allows it, the
/// person approved it if it needs that, and its inputs are right.
pub fn begin(s: &mut Session, name: &str, p: &Json) -> Result<Begun> {
    let d = s.doc()?;
    let (doc, trust) = (d.uid, d.trust);
    let m = super::find(&d.wb, name)
        .and_then(|i| d.wb.macros.get(i))
        .cloned()
        .ok_or_else(|| EngineError::Other(format!("There is no macro called '{name}'.")))?;
    let asks = gate(&m, trust)?;
    approve(s, &m, doc, &asks, p.get("approve").and_then(Json::as_bool).unwrap_or(false))?;
    let prepared = prepare(s, &m, p.get("inputs"))?;
    let rule = conflict_rule(p, &m)?;
    let time_limit_ms = time_limit(p);
    let inputs = prepared.inputs_json(&s.doc()?.wb);
    Ok(Begun { mac: m, prepared, rule, time_limit_ms, inputs })
}

/// The output rule, inside the run's undo group: the machine for the run. Unless the macro may
/// overwrite, the machine checks each step as it runs and stops the run at one that writes over
/// data the workbook held when the run started (asking under `ask`, failing otherwise); the run's
/// undo group then rolls back. Shifting needs to know where the macro writes before it runs, so
/// `shiftDown` and `shiftRight` stop the run as `fail` does.
pub fn make_way(s: &mut Session, b: &Begun) -> Result<Machine> {
    let mut machine = Machine::new(&b.prepared);
    machine.time_limit_ms = b.time_limit_ms;
    if b.rule != OnConflict::Overwrite {
        let start = s.docs.iter().find(|d| d.uid == b.prepared.doc).ok_or(EngineError::NoDocument)?.wb.clone();
        machine.guard = Some(Guard::new(start, b.rule == OnConflict::Ask));
    }
    Ok(machine)
}

/// Runs macro `name` on the active workbook: `{inputs?, onConflict?, overwrite?, approve?,
/// timeLimit?}` (see `macro.run`).
pub fn run(s: &mut Session, name: &str, p: &Json) -> Result<Json> {
    let b = begin(s, name, p)?;
    let label = format!("Run Macro: {}", b.mac.name);
    s.grouped(&label, |s| {
        let mut machine = make_way(s, &b)?;
        let name = &b.mac.name;
        machine.run_to_end(s).map_err(|e| e.into_engine(name))?;
        match machine.stopped {
            Some((msg, true)) => Err(EngineError::Other(format!("Macro '{name}' stopped: {msg} Nothing was changed."))),
            stopped => {
                let mut out = json!({"macro": name, "ran": machine.executed, "inputs": b.inputs});
                if let Some((msg, _)) = stopped {
                    out["stopped"] = json!(msg);
                }
                Ok(out)
            }
        }
    })
}

/// The run's `timeLimit` (seconds) in milliseconds, or the default.
fn time_limit(p: &Json) -> f64 {
    p.get("timeLimit").and_then(Json::as_f64).map(|t| (t * 1000.0).clamp(1.0, 600_000.0)).unwrap_or(DEFAULT_TIME_LIMIT_MS)
}

/// The conflict rule for this run: `onConflict`, else the macro's own. `overwrite: true` is the
/// answer to the question, so it turns `ask` into `overwrite` whichever way `ask` was given.
fn conflict_rule(p: &Json, m: &Macro) -> Result<OnConflict> {
    let rule = match p.get("onConflict").and_then(Json::as_str) {
        Some(t) => OnConflict::parse(t)
            .ok_or_else(|| EngineError::Other(format!("onConflict must be ask, fail, overwrite, shiftDown or shiftRight, not '{t}'.")))?,
        None => m.output.on_conflict,
    };
    if rule == OnConflict::Ask && p.get("overwrite").and_then(Json::as_bool) == Some(true) {
        return Ok(OnConflict::Overwrite);
    }
    Ok(rule)
}
