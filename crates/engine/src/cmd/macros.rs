//! Macros: typed command lists that run as one undo step. See [`crate::macros`] for the model,
//! tokens, runner, debugger and policy. Automate scripts (`automate.*`) are a separate, older feature.

use serde_json::Value as Json;

use super::*;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "macro.run",
            "Run Macro",
            [],
            None,
            "{name, inputs?: {Input: \"B2:D9\"} | \"B2:D9\", onConflict?: ask|fail|overwrite|shiftDown|shiftRight, overwrite?: bool, approve?: bool, timeLimit?: seconds} runs as one undo step; any error rolls it all back → {macro, ran, inputs, stopped?, shifted?}",
            has_doc,
            run
        ),
        cmd!(
            query "macro.check",
            "Check Macro",
            [],
            None,
            "{name, inputs?, onConflict?, overwrite?, timeLimit?: seconds} validates the inputs and dry-runs the macro on the workbook (then puts everything back) → {ok (macro.run with these parameters would go ahead without a question), errors?, needsApproval?, onConflict, inputs, output?, conflicts, footprint}",
            has_doc,
            check
        ),
        cmd!(
            "macro.debug",
            "Step Into",
            [],
            None,
            "{action: start|step|continue|runTo|stop|state, name, inputs?, onConflict?, overwrite?, approve?, timeLimit?: seconds per action (start, as macro.run), step: path (runTo), keep?: bool (stop), breakpoints?: [\"3\", \"2.1\"] (steps of the macro), breakpointsOnly?: bool (the list replaces the steps' own breakpoints)} debugs a macro one step at a time; stop keeps the changes as one undo step or rolls them back. Runs on its own, never as a step of a script or macro. Until it finishes or stops, its workbook runs only macro.debug, queries, selection and sheet changes, Go To, Find, Copy, zoom, Switch Window, Close and Stop Action (other workbooks stay editable) → {state: paused|finished|stopped|failed, current: {step, kind, command?, params?, iteration?}, lastParams, executed, changed (at most 500), changedCells, atBreakpoint?, shifted?, wroteFiles?, message?, error?}",
            has_doc,
            |s, p| crate::macros::debug::debug(s, p)
        ),
    ]
}

fn name_param<'a>(cmd: &str, p: &'a Json) -> Result<&'a str> {
    str_param(p, "name").filter(|n| !n.trim().is_empty()).ok_or_else(|| bad(cmd, "missing `name`"))
}

fn run(s: &mut Session, p: &Json) -> Result<Json> {
    let name = name_param("macro.run", p)?;
    crate::macros::run::run(s, name, p)
}

fn check(s: &mut Session, p: &Json) -> Result<Json> {
    let name = name_param("macro.check", p)?;
    crate::macros::run::check(s, name, p)
}
