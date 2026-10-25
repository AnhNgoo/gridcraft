//! Macros: running, input checks, the footprint dry run, the policy and the limits.

use std::collections::BTreeSet;
use std::sync::Arc;

use gridcraft_core::CellRef;
use serde_json::{Value as Json, json};

use super::{s, v};
use crate::{EngineError, Session};

fn undo_labels(s: &Session) -> Vec<String> {
    s.doc().unwrap().undo.iter().map(|e| e.label.clone()).collect()
}

fn set(s: &mut Session, cell: &str, input: &str) {
    s.execute("cell.set", json!({"cell": cell, "input": input})).unwrap();
}

/// Puts a macro into the active workbook (replacing one with the same name).
pub(super) fn put(s: &mut Session, m: Json) {
    let m = crate::macros::parse_macro(&m).unwrap();
    let d = s.doc_mut().unwrap();
    let mut wb = (*d.wb).clone();
    wb.macros.retain(|x| !x.name.eq_ignore_ascii_case(&m.name));
    wb.macros.push(m);
    d.wb = Arc::new(wb);
    d.saved = d.wb.clone();
}

/// Names in B2:B5 and amounts in C2:C5 under a header row.
fn data(s: &mut Session) {
    for (cell, input) in [("B2", "Name"), ("C2", "Amount"), ("B3", "Ann"), ("C3", "120"), ("B4", "Bo"), ("C4", "80"), ("B5", "Cy"), ("C5", "200")] {
        set(s, cell, input);
    }
}

/// A macro that writes a label and a total next to its input and bolds the header.
fn totals() -> Json {
    json!({
        "name": "Totals",
        "inputs": [{"name": "Input", "header": true, "cols": [2, 2], "colTypes": ["text", "number"]}],
        "output": {"anchor": "Input.right(2).cell(1,2)"},
        "steps": [
            {"command": "cell.set", "params": {"cell": "Output", "input": "Total"}},
            {"command": "cell.set", "params": {"cell": "Output.offset(1,0)", "input": "=SUM({{Input.data.col(2)}})"}},
            {"command": "home.bold", "params": {"range": "Input.row(1)"}}
        ]
    })
}

fn bold(s: &Session, a: &str) -> bool {
    let d = s.doc().unwrap();
    let sh = d.wb.active().unwrap();
    d.wb.styles.get(sh.style_id(gridcraft_core::CellRef::parse(a).unwrap())).font.bold
}

#[test]
fn a_run_resolves_tokens_and_is_one_undo_step() {
    let mut s = s();
    data(&mut s);
    put(&mut s, totals());
    let before = undo_labels(&s);
    let r = s.execute("macro.run", json!({"name": "totals", "inputs": {"Input": "B2:C5"}})).unwrap();
    assert_eq!(r["ran"], 3);
    assert_eq!(r["inputs"], json!({"Input": "B2:C5"}));
    assert_eq!(v(&s, "E2").display(), "Total");
    assert_eq!(v(&s, "E3").display(), "400");
    assert_eq!(s.doc().unwrap().wb.active().unwrap().input_text(gridcraft_core::CellRef::parse("E3").unwrap()), "=SUM(C3:C5)");
    assert!(bold(&s, "B2") && bold(&s, "C2"));
    let after = undo_labels(&s);
    assert_eq!(after.len(), before.len() + 1);
    assert_eq!(after.last().unwrap(), "Run Macro: Totals");
    s.execute("edit.undo", json!({})).unwrap();
    assert!(v(&s, "E2").is_empty() && v(&s, "E3").is_empty() && !bold(&s, "B2"));
    assert_eq!(undo_labels(&s), before);
    // The selection is the default input.
    s.execute("selection.set", json!({"range": "B2:C5"})).unwrap();
    s.execute("macro.run", json!({"name": "Totals"})).unwrap();
    assert_eq!(v(&s, "E3").display(), "400");
}

#[test]
fn inputs_are_checked_with_messages_that_name_the_cells() {
    let mut s = s();
    data(&mut s);
    set(&mut s, "C4", "eighty");
    set(&mut s, "C5", "TRUE");
    put(&mut s, totals());
    let start = s.doc().unwrap().wb.clone();
    let err = s.execute("macro.run", json!({"name": "Totals", "inputs": "B2:C5"})).unwrap_err().to_string();
    assert!(err.contains("Input column 2 (Amount) must hold numbers; C4 holds text, C5 holds TRUE or FALSE."), "{err}");
    assert!(Arc::ptr_eq(&s.doc().unwrap().wb, &start), "nothing ran");

    // Shape: width, rows, a single cell, blanks, dates.
    let mut m = totals();
    m["inputs"][0] = json!({"name": "Input", "header": true, "rows": [1, 2], "cols": [3, 3], "nonEmpty": true, "colTypes": ["any", "date"]});
    put(&mut s, m);
    set(&mut s, "B4", "");
    let err = s.execute("macro.run", json!({"name": "Totals", "inputs": "B2:C5"})).unwrap_err().to_string();
    for needle in [
        "Input needs at most 2 data rows; B2:C5 has 3.",
        "Input needs exactly 3 columns; B2:C5 has 2.",
        "Input column 2 (Amount) must hold dates; C3 holds a number that isn't a date, C4 holds text, C5 holds TRUE or FALSE.",
        "Input must have no blank cells; B4 is blank.",
    ] {
        assert!(err.contains(needle), "missing `{needle}` in {err}");
    }
    let mut m = totals();
    m["inputs"][0] = json!({"name": "Input", "kind": "cell"});
    put(&mut s, m);
    let err = s.execute("macro.run", json!({"name": "Totals", "inputs": "B2:C5"})).unwrap_err().to_string();
    assert!(err.contains("Input must be a single cell; B2:C5 is 8 cells."), "{err}");
    // A second input with no range, and a bad reference.
    put(&mut s, json!({"name": "Two", "inputs": [{"name": "Input"}, {"name": "Rates"}], "steps": []}));
    let err = s.execute("macro.run", json!({"name": "Two"})).unwrap_err().to_string();
    assert!(err.contains("Rates needs a range"), "{err}");
    let err = s.execute("macro.run", json!({"name": "Two", "inputs": {"Rates": "Nowhere!A1"}})).unwrap_err().to_string();
    assert!(err.contains("Rates: There is no sheet named 'Nowhere'."), "{err}");
    // Whole columns shrink to the cells in use.
    put(
        &mut s,
        json!({"name": "Col", "inputs": [{"name": "Input", "rows": [1, 10]}], "steps": [{"command": "cell.set", "params": {"cell": "Input.below", "input": "end"}}], "output": {"onConflict": "overwrite"}}),
    );
    s.execute("macro.run", json!({"name": "Col", "inputs": "B:B"})).unwrap();
    assert_eq!(v(&s, "B6").display(), "end");
    assert!(s.execute("macro.run", json!({"name": "nope"})).unwrap_err().to_string().contains("no macro called 'nope'"));
}

#[test]
fn an_error_rolls_everything_back_and_names_the_step() {
    let mut s = s();
    data(&mut s);
    let mut m = totals();
    m["output"]["onConflict"] = json!("overwrite");
    m["steps"][2] = json!({"type": "repeat", "over": "Input.data", "body": [
        {"command": "home.italic", "params": {"range": "Row"}},
        {"command": "cell.set", "params": {"cell": "Row.cell(1,9)", "input": "=1/"}}
    ]});
    put(&mut s, m);
    let (before, undo) = (s.doc().unwrap().wb.clone(), undo_labels(&s));
    let err = s.execute("macro.run", json!({"name": "Totals", "inputs": "B2:C5"})).unwrap_err().to_string();
    assert!(err.starts_with("Macro 'Totals' stopped. Step 3.2 (cell.set):"), "{err}");
    assert!(err.ends_with("Nothing was changed."), "{err}");
    assert!(Arc::ptr_eq(&s.doc().unwrap().wb, &before));
    assert_eq!(undo_labels(&s), undo);
    assert!(s.doc().unwrap().redo.is_empty());
}

#[test]
fn conflicts_ask_fail_or_overwrite() {
    let mut s = s();
    data(&mut s);
    set(&mut s, "E3", "keep me");
    // A formula that reads what the macro writes: its value changes, but it isn't written.
    set(&mut s, "H1", "=E3");
    put(&mut s, totals());
    let start = s.doc().unwrap().wb.clone();
    let r = s.execute("macro.run", json!({"name": "Totals", "inputs": "B2:C5"}));
    let Err(EngineError::Confirm(q)) = r else { panic!("{r:?}") };
    assert!(q.contains("would write over cells that hold data (E3 on Sheet1)"), "{q}");
    assert!(Arc::ptr_eq(&s.doc().unwrap().wb, &start), "programmatic runs default to failing");
    let err = s.execute("macro.run", json!({"name": "Totals", "inputs": "B2:C5", "onConflict": "fail"})).unwrap_err().to_string();
    assert!(err.contains("Nothing was changed"), "{err}");
    assert!(s.execute("macro.run", json!({"name": "Totals", "onConflict": "sideways"})).is_err());

    // macro.check shows the footprint and the conflict, and changes nothing.
    let journal = s.journal.len();
    let sel = s.doc().unwrap().selection.clone();
    let c = s.execute("macro.check", json!({"name": "Totals", "inputs": "B2:C5"})).unwrap();
    // Under ask the run would ask first, so it wouldn't go ahead as it is.
    assert_eq!((&c["ok"], &c["onConflict"]), (&json!(false), &json!("ask")));
    assert_eq!(c["errors"], json!(["It would write over cells that hold data (E3 on Sheet1); the run asks first."]));
    assert_eq!(c["conflicts"], 1);
    assert_eq!(c["output"], "E2");
    assert_eq!(c["footprint"]["sheets"][0]["cells"], json!(["E2:E3"]));
    assert_eq!(c["footprint"]["sheets"][0]["conflicts"], json!(["E3"]));
    assert!(Arc::ptr_eq(&s.doc().unwrap().wb, &start));
    assert_eq!((s.journal.len(), &s.doc().unwrap().selection), (journal, &sel));
    // With the answer given, or with the rule overwrite, it would.
    for p in [json!({"overwrite": true}), json!({"onConflict": "overwrite"})] {
        let mut p = p;
        (p["name"], p["inputs"]) = (json!("Totals"), json!("B2:C5"));
        let c = s.execute("macro.check", p.clone()).unwrap();
        assert_eq!((&c["ok"], &c["onConflict"], c.get("errors")), (&json!(true), &json!("overwrite"), None), "{p}");
    }

    // The answer to the question overwrites.
    s.execute("macro.run", json!({"name": "Totals", "inputs": "B2:C5", "overwrite": true})).unwrap();
    assert_eq!(v(&s, "E3").display(), "400");
    assert_eq!(v(&s, "H1").display(), "400");
}

/// The value of `cell` on the sheet called `sheet`.
fn on(s: &Session, sheet: &str, cell: &str) -> String {
    let wb = &s.doc().unwrap().wb;
    wb.sheet(wb.sheet_index(sheet).unwrap()).unwrap().value(CellRef::parse(cell).unwrap()).display()
}

#[test]
fn inputs_and_the_output_follow_their_cells_when_a_step_moves_them() {
    // A sheet inserted in front: the input is still on Sheet1, not on whatever sheet is first now.
    let mut s = s();
    let steps = json!([
        {"command": "home.insertSheet", "params": {"name": "Summary", "before": 0}},
        {"command": "cell.set", "params": {"cell": "Input", "input": "x"}}
    ]);
    put(
        &mut s,
        json!({"name": "Front", "inputs": [{"name": "Input", "kind": "cell"}], "output": {"onConflict": "overwrite"}, "steps": steps.clone()}),
    );
    s.execute("macro.run", json!({"name": "Front", "inputs": "Sheet1!B2"})).unwrap();
    assert_eq!((on(&s, "Sheet1", "B2"), on(&s, "Summary", "B2")), ("x".into(), "".into()));

    // Data in the input: the step writes into its own input, which isn't data written over, on
    // the input's sheet wherever it moved.
    let mut s = super::s();
    set(&mut s, "B2", "held");
    put(&mut s, json!({"name": "Front", "inputs": [{"name": "Input", "kind": "cell"}], "output": {"onConflict": "fail"}, "steps": steps}));
    s.execute("macro.run", json!({"name": "Front", "inputs": "Sheet1!B2"})).unwrap();
    assert_eq!(on(&s, "Sheet1", "B2"), "x");

    // Rows inserted above: the input and the output move down with their cells.
    let mut s = super::s();
    set(&mut s, "B5", "in");
    let steps = json!([
        {"command": "home.insertRows", "params": {"rows": "2:3"}},
        {"command": "cell.set", "params": {"cell": "Input", "input": "x"}},
        {"command": "cell.set", "params": {"cell": "Output", "input": "y"}}
    ]);
    put(
        &mut s,
        json!({"name": "Down", "inputs": [{"name": "Input", "kind": "cell"}], "output": {"anchor": "D5", "onConflict": "overwrite"}, "steps": steps}),
    );
    s.execute("macro.run", json!({"name": "Down", "inputs": "B5"})).unwrap();
    assert_eq!((v(&s, "B7").display(), v(&s, "D7").display()), ("x".into(), "y".into()));
    assert_eq!((v(&s, "B5").display(), v(&s, "D5").display()), ("".into(), "".into()));

    // A step that deletes an input's cells stops the run.
    let mut s = super::s();
    set(&mut s, "B5", "in");
    let steps = json!([{"command": "home.deleteRows", "params": {"rows": "5"}}, {"command": "cell.set", "params": {"cell": "Input", "input": "x"}}]);
    put(&mut s, json!({"name": "Gone", "inputs": [{"name": "Input", "kind": "cell"}], "output": {"onConflict": "overwrite"}, "steps": steps}));
    let start = s.doc().unwrap().wb.clone();
    let err = s.execute("macro.run", json!({"name": "Gone", "inputs": "B5"})).unwrap_err().to_string();
    assert!(err.contains("Step 1 (home.deleteRows): It deleted the cells of Input."), "{err}");
    assert!(Arc::ptr_eq(&s.doc().unwrap().wb, &start));

    // A step inside a Repeat that changes the size of the range it goes over stops the run.
    let mut s = super::s();
    let body = json!([{"command": "home.insertRows", "params": {"rows": "2"}}]);
    put(&mut s, json!({"name": "Grow", "output": {"onConflict": "overwrite"}, "steps": [{"type": "repeat", "over": "A1:A3", "body": body}]}));
    let err = s.execute("macro.run", json!({"name": "Grow"})).unwrap_err().to_string();
    assert!(err.contains("Step 1.1 (home.insertRows): It changed the size of the range Repeat 1 goes over."), "{err}");
}

#[test]
fn shifting_makes_room_and_is_checked_again() {
    for (rule, moved) in [("shiftDown", ["E4", "E5"]), ("shiftRight", ["F2", "F3"])] {
        let mut s = s();
        data(&mut s);
        set(&mut s, "E2", "old 1");
        set(&mut s, "E3", "old 2");
        put(&mut s, totals());
        let r = s.execute("macro.run", json!({"name": "Totals", "inputs": "B2:C5", "onConflict": rule})).unwrap();
        assert_eq!(r["shifted"], rule);
        assert_eq!((v(&s, "E2").display(), v(&s, "E3").display()), ("Total".into(), "400".into()), "{rule}");
        assert_eq!((v(&s, moved[0]).display(), v(&s, moved[1]).display()), ("old 1".into(), "old 2".into()), "{rule}");
        // The shift and the run are one undo step.
        s.execute("edit.undo", json!({})).unwrap();
        assert_eq!(v(&s, "E2").display(), "old 1", "{rule}");
        assert!(v(&s, moved[1]).is_empty(), "{rule}");
    }
    // A cell written with what it already holds isn't a conflict, but it moves with the others,
    // so rows stay together.
    let mut s = super::s();
    for (cell, input) in [("F2", "x"), ("G2", "b"), ("F3", "f3"), ("G3", "g3")] {
        set(&mut s, cell, input);
    }
    put(
        &mut s,
        json!({"name": "Pair", "output": {"onConflict": "shiftDown"}, "steps": [
            {"command": "cell.set", "params": {"cell": "F2", "input": "a"}},
            {"command": "cell.set", "params": {"cell": "G2", "input": "b"}}
        ]}),
    );
    let c = s.execute("macro.check", json!({"name": "Pair"})).unwrap();
    assert_eq!((c["conflicts"].as_u64(), &c["footprint"]["sheets"][0]["cells"]), (Some(1), &json!(["F2:G2"])));
    s.execute("macro.run", json!({"name": "Pair"})).unwrap();
    let row = |s: &Session, r: u32| (v(s, &format!("F{r}")).display(), v(s, &format!("G{r}")).display());
    assert_eq!(row(&s, 2), ("a".into(), "b".into()));
    assert_eq!(row(&s, 3), ("x".into(), "b".into()));
    assert_eq!(row(&s, 4), ("f3".into(), "g3".into()));
    // An anchor that moves with the shifted data still lands on it: the second check refuses.
    let mut s = super::s();
    data(&mut s);
    set(&mut s, "E2", "old");
    s.execute("formulas.defineName", json!({"name": "Target", "refersTo": "=Sheet1!$E$2"})).unwrap();
    put(
        &mut s,
        json!({"name": "Named", "steps": [{"command": "cell.set", "params": {"cell": "Name:Target", "input": "new"}}], "output": {"onConflict": "shiftDown"}}),
    );
    let start = s.doc().unwrap().wb.clone();
    let err = s.execute("macro.run", json!({"name": "Named"})).unwrap_err().to_string();
    assert!(err.contains("shifting the data didn't make room (E3 on Sheet1)"), "{err}");
    assert!(Arc::ptr_eq(&s.doc().unwrap().wb, &start));
    // The check shifts and checks again as the run does, and puts everything back, the document's
    // revision included.
    let journal = s.journal.len();
    let revision = s.doc().unwrap().revision;
    let c = s.execute("macro.check", json!({"name": "Named"})).unwrap();
    assert_eq!((&c["ok"], &c["errors"]), (&json!(false), &json!(["Shifting the data doesn't make room (E3 on Sheet1)."])), "{c}");
    assert!(Arc::ptr_eq(&s.doc().unwrap().wb, &start));
    assert_eq!((s.journal.len(), s.doc().unwrap().revision), (journal, revision));
    // Where the shift does make room, the check says the run would go ahead.
    let mut s = super::s();
    data(&mut s);
    set(&mut s, "E2", "old 1");
    put(&mut s, totals());
    let c = s.execute("macro.check", json!({"name": "Totals", "inputs": "B2:C5", "onConflict": "shiftDown"})).unwrap();
    assert_eq!((&c["ok"], c.get("errors")), (&json!(true), None), "{c}");
}

#[test]
fn stop_action_stops_a_macro_being_debugged() {
    // Stop Action rolls back the open undo groups, the debugger's among them; the run ends with
    // its group, and the answer says so.
    let mut s = s();
    data(&mut s);
    put(&mut s, totals());
    let (start, undo) = (s.doc().unwrap().wb.clone(), undo_labels(&s));
    s.execute("macro.debug", json!({"action": "start", "name": "Totals", "inputs": "B2:C5", "breakpoints": ["1"]})).unwrap();
    s.execute("macro.debug", json!({"action": "step"})).unwrap();
    assert_eq!(v(&s, "E2").display(), "Total");
    let r = s.execute("automate.stopAction", json!({})).unwrap();
    assert_eq!((r["stopped"].as_bool(), r["macro"].as_str()), (Some(true), Some("Totals")), "{r}");
    assert!(r["message"].as_str().unwrap().contains("was stopped and rolled back"), "{r}");
    assert!(s.macro_debug.is_none(), "the debugger ended with its group");
    assert!(Arc::ptr_eq(&s.doc().unwrap().wb, &start));
    assert_eq!(undo_labels(&s), undo);
    assert!(!s.in_group(s.doc().unwrap().uid));
    set(&mut s, "A1", "free");
    assert!(s.execute("macro.debug", json!({"action": "step"})).unwrap_err().to_string().contains("No macro is being debugged"));
    // Stop Action on another workbook leaves the debugger alone.
    s.execute("macro.debug", json!({"action": "start", "name": "Totals", "inputs": "B2:C5"})).unwrap();
    s.new_workbook();
    let r = s.execute("automate.stopAction", json!({})).unwrap();
    assert_eq!(r, json!({"stopped": false}));
    assert!(s.macro_debug.is_some());
}

#[test]
fn the_debugger_runs_on_its_own_and_is_never_recorded() {
    // As a step of a script, the debugger would outlive the script's rollback of its groups.
    let mut s = s();
    stepper(&mut s);
    let start = s.doc().unwrap().wb.clone();
    let commands = json!([
        {"command": "macro.debug", "params": {"action": "start", "name": "Walk", "inputs": "B2:C5"}},
        {"command": "cell.set", "params": {"cell": "A1", "input": "after"}}
    ]);
    s.execute("automate.newScript", json!({"name": "S", "commands": commands})).unwrap();
    let after_script = s.doc().unwrap().wb.clone();
    let e = s.execute("automate.runScript", json!({"name": "S"})).unwrap_err().to_string();
    assert!(e.contains("runs on its own"), "{e}");
    assert!(s.macro_debug.is_none());
    assert!(Arc::ptr_eq(&s.doc().unwrap().wb, &after_script));
    assert!(!s.in_group(s.doc().unwrap().uid));
    set(&mut s, "A1", "free");
    assert!(!Arc::ptr_eq(&s.doc().unwrap().wb, &start));
    // Neither recorder keeps it.
    s.start_recording();
    s.execute("automate.recordActions", json!({})).unwrap();
    debug(&mut s, json!({"action": "start", "name": "Walk", "inputs": "B2:C5"}));
    debug(&mut s, json!({"action": "stop"}));
    let saved = s.execute("automate.recordActions", json!({"stop": true, "name": "R"})).unwrap();
    assert_eq!(saved["steps"], 0, "{saved}");
    let recorded = s.stop_recording().unwrap();
    assert!(!recorded.steps.iter().any(|st| st.id == "macro.debug"), "{:?}", recorded.steps);
}

#[test]
fn a_debugger_whose_group_was_closed_elsewhere_locks_nothing() {
    let mut s = s();
    stepper(&mut s);
    let uid = s.doc().unwrap().uid;
    debug(&mut s, json!({"action": "start", "name": "Walk", "inputs": "B2:C5"}));
    debug(&mut s, json!({"action": "step"}));
    // As if a command had rolled back the open groups without telling the debugger.
    assert!(s.abort_groups(uid));
    set(&mut s, "A1", "free");
    assert!(s.macro_debug.is_none(), "the lock let the edit through and ended the debugger");
    // A debugger action says the run had ended; nothing is reported as kept or rolled back.
    debug(&mut s, json!({"action": "start", "name": "Walk", "inputs": "B2:C5"}));
    debug(&mut s, json!({"action": "step"}));
    assert!(s.abort_groups(uid));
    let e = s.execute("macro.debug", json!({"action": "stop", "keep": true})).unwrap_err().to_string();
    assert!(e.contains("no longer being debugged"), "{e}");
    assert!(s.macro_debug.is_none());
    // Starting again works at once.
    debug(&mut s, json!({"action": "start", "name": "Walk", "inputs": "B2:C5"}));
    assert!(s.abort_groups(uid));
    assert_eq!(debug(&mut s, json!({"action": "start", "name": "Walk", "inputs": "B2:C5"}))["state"], "paused");
}

/// Whether `a` on sheet `sheet` is bold.
fn bold_on(s: &Session, sheet: &str, a: &str) -> bool {
    let d = s.doc().unwrap();
    let sh = d.wb.sheet(d.wb.sheet_index(sheet).unwrap()).unwrap();
    d.wb.styles.get(sh.style_id(CellRef::parse(a).unwrap())).font.bold
}

#[test]
fn between_steps_only_looking_around_runs_and_the_run_continues_where_it_was() {
    let mut s = s();
    s.execute("home.insertSheet", json!({"name": "Other"})).unwrap();
    s.execute("sheet.activate", json!({"sheet": 0})).unwrap();
    put(
        &mut s,
        json!({"name": "Sel", "output": {"onConflict": "overwrite"}, "steps": [
            {"command": "selection.set", "params": {"range": "C3"}},
            {"command": "home.bold", "params": {}}
        ]}),
    );
    debug(&mut s, json!({"action": "start", "name": "Sel"}));
    debug(&mut s, json!({"action": "step"}));
    // Looking around runs: another sheet, another selection, Go To, Find, Copy, zoom.
    s.execute("sheet.activate", json!({"sheet": "Other"})).unwrap();
    s.execute("selection.set", json!({"range": "A1"})).unwrap();
    s.execute("edit.goTo", json!({"reference": "B2"})).unwrap();
    s.execute("edit.copy", json!({})).unwrap();
    s.execute("view.zoom", json!({"percent": 150})).unwrap();
    assert!(s.execute("selection.stats", json!({})).is_ok(), "queries run");
    // Anything else waits, undoable or not, saving included.
    let path = std::env::temp_dir().join(format!("gc-macro-debug-save-{}.xlsx", std::process::id()));
    let busy = |r: Result<Json, EngineError>| matches!(&r, Err(EngineError::Disabled(_, why)) if why == crate::macros::debug::BUSY);
    assert!(busy(s.execute("view.customViews", json!({"save": "Mine"}))));
    assert!(busy(s.execute("view.pageLayout", json!({}))));
    assert!(busy(s.execute("file.saveAs", json!({"path": path.to_string_lossy()}))));
    assert!(busy(s.execute("file.save", json!({}))));
    assert!(busy(s.execute("automate.recordActions", json!({}))));
    assert!(busy(s.execute("cell.set", json!({"cell": "A1", "input": "x"}))));
    // A check would dry-run the macro's steps, which the lock refuses: it waits too.
    assert!(busy(s.execute("macro.check", json!({"name": "Sel"}))));
    assert!(!path.exists(), "nothing was saved");
    // The next step runs on the run's sheet and selection, as macro.run would.
    let st = debug(&mut s, json!({"action": "step"}));
    assert_eq!(st["state"], "finished", "{st}");
    assert!(bold_on(&s, "Sheet1", "C3"));
    assert!(!bold_on(&s, "Other", "B2") && !bold_on(&s, "Other", "A1"));
    let d = s.doc().unwrap();
    assert_eq!((d.wb.active_sheet, d.selection.active), (0, CellRef::parse("C3").unwrap()));
}

#[test]
fn a_panic_inside_a_step_is_that_steps_error_and_the_run_goes_on() {
    crate::register_test_command("test.debugInnerPanics", |_, _| panic!("inner"));
    // A step whose command catches a nested command's panic as an error and goes on editing.
    crate::register_test_command("test.debugSwallowsPanic", |s, _| {
        let _ = s.execute("test.debugInnerPanics", json!({}));
        s.execute("cell.set", json!({"cell": "B1", "input": "after"}))
    });
    let mut s = s();
    put(
        &mut s,
        json!({"name": "M", "output": {"onConflict": "overwrite"}, "steps": [
            {"command": "cell.set", "params": {"cell": "A1", "input": "x"}},
            {"command": "test.debugSwallowsPanic", "params": {}}
        ]}),
    );
    let undo = undo_labels(&s);
    debug(&mut s, json!({"action": "start", "name": "M"}));
    debug(&mut s, json!({"action": "step"}));
    let st = debug(&mut s, json!({"action": "step"}));
    assert_eq!(st["state"], "finished", "{st}");
    assert_eq!((v(&s, "A1").display(), v(&s, "B1").display()), ("x".into(), "after".into()));
    let after = undo_labels(&s);
    assert_eq!((after.len(), after.last().map(String::as_str)), (undo.len() + 1, Some("Run Macro: M")), "{after:?}");
    s.execute("edit.undo", json!({})).unwrap();
    assert!(v(&s, "A1").is_empty() && v(&s, "B1").is_empty());
}

#[test]
fn debugger_parameters_are_checked_before_anything_changes() {
    let mut s = s();
    stepper(&mut s);
    let start = |bps: Json| json!({"action": "start", "name": "Walk", "inputs": "B2:C5", "breakpoints": bps});
    let e = s.execute("macro.debug", start(json!(["9"]))).unwrap_err().to_string();
    assert!(e.contains("has no step 9 for a breakpoint"), "{e}");
    let many: Vec<String> = (0..10_001).map(|_| "1".to_string()).collect();
    let e = s.execute("macro.debug", start(json!(many))).unwrap_err().to_string();
    assert!(e.contains("at most 10000"), "{e}");
    assert!(s.macro_debug.is_none() && !s.in_group(s.doc().unwrap().uid));
    debug(&mut s, start(json!([])));
    // `keep` is true or false: "true" would have read as false and rolled the run back.
    assert!(s.execute("macro.debug", json!({"action": "stop", "keep": "true"})).is_err());
    assert!(s.execute("macro.debug", json!({"action": "step", "breakpointsOnly": "yes"})).is_err());
    assert!(s.execute("macro.debug", json!({"action": 5})).is_err());
    // An unknown action or step path changes nothing, breakpoints included.
    assert!(s.execute("macro.debug", json!({"action": "leap", "breakpoints": ["2"]})).is_err());
    assert!(s.execute("macro.debug", json!({"action": "step", "breakpoints": ["2.x"]})).is_err());
    assert!(s.execute("macro.debug", json!({"action": "runTo", "step": "9", "breakpoints": ["2"]})).is_err());
    let st = debug(&mut s, json!({"action": "state"}));
    assert_eq!((st["current"]["step"].as_str(), &st["breakpoints"]), (Some("1"), &json!([])), "{st}");
    // `breakpointsOnly` alone applies to the list the run has (none here): the flag on 3.1 no
    // longer stops it.
    let st = debug(&mut s, json!({"action": "continue", "breakpointsOnly": true}));
    assert_eq!(st["state"], "finished", "{st}");
}

#[test]
fn the_debuggers_answers_count_its_own_steps_and_say_what_stays() {
    // `executed` counts the run's steps, not the footprint's dry run before it.
    let mut s = s();
    data(&mut s);
    put(&mut s, totals());
    let st = debug(&mut s, json!({"action": "start", "name": "Totals", "inputs": "B2:C5"}));
    assert_eq!(st["executed"], 0, "{st}");
    assert_eq!(debug(&mut s, json!({"action": "step"}))["executed"], 1);
    let st = debug(&mut s, json!({"action": "continue"}));
    assert_eq!((st["state"].as_str(), st["executed"].as_u64()), (Some("finished"), Some(3)), "{st}");
    // A shift at the start is reported, as macro.run reports it.
    let mut s = super::s();
    data(&mut s);
    set(&mut s, "E2", "old");
    put(&mut s, totals());
    let st = debug(&mut s, json!({"action": "start", "name": "Totals", "inputs": "B2:C5", "onConflict": "shiftDown"}));
    assert_eq!(st["shifted"], "shiftDown", "{st}");
    assert_eq!(debug(&mut s, json!({"action": "stop"}))["shifted"], "shiftDown");
    // The files the run wrote stay when the workbook is put back: a failed step, a Stop step
    // that rolls back, and stopping without keeping each say so.
    let dir = std::env::temp_dir().join(format!("gc-macro-debug-files-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let export = json!({"command": "file.exportCsv", "params": {"path": format!("{}/out.csv", dir.to_string_lossy())}});
    for (last, how) in [
        (json!({"command": "cell.set", "params": {"cell": "Nowhere!A2", "input": "two"}}), "failed"),
        (json!({"type": "stop", "message": "undo", "rollback": true}), "stopped"),
        (json!({"command": "cell.set", "params": {"cell": "A2", "input": "two"}}), "stop"),
    ] {
        let mut s = super::s();
        put(&mut s, json!({"name": "F", "output": {"onConflict": "overwrite"}, "permissions": ["file.exportCsv"], "steps": [export, last]}));
        debug(&mut s, json!({"action": "start", "name": "F", "approve": true}));
        debug(&mut s, json!({"action": "step"}));
        let st = if how == "stop" { debug(&mut s, json!({"action": "stop"})) } else { debug(&mut s, json!({"action": "step"})) };
        let msg = st["message"].as_str().unwrap_or_default();
        assert!(msg.contains("the file step 1 (file.exportCsv) wrote stays") && !msg.contains("Nothing was changed"), "{how}: {st}");
        assert_eq!(st["wroteFiles"], json!(["step 1 (file.exportCsv)"]), "{how}: {st}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_debugger_lists_deleted_and_renamed_sheets_and_caps_the_list() {
    let mut s = s();
    s.execute("home.insertSheet", json!({"name": "Sheet2"})).unwrap();
    s.execute("home.insertSheet", json!({"name": "Sheet3"})).unwrap();
    s.execute("sheet.activate", json!({"sheet": 0})).unwrap();
    put(
        &mut s,
        json!({"name": "Sheets", "output": {"onConflict": "overwrite"}, "steps": [
            {"command": "sheet.rename", "params": {"sheet": "Sheet2", "name": "Renamed"}},
            {"command": "home.deleteSheet", "params": {"sheet": "Sheet3"}},
            {"command": "cell.set", "params": {"cell": "A1", "input": "end"}}
        ]}),
    );
    debug(&mut s, json!({"action": "start", "name": "Sheets"}));
    debug(&mut s, json!({"action": "step"}));
    let st = debug(&mut s, json!({"action": "step"}));
    let changed = st["changed"].as_array().unwrap();
    assert!(changed.contains(&json!("Renamed (renamed from Sheet2)")), "{st}");
    assert!(changed.contains(&json!("Sheet3 (deleted sheet)")), "{st}");
    assert!(!changed.iter().any(|c| c.as_str().is_some_and(|c| c.contains("(new sheet)"))), "{st}");
    // As if a step had written cells that make more ranges than the answer lists, on two sheets.
    let d = s.doc_mut().unwrap();
    let mut wb = (*d.wb).clone();
    for sh in wb.sheets.iter_mut() {
        let sh = Arc::make_mut(sh);
        for r in 0..600u32 {
            sh.set_value(CellRef::new(r + 10, r % 2), gridcraft_core::Value::Number(1.0));
        }
    }
    d.wb = Arc::new(wb);
    let st = debug(&mut s, json!({"action": "state"}));
    assert_eq!(st["changed"].as_array().unwrap().len(), 500, "{}", st["changed"].as_array().unwrap().len());
    assert_eq!(st["changedCells"], 1200);
    debug(&mut s, json!({"action": "stop"}));
}

#[test]
fn the_debugger_shows_a_repeat_over_whole_columns_as_the_run_goes_over_them() {
    let mut s = s();
    data(&mut s);
    put(
        &mut s,
        json!({"name": "Cols", "output": {"onConflict": "overwrite"}, "steps": [
            {"type": "repeat", "over": "B:C", "of": "columns", "body": [{"command": "cell.set", "params": {"cell": "Z1", "input": "x"}}]}
        ]}),
    );
    let st = debug(&mut s, json!({"action": "start", "name": "Cols"}));
    assert_eq!(st["current"]["over"], "B2:C5", "{st}");
    debug(&mut s, json!({"action": "stop"}));
}

#[test]
fn closing_the_debugged_workbook_ends_the_debugger() {
    // `file.close` is not an edit, so the debugger's lock lets it through; the group went with
    // the document, and so must the debugger, or the next workbook could not debug anything and
    // the state would report that no document is open.
    let mut s = s();
    data(&mut s);
    put(&mut s, totals());
    s.execute("macro.debug", json!({"action": "start", "name": "Totals", "inputs": "B2:C5", "breakpoints": ["1"]})).unwrap();
    assert!(s.macro_debug.is_some());
    s.execute("file.close", json!({"force": true})).unwrap();
    assert!(s.macro_debug.is_none(), "the debugger ended with its workbook");
    s.new_workbook();
    data(&mut s);
    put(&mut s, totals());
    s.execute("macro.debug", json!({"action": "start", "name": "Totals", "inputs": "B2:C5", "breakpoints": ["1"]})).unwrap();
    assert!(s.execute("macro.debug", json!({})).is_ok(), "the new workbook can be debugged");
    s.execute("macro.debug", json!({"action": "stop"})).unwrap();
}

#[test]
fn a_shift_that_would_move_an_input_is_refused() {
    // A shift inserts cells over the conflicting block, which moves the block's columns (or rows)
    // from there on. When one of the macro's own inputs lies in the way, its data would move and
    // shear while the run keeps the old coordinates, so the shift is refused and nothing changes.
    let mut s = s();
    data(&mut s);
    set(&mut s, "B1", "Old title");
    put(
        &mut s,
        json!({"name": "Title", "inputs": [{"name": "Input", "header": true}], "steps": [
            {"command": "cell.set", "params": {"cell": "Input.cell(0,1)", "input": "Report"}}
        ]}),
    );
    let start = s.doc().unwrap().wb.clone();
    let err = s.execute("macro.run", json!({"name": "Title", "inputs": "B2:C5", "onConflict": "shiftDown"})).unwrap_err().to_string();
    assert!(err.contains("would move Input (") && err.contains("B2:C5"), "{err}");
    assert!(Arc::ptr_eq(&s.doc().unwrap().wb, &start), "nothing changed");
    assert_eq!((v(&s, "B1").display(), v(&s, "B3").display()), ("Old title".into(), "Ann".into()));
    // Writing left of the input and shifting right would move it too.
    put(
        &mut s,
        json!({"name": "Left", "inputs": [{"name": "Input", "header": true}], "steps": [
            {"command": "cell.set", "params": {"cell": "Input.offset(1,-1).cell(1,1)", "input": "x"}}
        ]}),
    );
    set(&mut s, "A3", "in the way");
    let err = s.execute("macro.run", json!({"name": "Left", "inputs": "B2:C5", "onConflict": "shiftRight"})).unwrap_err().to_string();
    assert!(err.contains("would move Input"), "{err}");
    // The output counts as well.
    put(
        &mut s,
        json!({"name": "Out", "inputs": [{"name": "Input", "header": true}], "output": {"anchor": "Input.below(1)"}, "steps": [
            {"command": "cell.set", "params": {"cell": "Input.cell(0,1)", "input": "Report"}}
        ]}),
    );
    let err = s.execute("macro.run", json!({"name": "Out", "inputs": "B2:C5", "onConflict": "shiftDown"})).unwrap_err().to_string();
    assert!(err.contains("would move Input"), "{err}");
    // A shift that moves nothing of the macro's own still works, over the block of every written
    // cell, so a row the macro writes stays together.
    set(&mut s, "H1", "elsewhere");
    put(
        &mut s,
        json!({"name": "Wide", "inputs": [{"name": "Input", "header": true}], "steps": [
            {"command": "cell.set", "params": {"cell": "H1", "input": "over"}},
            {"command": "cell.set", "params": {"cell": "H9", "input": "far"}}
        ]}),
    );
    s.execute("macro.run", json!({"name": "Wide", "inputs": "B2:C5", "onConflict": "shiftDown"})).unwrap();
    assert_eq!((v(&s, "H1").display(), v(&s, "H9").display(), v(&s, "H10").display()), ("over".into(), "far".into(), "elsewhere".into()));
    assert_eq!(v(&s, "B3").display(), "Ann", "the input did not move");
}

#[test]
fn a_shift_is_refused_when_the_footprint_cannot_follow_every_write() {
    // The touch log keeps at most a million writes; past that, a write that kept a cell's value
    // is missing from the footprint, so a shift could split a row the macro writes. The footprint
    // says so and the shift is refused (overwrite, with no dry run, still runs).
    let mut s = s();
    set(&mut s, "A1", "x");
    set(&mut s, "E2", "old");
    put(
        &mut s,
        json!({"name": "Flood", "steps": [
            {"command": "edit.fillDown", "params": {"range": "A1:A1000001"}},
            {"command": "cell.set", "params": {"cell": "E2", "input": "new"}}
        ]}),
    );
    let c = s.execute("macro.check", json!({"name": "Flood"})).unwrap();
    assert_eq!(c["footprint"]["logFull"], true, "{c}");
    let err = s.execute("macro.run", json!({"name": "Flood", "onConflict": "shiftDown"})).unwrap_err().to_string();
    assert!(err.contains("more cells than the footprint can follow"), "{err}");
    assert_eq!(v(&s, "E2").display(), "old", "nothing changed");
}

#[test]
fn a_dry_run_leaves_a_clean_workbook_clean() {
    // `view.zoom` and `sheet.activate` move the saved mark to the current workbook on a clean
    // document; the dry run's rollback puts the workbook back, so the mark must come back too.
    let mut s = s();
    data(&mut s);
    let d = s.doc_mut().unwrap();
    d.saved = d.wb.clone();
    assert!(!s.doc().unwrap().is_dirty());
    put(&mut s, json!({"name": "Look", "steps": [{"command": "view.zoom", "params": {"percent": 120}}]}));
    s.execute("macro.check", json!({"name": "Look"})).unwrap();
    assert!(!s.doc().unwrap().is_dirty(), "a dry run changed nothing");
    // A run refused by the conflict rule changed nothing either.
    put(&mut s, totals());
    set(&mut s, "E3", "taken");
    let d = s.doc_mut().unwrap();
    d.saved = d.wb.clone();
    assert!(s.execute("macro.run", json!({"name": "Totals", "inputs": "B2:C5", "onConflict": "fail"})).is_err());
    assert!(!s.doc().unwrap().is_dirty());
}

#[test]
fn a_dry_run_puts_the_journal_back_even_after_it_trimmed_it() {
    // Every nested command a dry run executes is journaled, so a long macro pushes the journal
    // past its limit and the journal drops its oldest entries. Putting the journal back by its
    // old length would then keep entries from the dry run and point a recording at the wrong
    // place; the restore counts what was dropped.
    let mut s = s();
    data(&mut s);
    s.execute("selection.set", json!({"range": "A1"})).unwrap();
    let journal = s.journal.clone();
    s.recording = Some(journal.len().saturating_sub(1));
    put(
        &mut s,
        json!({"name": "Long", "steps": [{"type": "repeat", "over": "A1:A200", "body": [
            {"type": "repeat", "over": "A1:A60", "body": [{"command": "cell.set", "params": {"cell": "Row.cell(1,1)", "input": "x"}}]}
        ]}]}),
    );
    s.execute("macro.check", json!({"name": "Long"})).unwrap();
    let sets = |j: &[(String, Json)]| j.iter().filter(|(id, _)| id == "cell.set").count();
    assert!(sets(&s.journal) <= sets(&journal), "no dry-run entry stays");
    assert!(s.journal.len() <= journal.len(), "{} entries, had {}", s.journal.len(), journal.len());
    assert!(journal.ends_with(&s.journal), "what stays is the journal's own tail");
    assert!(s.recording.is_some_and(|r| r <= s.journal.len()), "{:?}", s.recording);
    // A short dry run leaves the journal exactly as it was.
    put(&mut s, json!({"name": "Short", "steps": [{"command": "cell.set", "params": {"cell": "H1", "input": "x"}}]}));
    let (journal, recording) = (s.journal.clone(), s.recording);
    s.execute("macro.check", json!({"name": "Short"})).unwrap();
    assert_eq!((&s.journal, s.recording), (&journal, recording));
}

#[test]
fn each_step_is_checked_for_writes_over_data() {
    let dir = std::env::temp_dir().join(format!("gc-macro-guard-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let csv = dir.join("out.csv").to_string_lossy().to_string();
    let export = json!({"command": "file.exportCsv", "params": {"path": csv}});
    let solver = json!({"command": "data.solver", "params": {"objective": "C1", "variables": "A1", "constraints": ["A1 <= 5"], "method": "simplex", "saveModel": false}});
    let late = json!({"command": "cell.set", "params": {"cell": "B1", "input": "over"}});
    // A file step or Solver, then a step that writes over B1: the run stops there.
    for first in [&export, &solver] {
        let mut s = super::s();
        set(&mut s, "A1", "1");
        set(&mut s, "C1", "=A1");
        set(&mut s, "B1", "keep");
        put(&mut s, json!({"name": "Late", "inputs": [{"name": "Input"}], "permissions": ["file.exportCsv"], "steps": [first, late]}));
        let start = s.doc().unwrap().wb.clone();
        for rule in ["fail", "ask"] {
            let r = s.execute("macro.run", json!({"name": "Late", "inputs": "A1", "onConflict": rule, "approve": true}));
            let e = match (rule, r) {
                ("fail", Err(EngineError::Other(e))) | ("ask", Err(EngineError::Confirm(e))) => e,
                (_, r) => panic!("{first} {rule}: {r:?}"),
            };
            assert!(e.contains("Step 2 (cell.set): It would write over data (B1 on Sheet1)"), "{e}");
            // The workbook is put back; a file the run wrote stays, and the message says so.
            let left =
                if first == &export { "The workbook was put back; the file step 1 (file.exportCsv) wrote stays." } else { "Nothing was changed." };
            assert!(e.ends_with(left), "{e}");
            assert_eq!(v(&s, "B1").display(), "keep", "{first} {rule}");
            assert!(Arc::ptr_eq(&s.doc().unwrap().wb, &start), "{first} {rule}: rolled back");
        }
        // The answer to the question overwrites.
        s.execute("macro.run", json!({"name": "Late", "inputs": "A1", "overwrite": true})).unwrap();
        assert_eq!(v(&s, "B1").display(), "over");
    }
    // A step that writes over what the macro itself wrote isn't a conflict.
    let mut s = super::s();
    let again = |input: &str| json!({"command": "cell.set", "params": {"cell": "E1", "input": input}});
    put(
        &mut s,
        json!({"name": "Twice", "permissions": ["file.exportCsv"], "output": {"onConflict": "fail"}, "steps": [again("a"), export, again("b")]}),
    );
    s.execute("macro.run", json!({"name": "Twice", "approve": true})).unwrap();
    assert_eq!(v(&s, "E1").display(), "b");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_run_that_takes_another_path_than_its_check_is_still_checked() {
    // RAND gives the run other values than the dry run before it, so an If can go the other way:
    // the write the footprint didn't see must still not go over data.
    let mut s = s();
    let steps = json!([
        {"command": "cell.set", "params": {"cell": "A1", "input": "=RAND()"}},
        {"type": "if", "cell": "A1", "op": "gt", "value": 0.5, "then": [{"command": "cell.set", "params": {"cell": "B1", "input": "over"}}]}
    ]);
    put(&mut s, json!({"name": "Coin", "output": {"onConflict": "fail"}, "steps": steps}));
    let mut outcomes = BTreeSet::new();
    for _ in 0..40 {
        set(&mut s, "B1", "keep");
        let r = s.execute("macro.run", json!({"name": "Coin"}));
        assert_eq!(v(&s, "B1").display(), "keep", "{r:?}");
        outcomes.insert(match r {
            Ok(_) => "ran",
            Err(e) if e.to_string().contains("would write over") => "refused",
            Err(e) => panic!("{e}"),
        });
    }
    assert_eq!(outcomes.len(), 2, "both paths were taken: {outcomes:?}");
}

#[test]
fn run_parameters_that_would_be_ignored_are_refused() {
    let mut s = s();
    data(&mut s);
    put(&mut s, totals());
    for (p, says) in [
        (json!({"inputs": {"Inptu": "B2:C5"}}), "it has no input called 'Inptu' (its inputs: Input)."),
        (json!({"inputs": {"Input": 5}}), "give the input Input as reference text"),
        (json!({"inputs": ["B2:C5"]}), "give `inputs` as reference text"),
        (json!({"inputs": "B2:C5", "onConflict": 1}), "`onConflict` must be"),
        (json!({"inputs": "B2:C5", "overwrite": "yes"}), "`overwrite` must be true or false."),
        (json!({"inputs": "B2:C5", "approve": 1}), "`approve` must be true or false."),
        (json!({"inputs": "B2:C5", "timeLimit": "30"}), "`timeLimit` must be a number of seconds above 0."),
        (json!({"inputs": "B2:C5", "timeLimit": 0}), "`timeLimit` must be a number of seconds above 0."),
    ] {
        let mut p = p;
        p["name"] = json!("Totals");
        let err = s.execute("macro.run", p.clone()).unwrap_err().to_string();
        assert!(err.contains(&format!("Macro 'Totals': {says}")), "{p}: {err}");
    }
    put(&mut s, json!({"name": "None", "steps": []}));
    let err = s.execute("macro.run", json!({"name": "None", "inputs": "A1"})).unwrap_err().to_string();
    assert!(err.contains("Macro 'None': it takes no inputs."), "{err}");
    // `null` is the same as not given, and an input's name matches in any case.
    let p = json!({"name": "Totals", "inputs": {"input": "B2:C5"}, "onConflict": null, "overwrite": true, "timeLimit": null});
    s.execute("macro.run", p).unwrap();
}

#[test]
fn a_run_passes_a_bounded_amount_of_parameters() {
    // A step with a 3 MB parameter in a Repeat of 100 rows would hand its command 300 MB; the run
    // stops at 256 MB, which matters on the web build, where there is no time limit.
    let mut s = s();
    let big = "x".repeat(3_000_000);
    let body = json!([{"command": "home.bold", "params": {"range": "A1", "note": big}}]);
    put(&mut s, json!({"name": "Big", "output": {"onConflict": "overwrite"}, "steps": [{"type": "repeat", "over": "A1:A100", "body": body}]}));
    let err = s.execute("macro.run", json!({"name": "Big"})).unwrap_err().to_string();
    assert!(err.contains("passed more than 256 MB of parameters to its steps"), "{err}");
    // 50 rows are 150 MB: the run alone stays under the limit, the dry run and the run together
    // go past it, as they share the step budget.
    let body = json!([{"command": "home.bold", "params": {"range": "A1", "note": "x".repeat(3_000_000)}}]);
    for (rule, ok) in [("overwrite", true), ("fail", false)] {
        let repeat = json!({"type": "repeat", "over": "A1:A50", "body": body});
        put(&mut s, json!({"name": "Half", "output": {"onConflict": rule}, "steps": [repeat]}));
        assert_eq!(s.execute("macro.run", json!({"name": "Half"})).is_ok(), ok, "{rule}");
    }
}

#[test]
fn the_check_says_whether_the_run_would_go_ahead() {
    let mut s = s();
    // A Stop step that rolls the run back.
    put(&mut s, json!({"name": "Halt", "output": {"onConflict": "fail"}, "steps": [{"type": "stop", "message": "Not today.", "rollback": true}]}));
    let c = s.execute("macro.check", json!({"name": "Halt"})).unwrap();
    assert_eq!((&c["ok"], &c["errors"]), (&json!(false), &json!(["A Stop step ends the run and rolls it back: Not today."])));
    // A shift that would move an input.
    set(&mut s, "B1", "taken");
    set(&mut s, "B3", "in");
    let step = json!({"command": "cell.set", "params": {"cell": "B1", "input": "new"}});
    put(&mut s, json!({"name": "Move", "inputs": [{"name": "Input", "kind": "cell"}], "output": {"onConflict": "shiftDown"}, "steps": [step]}));
    let c = s.execute("macro.check", json!({"name": "Move", "inputs": "B3"})).unwrap();
    assert_eq!(c["ok"], false);
    assert!(c["errors"][0].as_str().unwrap().starts_with("Shifting the data would move Input (Sheet1!B3)."), "{c}");
    // A macro whose dry run takes 90,000 steps passes its own dry run, but the run spends the
    // budget twice under fail, so the check says it wouldn't finish.
    let spin = json!([{"type": "repeat", "over": "A1:A300", "body": [{"type": "repeat", "over": "A1:A300", "body": [{"type": "if", "cell": "A1", "op": "empty"}]}]}]);
    for (rule, ok) in [("fail", false), ("overwrite", true)] {
        put(&mut s, json!({"name": "Spin", "output": {"onConflict": rule}, "steps": spin}));
        let c = s.execute("macro.check", json!({"name": "Spin"})).unwrap();
        assert_eq!(c["ok"], ok, "{rule}: {c}");
        assert_eq!(s.execute("macro.run", json!({"name": "Spin"})).is_ok(), ok, "{rule}: the run agrees");
    }
    let c = s.execute("macro.check", json!({"name": "Spin", "onConflict": "fail"})).unwrap();
    assert!(c["errors"][0].as_str().unwrap().starts_with("The run would execute more than 100000 steps, counting the check before it"), "{c}");
    // Parameters the run would refuse, the check reports.
    let c = s.execute("macro.check", json!({"name": "Spin", "onConflict": 5})).unwrap();
    assert!(c["errors"][0].as_str().unwrap().contains("`onConflict` must be"), "{c}");
}

#[test]
fn the_question_after_a_partial_footprint_says_what_the_answer_covers() {
    let dir = std::env::temp_dir().join(format!("gc-macro-unseen-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let csv = dir.join("out.csv").to_string_lossy().to_string();
    let mut s = s();
    set(&mut s, "B1", "keep");
    set(&mut s, "C1", "also");
    let steps = json!([
        {"command": "cell.set", "params": {"cell": "B1", "input": "over"}},
        {"command": "file.exportCsv", "params": {"path": csv}},
        {"command": "cell.set", "params": {"cell": "C1", "input": "over"}}
    ]);
    put(&mut s, json!({"name": "Two", "permissions": ["file.exportCsv"], "steps": steps}));
    let r = s.execute("macro.run", json!({"name": "Two", "approve": true}));
    let Err(EngineError::Confirm(q)) = r else { panic!("{r:?}") };
    assert!(q.contains("(B1 on Sheet1). The check stopped at step 2 (file.exportCsv writes a file), so overwriting also lets the steps after it write over data unchecked."), "{q}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_dry_run_puts_back_the_go_to_list() {
    let mut s = s();
    let step = json!({"command": "edit.goTo", "params": {"reference": "D4", "record": true}});
    put(&mut s, json!({"name": "Go", "steps": [step]}));
    s.execute("macro.check", json!({"name": "Go"})).unwrap();
    assert_eq!(s.doc().unwrap().go_to_recent, Vec::<String>::new());
}

#[test]
fn the_answer_to_the_question_counts_however_ask_was_given() {
    let mut s = s();
    data(&mut s);
    put(&mut s, totals());
    set(&mut s, "E3", "taken");
    let p = json!({"name": "Totals", "inputs": "B2:C5", "onConflict": "ask"});
    assert!(matches!(s.execute("macro.run", p.clone()), Err(EngineError::Confirm(_))));
    let mut answered = p;
    answered["overwrite"] = json!(true);
    s.execute("macro.run", answered).unwrap();
    assert_eq!(v(&s, "E3").display(), "400");
}

#[test]
fn the_step_limit_is_one_budget_across_the_dry_run_and_the_run() {
    // 300 x 300 passes is under the limit for one phase; the dry run and the run together are not.
    let mut s = s();
    put(
        &mut s,
        json!({"name": "Spin", "steps": [{"type": "repeat", "over": "A1:A300", "body": [
            {"type": "repeat", "over": "A1:A300", "body": [{"type": "if", "cell": "A1", "op": "empty"}]}
        ]}]}),
    );
    let err = s.execute("macro.run", json!({"name": "Spin"})).unwrap_err().to_string();
    assert!(err.contains("more than 100000 steps"), "{err}");
    // With no dry run (overwrite), the same macro fits.
    s.execute("macro.run", json!({"name": "Spin", "onConflict": "overwrite"})).unwrap();
}

#[test]
fn the_footprint_covers_other_sheets_objects_and_edges() {
    let mut s = s();
    data(&mut s);
    s.execute("home.insertSheet", json!({"name": "Notes"})).unwrap();
    set(&mut s, "A1", "note");
    s.execute("sheet.activate", json!({"sheet": "Sheet1"})).unwrap();
    s.execute("insert.chart", json!({"range": "B2:C5", "type": "column", "at": "H2"})).unwrap();
    let chart = s.doc().unwrap().wb.sheet(0).unwrap().charts[0].id;
    put(
        &mut s,
        json!({"name": "Wide", "output": {"onConflict": "fail"}, "steps": [
            {"command": "cell.set", "params": {"cell": "Notes!A1", "input": "x"}},
            {"command": "cell.set", "params": {"cell": "Notes!B2", "input": "y"}},
            {"command": "home.insertSheet", "params": {"name": "Extra"}},
            {"command": "sheet.activate", "params": {"sheet": "Sheet1"}},
            {"command": "insert.shape", "params": {"kind": "ellipse", "at": "J2"}},
            {"command": "object.delete", "params": {"kind": "chart", "id": chart}}
        ]}),
    );
    let c = s.execute("macro.check", json!({"name": "Wide", "inputs": {}})).unwrap();
    let fp = &c["footprint"];
    assert_eq!(
        fp["sheets"],
        json!([{"sheet": "Notes", "cells": ["A1", "B2"], "changed": 2, "conflicts": ["A1"], "conflictCount": 1, "bounds": "A1:B2"}])
    );
    assert_eq!(fp["sheetsAdded"], json!(["Extra"]));
    assert_eq!((fp["objectsAdded"].as_u64(), fp["objectsRemoved"].as_u64()), (Some(1), Some(1)));
    assert_eq!(c["conflicts"], 2, "the deleted chart and the note");
    assert_eq!(s.doc().unwrap().wb.sheets.len(), 2, "the dry run's new sheet went away");
    let err = s.execute("macro.run", json!({"name": "Wide"})).unwrap_err().to_string();
    assert!(err.contains("(A1 on Notes; 1 object)"), "{err}");

    // Running past the edge of the sheet fails before anything changes.
    put(
        &mut s,
        json!({"name": "Edge", "inputs": [{"name": "Input"}], "steps": [{"command": "cell.set", "params": {"cell": "Input.offset(-3,0)", "input": "x"}}]}),
    );
    let err = s.execute("macro.run", json!({"name": "Edge", "inputs": "B2"})).unwrap_err().to_string();
    assert!(err.contains("Step 1 (cell.set): 'Input.offset(-3,0)' runs past the top of the sheet."), "{err}");
    let c = s.execute("macro.check", json!({"name": "Edge", "inputs": "B2"})).unwrap();
    assert_eq!((c["ok"].as_bool(), c["footprint"]["error"]["edge"].as_bool()), (Some(false), Some(true)));
}

#[test]
fn repeat_if_and_stop() {
    let mut s = s();
    data(&mut s);
    put(
        &mut s,
        json!({"name": "Flag", "inputs": [{"name": "Input", "header": true}], "output": {"onConflict": "overwrite"}, "steps": [
            {"type": "repeat", "over": "Input.data", "body": [
                {"type": "if", "cell": "Row.cell(1,2)", "op": "gt", "value": 100,
                 "then": [{"command": "cell.set", "params": {"cell": "Row.right", "input": "big"}}],
                 "else": [{"command": "cell.set", "params": {"cell": "Row.right", "input": "small"}}]},
                {"type": "if", "cell": "Row.cell(1,1)", "op": "eq", "value": "bo", "then": [{"type": "stop", "message": "Found Bo."}]}
            ]},
            {"command": "cell.set", "params": {"cell": "A1", "input": "not reached"}}
        ]}),
    );
    let r = s.execute("macro.run", json!({"name": "Flag", "inputs": "B2:C5"})).unwrap();
    assert_eq!(r["stopped"], "Found Bo.");
    assert_eq!((v(&s, "D3").display(), v(&s, "D4").display()), ("big".into(), "small".into()));
    assert!(v(&s, "D5").is_empty() && v(&s, "A1").is_empty());
    assert_eq!(undo_labels(&s).last().unwrap(), "Run Macro: Flag", "a stop keeps the changes");
    // A Stop that rolls back is an error that changes nothing.
    put(
        &mut s,
        json!({"name": "Guard", "output": {"onConflict": "overwrite"}, "steps": [
            {"command": "cell.set", "params": {"cell": "A1", "input": "x"}},
            {"type": "if", "cell": "C3", "op": "notEmpty", "then": [{"type": "stop", "message": "C3 must be empty.", "rollback": true}]}
        ]}),
    );
    let err = s.execute("macro.run", json!({"name": "Guard"})).unwrap_err().to_string();
    assert_eq!(err, "Macro 'Guard' stopped: C3 must be empty. Nothing was changed.");
    assert!(v(&s, "A1").is_empty());
    // Comparisons.
    for (op, value, want) in [
        ("eq", json!(120), true),
        ("eq", json!("120"), true),
        ("ne", json!(120), false),
        ("ge", json!(120), true),
        ("lt", json!(120.5), true),
        ("le", json!(100), false),
        ("contains", json!("12"), true),
        ("empty", json!(null), false),
    ] {
        put(
            &mut s,
            json!({"name": "Cmp", "output": {"onConflict": "overwrite"}, "steps": [
                {"type": "if", "cell": "C3", "op": op, "value": value, "then": [{"command": "cell.set", "params": {"cell": "A9", "input": "yes"}}], "else": [{"command": "cell.set", "params": {"cell": "A9", "input": "no"}}]}
            ]}),
        );
        s.execute("macro.run", json!({"name": "Cmp"})).unwrap();
        assert_eq!(v(&s, "A9").display(), if want { "yes" } else { "no" }, "{op} {value}");
    }
    // If needs one cell.
    put(&mut s, json!({"name": "Many", "steps": [{"type": "if", "cell": "A1:A2", "op": "empty"}]}));
    assert!(s.execute("macro.run", json!({"name": "Many"})).unwrap_err().to_string().contains("If looks at one cell"));
    // Disabled steps are skipped.
    put(
        &mut s,
        json!({"name": "Off", "output": {"onConflict": "overwrite"}, "steps": [{"command": "cell.set", "params": {"cell": "A8", "input": "x"}, "enabled": false}]}),
    );
    assert_eq!(s.execute("macro.run", json!({"name": "Off"})).unwrap()["ran"], 0);
}

#[test]
fn macros_are_bounded() {
    let mut s = s();
    put(&mut s, json!({"name": "Long", "steps": [{"type": "repeat", "over": "A1:A20", "max": 10, "body": [{"type": "stop"}]}]}));
    let err = s.execute("macro.run", json!({"name": "Long"})).unwrap_err().to_string();
    assert!(err.contains("Repeat over A1:A20 has 20 rows, more than its limit of 10."), "{err}");
    // 400 × 400 passes is over the step limit.
    put(
        &mut s,
        json!({"name": "Spin", "output": {"onConflict": "overwrite"}, "steps": [{"type": "repeat", "over": "A1:A400", "body": [
            {"type": "repeat", "over": "A1:A400", "body": [{"type": "if", "cell": "A1", "op": "empty"}]}
        ]}]}),
    );
    let err = s.execute("macro.run", json!({"name": "Spin"})).unwrap_err().to_string();
    assert!(err.contains("ran more than 100000 steps"), "{err}");
    let err = s.execute("macro.run", json!({"name": "Spin", "timeLimit": 0.001})).unwrap_err().to_string();
    assert!(err.contains("ran longer than"), "{err}");
    // The footprint's dry run keeps to the time limit too, and so does the check's.
    put(
        &mut s,
        json!({"name": "Spin", "output": {"onConflict": "fail"}, "steps": [{"type": "repeat", "over": "A1:A400", "body": [
            {"type": "repeat", "over": "A1:A400", "body": [{"type": "if", "cell": "A1", "op": "empty"}]}
        ]}]}),
    );
    let err = s.execute("macro.run", json!({"name": "Spin", "timeLimit": 0.001})).unwrap_err().to_string();
    assert!(err.contains("ran longer than"), "{err}");
    let c = s.execute("macro.check", json!({"name": "Spin", "timeLimit": 0.001})).unwrap();
    assert!(c["errors"][0].as_str().unwrap().contains("ran longer than"), "{c}");
}

#[test]
fn a_token_in_a_path_needs_the_permission_and_approval() {
    let mut s = s();
    let dir = std::env::temp_dir().join(format!("gc-macro-token-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let files = |dir: &std::path::Path| std::fs::read_dir(dir).unwrap().count();
    let in_dir = format!("{}/{{{{Input}}}}", dir.to_string_lossy());
    for path in [json!({"$ref": "Input"}), json!(in_dir), json!(" ")] {
        let step = json!({"command": "file.exportCsv", "params": {"path": path}});
        put(&mut s, json!({"name": "Export", "inputs": [{"name": "Input"}], "steps": [step]}));
        // Without the permission nothing runs, not even the check's dry run.
        let err = s.execute("macro.run", json!({"name": "Export"})).unwrap_err().to_string();
        assert!(err.contains("Step 1 (file.exportCsv) writes a file. Add file.exportCsv to the macro's permissions"), "{path}: {err}");
        let c = s.execute("macro.check", json!({"name": "Export"})).unwrap();
        assert!(c["errors"][0].as_str().unwrap().contains("writes a file"), "{path}: {c}");
        assert!(c.get("footprint").is_none());
        // With it, the person approves first; the check doesn't run it before then.
        put(&mut s, json!({"name": "Export", "inputs": [{"name": "Input"}], "steps": [step], "permissions": ["file.exportCsv"]}));
        let r = s.execute("macro.run", json!({"name": "Export"}));
        let Err(EngineError::Approve(q)) = &r else { panic!("{path}: {r:?}") };
        assert!(q.contains("file.exportCsv, which writes a file"), "{q}");
        let c = s.execute("macro.check", json!({"name": "Export"})).unwrap();
        assert_eq!(c["needsApproval"], json!(["runs file.exportCsv, which writes a file"]), "{path}");
        assert!(c.get("footprint").is_none());
        assert_eq!(files(&dir), 0, "{path}");
        assert!(!std::path::Path::new("A1.csv").exists() && !std::path::Path::new(" .csv").exists());
    }
    // Once approved, the check's dry run stops before the step, and only the run writes the file.
    let step = json!({"command": "file.exportCsv", "params": {"path": in_dir}});
    put(&mut s, json!({"name": "Export", "inputs": [{"name": "Input"}], "steps": [step], "permissions": ["file.exportCsv"]}));
    s.execute("macro.run", json!({"name": "Export", "approve": true, "inputs": "B2"})).unwrap();
    assert_eq!(files(&dir), 1);
    std::fs::remove_dir_all(&dir).unwrap();
    std::fs::create_dir_all(&dir).unwrap();
    let c = s.execute("macro.check", json!({"name": "Export", "inputs": "B2"})).unwrap();
    assert_eq!(c["footprint"]["partial"]["step"], "1", "{c}");
    assert_eq!(files(&dir), 0, "the check writes nothing");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_debugger_runs_a_token_path_only_with_the_permission_and_approval() {
    let mut s = s();
    let dir = std::env::temp_dir().join(format!("gc-macro-debug-token-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let files = |dir: &std::path::Path| std::fs::read_dir(dir).unwrap().count();
    let step = json!({"command": "file.exportCsv", "params": {"path": format!("{}/{{{{Input}}}}", dir.to_string_lossy())}});
    put(&mut s, json!({"name": "Export", "inputs": [{"name": "Input"}], "steps": [step]}));
    let err = s.execute("macro.debug", json!({"action": "start", "name": "Export"})).unwrap_err().to_string();
    assert!(err.contains("writes a file"), "{err}");
    put(&mut s, json!({"name": "Export", "inputs": [{"name": "Input"}], "steps": [step], "permissions": ["file.exportCsv"]}));
    assert!(matches!(s.execute("macro.debug", json!({"action": "start", "name": "Export"})), Err(EngineError::Approve(_))));
    assert_eq!(files(&dir), 0);
    // Approved, it pauses before the step and writes the file only when the step runs.
    s.execute("macro.debug", json!({"action": "start", "name": "Export", "approve": true, "inputs": "B2", "breakpoints": ["1"]})).unwrap();
    assert_eq!(files(&dir), 0, "paused before the step");
    s.execute("macro.debug", json!({"action": "step"})).unwrap();
    assert_eq!(files(&dir), 1, "the debugger runs the approved step");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_policy_keeps_macros_away_from_files_and_the_app() {
    let mut s = s();
    for (id, params) in [
        ("file.saveAs", json!({"path": "/tmp/x.xlsx"})),
        ("file.close", json!({})),
        ("app.language.set", json!({"language": "de"})),
        ("automate.runScript", json!({"name": "Script 1"})),
        ("edit.undo", json!({})),
        ("macro.run", json!({"name": "Self"})),
        ("view.newWindow", json!({})),
        ("view.hideWindow", json!({})),
    ] {
        put(
            &mut s,
            json!({"name": "Bad", "steps": [{"command": "cell.set", "params": {"cell": "A1", "input": "x"}}, {"command": id, "params": params}]}),
        );
        let err = s.execute("macro.run", json!({"name": "Bad"})).unwrap_err().to_string();
        assert!(err.contains(&format!("Step 2 ({id}) can't run in a macro")), "{err}");
        assert!(v(&s, "A1").is_empty(), "{id}: nothing ran");
    }
    // A disabled step isn't checked.
    put(&mut s, json!({"name": "Bad", "steps": [{"command": "file.close", "enabled": false}]}));
    s.execute("macro.run", json!({"name": "Bad"})).unwrap();

    // Reading a file needs the permission, then the person's approval (once per macro content).
    let dir = std::env::temp_dir().join(format!("gc-macro-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let csv = dir.join("in.csv");
    std::fs::write(&csv, "a,b\n1,2\n").unwrap();
    let step = json!({"command": "data.fromTextCsv", "params": {"path": csv.to_string_lossy(), "sheet": "Imported"}});
    put(&mut s, json!({"name": "Import", "steps": [step]}));
    let err = s.execute("macro.run", json!({"name": "Import"})).unwrap_err().to_string();
    assert!(err.contains("Step 1 (data.fromTextCsv) reads a file. Add data.fromTextCsv to the macro's permissions"), "{err}");
    put(&mut s, json!({"name": "Import", "steps": [step], "permissions": ["data.fromTextCsv"]}));
    let r = s.execute("macro.run", json!({"name": "Import"}));
    let Err(EngineError::Approve(q)) = &r else { panic!("{r:?}") };
    assert_eq!(q, "Run macro 'Import'? It runs data.fromTextCsv, which reads a file.");
    assert_eq!(s.doc().unwrap().wb.sheets.len(), 1);
    let c = s.execute("macro.check", json!({"name": "Import"})).unwrap();
    assert_eq!(c["needsApproval"], json!(["runs data.fromTextCsv, which reads a file"]));
    assert!(c.get("footprint").is_none(), "nothing unapproved runs, even dry");
    s.execute("macro.run", json!({"name": "Import", "approve": true})).unwrap();
    assert_eq!(s.doc().unwrap().wb.sheets.len(), 2);
    s.execute("edit.undo", json!({})).unwrap();
    s.execute("macro.run", json!({"name": "Import"})).unwrap();
    // The dry run skips the file step and says so.
    let c = s.execute("macro.check", json!({"name": "Import"})).unwrap();
    assert_eq!(c["footprint"]["partial"]["step"], "1");
    // Changing the macro asks again.
    put(&mut s, json!({"name": "Import", "description": "changed", "steps": [step], "permissions": ["data.fromTextCsv"]}));
    assert!(matches!(s.execute("macro.run", json!({"name": "Import"})), Err(EngineError::Approve(_))));
    let _ = std::fs::remove_dir_all(&dir);

    // Steps that would open a dialog fail instead.
    put(&mut s, json!({"name": "Pic", "output": {"onConflict": "overwrite"}, "steps": [{"command": "insert.picture"}]}));
    let err = s.execute("macro.run", json!({"name": "Pic"})).unwrap_err().to_string();
    assert!(err.contains("would open the insertPicture dialog"), "{err}");
    assert!(s.ui_requests.is_empty());
    // A step that would start editing a cell, or put a name in the edit, runs without it.
    let (use_name, insert) = (json!({"name": "Rate"}), json!({"name": "SUM"}));
    let steps = json!([{"command": "formulas.useInFormula", "params": use_name}, {"command": "formulas.insertFunction", "params": insert}]);
    put(&mut s, json!({"name": "Edit", "output": {"onConflict": "overwrite"}, "steps": steps}));
    s.execute("macro.run", json!({"name": "Edit"})).unwrap();
    assert!(s.ui_requests.is_empty());
}

#[test]
fn macros_from_a_file_ask_before_running() {
    let mut s = s();
    let mut wb = gridcraft_model::Workbook::new();
    wb.macros.push(crate::macros::parse_macro(&json!({"name": "Theirs", "output": {"onConflict": "overwrite"}, "steps": [{"command": "cell.set", "params": {"cell": "A1", "input": "ran"}}]})).unwrap());
    let bytes = serde_json::to_vec(&wb).unwrap();
    s.execute("file.open", json!({"name": "theirs.json", "base64": crate::io::base64_encode(&bytes)})).unwrap();
    assert_eq!(s.doc().unwrap().trust, crate::Trust::FromFile);
    let r = s.execute("macro.run", json!({"name": "Theirs"}));
    let Err(EngineError::Approve(q)) = &r else { panic!("{r:?}") };
    assert_eq!(q, "Run macro 'Theirs'? It came with a workbook you opened.");
    assert!(v(&s, "A1").is_empty());
    s.execute("macro.run", json!({"name": "Theirs", "approve": true})).unwrap();
    assert_eq!(v(&s, "A1").display(), "ran");
    // Approved once per macro content: the second run doesn't ask.
    s.execute("macro.run", json!({"name": "Theirs"})).unwrap();
    // The approval holds the macro's whole content, not a hash two macros could share.
    let m = s.doc().unwrap().wb.macros[0].clone();
    let doc = s.doc().unwrap().uid;
    let key: (u64, String) = (doc, crate::macros::macro_json(&m).to_string());
    assert!(s.macro_approvals.contains(&key));
    // The JSON is canonical: the same macro with its keys in another order is the same macro.
    let a = crate::macros::parse_macro(&json!({"name": "M", "steps": [{"command": "cell.set", "params": {"cell": "A1", "input": "x"}}]})).unwrap();
    let b = crate::macros::parse_macro(&json!({"steps": [{"params": {"input": "x", "cell": "A1"}, "command": "cell.set"}], "name": "M"})).unwrap();
    assert_eq!(crate::macros::fingerprint(&a), crate::macros::fingerprint(&b));
}

#[test]
fn a_partial_footprint_doesnt_clear_later_writes() {
    let dir = std::env::temp_dir().join(format!("gc-macro-partial-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let csv = dir.join("out.csv").to_string_lossy().to_string();
    let export = json!({"command": "file.exportCsv", "params": {"path": csv}});
    let solver = json!({"command": "data.solver", "params": {"objective": "C1", "variables": "A1", "constraints": ["A1 <= 5"], "method": "simplex", "saveModel": false}});
    let late = json!({"command": "cell.set", "params": {"cell": "B1", "input": "over"}});
    // The dry run stops at an approved file step or at Solver; the step after it writes over B1.
    for first in [&export, &solver] {
        let mut s = super::s();
        set(&mut s, "A1", "1");
        set(&mut s, "C1", "=A1");
        set(&mut s, "B1", "keep");
        put(&mut s, json!({"name": "Late", "inputs": [{"name": "Input"}], "permissions": ["file.exportCsv"], "steps": [first, late]}));
        let start = s.doc().unwrap().wb.clone();
        let c = s.execute("macro.check", json!({"name": "Late", "inputs": "A1"})).unwrap();
        if c.get("needsApproval").is_none() {
            assert_eq!((c["conflicts"].as_u64(), &c["footprint"]["partial"]["step"]), (Some(0), &json!("1")), "{c}");
        }
        for rule in ["fail", "ask"] {
            let r = s.execute("macro.run", json!({"name": "Late", "inputs": "A1", "onConflict": rule, "approve": true}));
            let e = match (rule, r) {
                ("fail", Err(EngineError::Other(e))) | ("ask", Err(EngineError::Confirm(e))) => e,
                (_, r) => panic!("{first} {rule}: {r:?}"),
            };
            assert!(e.contains("Step 2 (cell.set): It would write over data (B1 on Sheet1)"), "{e}");
            assert_eq!(v(&s, "B1").display(), "keep", "{first} {rule}");
            assert!(Arc::ptr_eq(&s.doc().unwrap().wb, &start), "{first} {rule}: rolled back");
        }
        // The answer to the question overwrites.
        s.execute("macro.run", json!({"name": "Late", "inputs": "A1", "overwrite": true})).unwrap();
        assert_eq!(v(&s, "B1").display(), "over");
    }
    // A step that writes over what the macro itself wrote isn't a conflict.
    let mut s = super::s();
    let again = |input: &str| json!({"command": "cell.set", "params": {"cell": "E1", "input": input}});
    put(
        &mut s,
        json!({"name": "Twice", "permissions": ["file.exportCsv"], "output": {"onConflict": "fail"}, "steps": [again("a"), export, again("b")]}),
    );
    s.execute("macro.run", json!({"name": "Twice", "approve": true})).unwrap();
    assert_eq!(v(&s, "E1").display(), "b");
    // After a shift, the steps past the file step are checked too.
    let mut s = super::s();
    set(&mut s, "C1", "old");
    set(&mut s, "D1", "keep");
    let step = |cell: &str| json!({"command": "cell.set", "params": {"cell": cell, "input": "new"}});
    put(
        &mut s,
        json!({"name": "Shift", "permissions": ["file.exportCsv"], "output": {"onConflict": "shiftDown"}, "steps": [step("C1"), export, step("D1")]}),
    );
    let start = s.doc().unwrap().wb.clone();
    let err = s.execute("macro.run", json!({"name": "Shift", "approve": true})).unwrap_err().to_string();
    assert!(err.contains("Step 3 (cell.set): It would write over data (D1 on Sheet1)"), "{err}");
    assert_eq!((v(&s, "C1").display(), v(&s, "D1").display()), ("old".into(), "keep".into()));
    assert!(Arc::ptr_eq(&s.doc().unwrap().wb, &start), "the shift is rolled back too");
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------- debugger

fn debug(s: &mut Session, p: Json) -> Json {
    s.execute("macro.debug", p).unwrap()
}

/// Four steps: two plain ones, a Repeat over the data rows (with a breakpoint on its body), and
/// a final one.
fn stepper(s: &mut Session) {
    data(s);
    put(
        s,
        json!({"name": "Walk", "inputs": [{"name": "Input", "header": true}], "output": {"onConflict": "overwrite"}, "steps": [
            {"command": "cell.set", "params": {"cell": "E1", "input": "start"}},
            {"command": "cell.set", "params": {"cell": "Input.right.cell(1,1)", "input": "flag"}},
            {"type": "repeat", "over": "Input.data", "body": [
                {"command": "cell.set", "params": {"cell": "Row.right", "input": "seen"}, "breakpoint": true}
            ]},
            {"command": "cell.set", "params": {"cell": "E9", "input": "end"}}
        ]}),
    );
}

#[test]
fn the_debugger_steps_continues_and_runs_to_a_step_as_one_undo_step() {
    let mut s = s();
    stepper(&mut s);
    let undo = undo_labels(&s);
    let st = debug(&mut s, json!({"name": "Walk", "inputs": "B2:C5", "action": "start"}));
    assert_eq!(st["state"], "paused");
    assert_eq!(
        st["current"],
        json!({"step": "1", "kind": "command", "command": "cell.set", "params": {"cell": "E1", "input": "start"}, "breakpoint": false})
    );
    assert_eq!(st["executed"], 0);

    let st = debug(&mut s, json!({"action": "step"}));
    assert_eq!(st["current"]["step"], "2");
    assert_eq!(st["current"]["params"]["cell"], "D2", "tokens resolve for the next step");
    assert_eq!(st["lastParams"], json!({"cell": "E1", "input": "start"}));
    assert_eq!(st["changed"], json!(["Sheet1!E1"]));
    assert_eq!(v(&s, "E1").display(), "start");

    // Other edits wait; selecting cells and queries don't.
    let r = s.execute("cell.set", json!({"cell": "A1", "input": "x"}));
    assert!(matches!(&r, Err(EngineError::Disabled(_, why)) if why == "Finish or stop the macro first"), "{r:?}");
    assert!(s.execute("macro.run", json!({"name": "Walk"})).is_err());
    assert!(s.execute("edit.undo", json!({})).is_err());
    s.execute("selection.set", json!({"range": "A1"})).unwrap();
    assert!(s.execute("macro.debug", json!({"name": "Walk", "action": "start"})).unwrap_err().to_string().contains("is being debugged"));

    // Continue stops at the breakpoint inside the Repeat, once per row.
    let st = debug(&mut s, json!({"action": "continue"}));
    assert_eq!((st["current"]["step"].as_str(), st["atBreakpoint"].as_bool()), (Some("3.1"), Some(true)));
    assert_eq!(st["current"]["iteration"], json!({"at": 1, "of": 3}));
    assert_eq!(st["current"]["params"]["cell"], "D3");
    let st = debug(&mut s, json!({"action": "continue"}));
    assert_eq!(st["current"]["iteration"], json!({"at": 2, "of": 3}));
    // Run To stops at a breakpoint on the way, then at its step.
    let st = debug(&mut s, json!({"action": "runTo", "step": "4", "breakpoints": []}));
    assert_eq!((st["current"]["step"].as_str(), st["atBreakpoint"].as_bool()), (Some("3.1"), Some(true)));
    assert_eq!(st["current"]["iteration"], json!({"at": 3, "of": 3}));
    let st = debug(&mut s, json!({"action": "runTo", "step": "4"}));
    assert_eq!((st["current"]["step"].as_str(), st["atBreakpoint"].as_bool()), (Some("4"), Some(false)));
    assert_eq!((v(&s, "D4").display(), v(&s, "D5").display()), ("seen".into(), "seen".into()));
    // Breakpoints from the call: none left before the end.
    let st = debug(&mut s, json!({"action": "continue"}));
    assert_eq!(st["state"], "finished");
    assert_eq!(st["executed"], 7);
    assert!(st["changed"].as_array().unwrap().contains(&json!("Sheet1!E9")));
    assert_eq!(v(&s, "E9").display(), "end");
    let after = undo_labels(&s);
    assert_eq!(after.len(), undo.len() + 1, "{after:?}");
    assert_eq!(after.last().unwrap(), "Run Macro: Walk");
    // Edits work again; one undo takes the whole run back.
    s.execute("edit.undo", json!({})).unwrap();
    assert!(v(&s, "E1").is_empty() && v(&s, "D3").is_empty() && v(&s, "E9").is_empty());
    set(&mut s, "A1", "free");
}

#[test]
fn a_breakpoint_list_can_replace_the_steps_own() {
    let mut s = s();
    stepper(&mut s);
    // Only step 4 stops the run: the flag on 3.1 is ignored.
    let st = debug(&mut s, json!({"name": "Walk", "inputs": "B2:C5", "breakpoints": ["4"], "breakpointsOnly": true}));
    assert_eq!(st["current"]["step"], "1");
    let st = debug(&mut s, json!({"action": "continue"}));
    assert_eq!((st["current"]["step"].as_str(), st["atBreakpoint"].as_bool()), (Some("4"), Some(true)));
    debug(&mut s, json!({"action": "stop"}));
    // Clearing the list mid-run (an editor removing its last breakpoint) runs to the end.
    debug(&mut s, json!({"name": "Walk", "inputs": "B2:C5", "breakpoints": ["3.1"], "breakpointsOnly": true}));
    let st = debug(&mut s, json!({"action": "continue"}));
    assert_eq!(st["current"]["step"], "3.1");
    let st = debug(&mut s, json!({"action": "continue", "breakpoints": [], "breakpointsOnly": true}));
    assert_eq!(st["state"], "finished");
}

#[test]
fn stopping_the_debugger_keeps_or_rolls_back() {
    for keep in [false, true] {
        let mut s = s();
        stepper(&mut s);
        let (start, undo) = (s.doc().unwrap().wb.clone(), undo_labels(&s));
        debug(&mut s, json!({"name": "Walk", "inputs": "B2:C5", "breakpoints": ["2"]}));
        let st = debug(&mut s, json!({"action": "continue"}));
        assert_eq!((st["current"]["step"].as_str(), st["atBreakpoint"].as_bool()), (Some("2"), Some(true)));
        let st = debug(&mut s, json!({"action": "stop", "keep": keep}));
        assert_eq!((st["state"].as_str(), st["kept"].as_bool()), (Some("stopped"), Some(keep)));
        if keep {
            assert_eq!(v(&s, "E1").display(), "start");
            assert_eq!(undo_labels(&s).len(), undo.len() + 1);
            assert_eq!(st["changed"], json!(["Sheet1!E1"]));
        } else {
            assert!(Arc::ptr_eq(&s.doc().unwrap().wb, &start));
            assert_eq!(undo_labels(&s), undo);
        }
        set(&mut s, "A1", "free");
        assert!(s.execute("macro.debug", json!({"action": "step"})).unwrap_err().to_string().contains("No macro is being debugged"));
    }
}

#[test]
fn a_debugged_run_after_a_partial_footprint_stops_before_writing_over_data() {
    let mut s = s();
    set(&mut s, "A1", "1");
    set(&mut s, "C1", "=A1");
    set(&mut s, "B1", "keep");
    let solver = json!({"command": "data.solver", "params": {"objective": "C1", "variables": "A1", "constraints": ["A1 <= 5"], "method": "simplex", "saveModel": false}});
    let late = json!({"command": "cell.set", "params": {"cell": "B1", "input": "over"}});
    put(&mut s, json!({"name": "Late", "inputs": [{"name": "Input"}], "output": {"onConflict": "fail"}, "steps": [solver, late]}));
    let start = s.doc().unwrap().wb.clone();
    debug(&mut s, json!({"name": "Late", "action": "start", "inputs": "A1"}));
    debug(&mut s, json!({"action": "step"}));
    let st = debug(&mut s, json!({"action": "step"}));
    assert_eq!(st["state"], "failed", "{st}");
    assert!(st["message"].as_str().unwrap().contains("It would write over data (B1 on Sheet1)"), "{st}");
    assert_eq!(v(&s, "B1").display(), "keep");
    assert!(Arc::ptr_eq(&s.doc().unwrap().wb, &start));
}

#[test]
fn a_failing_step_ends_the_debugged_run_and_rolls_it_back() {
    let mut s = s();
    data(&mut s);
    put(
        &mut s,
        json!({"name": "Breaks", "output": {"onConflict": "overwrite"}, "steps": [
            {"command": "cell.set", "params": {"cell": "A1", "input": "one"}},
            {"command": "cell.set", "params": {"cell": "Nowhere!A2", "input": "two"}},
            {"type": "stop", "message": "never"}
        ]}),
    );
    let start = s.doc().unwrap().wb.clone();
    debug(&mut s, json!({"name": "Breaks"}));
    assert!(s.execute("macro.debug", json!({"action": "runTo", "step": "9"})).unwrap_err().to_string().contains("has no step 9"));
    assert!(s.execute("macro.debug", json!({"action": "leap"})).is_err());
    debug(&mut s, json!({"action": "step"}));
    let st = debug(&mut s, json!({"action": "step"}));
    assert_eq!(st["state"], "failed");
    assert_eq!(st["error"]["step"], "2");
    assert!(st["message"].as_str().unwrap().starts_with("Step 2 (cell.set):"), "{st}");
    assert!(Arc::ptr_eq(&s.doc().unwrap().wb, &start));
    set(&mut s, "B1", "free again");
    // A run that ends at once (a Stop first) finishes on start.
    put(&mut s, json!({"name": "Quick", "steps": [{"type": "stop", "message": "bye"}]}));
    let st = debug(&mut s, json!({"name": "Quick"}));
    assert_eq!(st["state"], "paused");
    let st = debug(&mut s, json!({"action": "step"}));
    assert_eq!((st["state"].as_str(), st["stopped"].as_str()), (Some("stopped"), Some("bye")));
    put(&mut s, json!({"name": "Empty", "steps": []}));
    assert_eq!(debug(&mut s, json!({"name": "Empty"}))["state"], "finished");
}

#[test]
fn only_the_workbooks_a_debugged_run_touched_wait() {
    let mut s = s();
    stepper(&mut s);
    debug(&mut s, json!({"name": "Walk", "inputs": "B2:C5"}));
    debug(&mut s, json!({"action": "step"}));
    // Another workbook stays editable, macros included.
    s.new_workbook();
    set(&mut s, "A1", "elsewhere");
    put(
        &mut s,
        json!({"name": "Here", "output": {"onConflict": "overwrite"}, "steps": [{"command": "cell.set", "params": {"cell": "A2", "input": "ran"}}]}),
    );
    s.execute("macro.run", json!({"name": "Here"})).unwrap();
    assert_eq!(v(&s, "A2").display(), "ran");
    s.execute("edit.undo", json!({})).unwrap();
    assert!(v(&s, "A2").is_empty());
    // The debugged one waits.
    s.set_active(0);
    let r = s.execute("cell.set", json!({"cell": "A1", "input": "x"}));
    assert!(matches!(&r, Err(EngineError::Disabled(_, why)) if why == "Finish or stop the macro first"), "{r:?}");
    // The run carries on in its own workbook whichever one is in front.
    s.set_active(1);
    let st = debug(&mut s, json!({"action": "continue", "breakpoints": []}));
    assert_eq!(st["state"], "paused");
    assert_eq!(s.active_index(), 0);
    debug(&mut s, json!({"action": "stop"}));
    set(&mut s, "A1", "free");
}
