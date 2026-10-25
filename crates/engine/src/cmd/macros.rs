//! Macros: typed command lists that run as one undo step. See [`crate::macros`] for the model,
//! tokens, runner and policy. Automate scripts (`automate.*`) are a separate, older feature.

use serde_json::Value as Json;

use super::*;

pub fn specs() -> Vec<CommandSpec> {
    vec![cmd!(
        "macro.run",
        "Run Macro",
        [],
        None,
        "{name, inputs?: {Input: \"B2:D9\"} | \"B2:D9\", onConflict?: ask|fail|overwrite|shiftDown|shiftRight (a shift stops the run as fail does), overwrite?: bool, approve?: bool, timeLimit?: seconds} runs as one undo step; any error rolls it all back → {macro, ran, inputs, stopped?}",
        has_doc,
        run
    )]
}

fn name_param<'a>(cmd: &str, p: &'a Json) -> Result<&'a str> {
    str_param(p, "name").filter(|n| !n.trim().is_empty()).ok_or_else(|| bad(cmd, "missing `name`"))
}

fn run(s: &mut Session, p: &Json) -> Result<Json> {
    let name = name_param("macro.run", p)?;
    crate::macros::run::run(s, name, p)
}
