//! Macros: running, input checks, checking each step for writes over data, the policy and the
//! limits.

use std::sync::Arc;

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
    assert!(q.contains("Step 2 (cell.set): It would write over data (E3 on Sheet1)"), "{q}");
    assert!(Arc::ptr_eq(&s.doc().unwrap().wb, &start), "programmatic runs default to failing");
    let err = s.execute("macro.run", json!({"name": "Totals", "inputs": "B2:C5", "onConflict": "fail"})).unwrap_err().to_string();
    assert!(err.contains("Nothing was changed"), "{err}");
    assert!(Arc::ptr_eq(&s.doc().unwrap().wb, &start));
    // Shifting needs to know where the macro writes before it runs: it stops as fail does.
    for rule in ["shiftDown", "shiftRight"] {
        let err = s.execute("macro.run", json!({"name": "Totals", "inputs": "B2:C5", "onConflict": rule})).unwrap_err().to_string();
        assert!(err.contains("would write over data (E3 on Sheet1)"), "{rule}: {err}");
        assert!(Arc::ptr_eq(&s.doc().unwrap().wb, &start), "{rule}");
    }
    assert!(s.execute("macro.run", json!({"name": "Totals", "onConflict": "sideways"})).is_err());

    // The answer to the question overwrites.
    s.execute("macro.run", json!({"name": "Totals", "inputs": "B2:C5", "overwrite": true})).unwrap();
    assert_eq!(v(&s, "E3").display(), "400");
    assert_eq!(v(&s, "H1").display(), "400");
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
    // A run that checks each step keeps to the time limit too.
    put(
        &mut s,
        json!({"name": "Spin", "output": {"onConflict": "fail"}, "steps": [{"type": "repeat", "over": "A1:A400", "body": [
            {"type": "repeat", "over": "A1:A400", "body": [{"type": "if", "cell": "A1", "op": "empty"}]}
        ]}]}),
    );
    let err = s.execute("macro.run", json!({"name": "Spin", "timeLimit": 0.001})).unwrap_err().to_string();
    assert!(err.contains("ran longer than"), "{err}");
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
        // Without the permission nothing runs.
        let err = s.execute("macro.run", json!({"name": "Export"})).unwrap_err().to_string();
        assert!(err.contains("Step 1 (file.exportCsv) writes a file. Add file.exportCsv to the macro's permissions"), "{path}: {err}");
        // With it, the person approves first.
        put(&mut s, json!({"name": "Export", "inputs": [{"name": "Input"}], "steps": [step], "permissions": ["file.exportCsv"]}));
        let r = s.execute("macro.run", json!({"name": "Export"}));
        let Err(EngineError::Approve(q)) = &r else { panic!("{path}: {r:?}") };
        assert!(q.contains("file.exportCsv, which writes a file"), "{q}");
        assert_eq!(files(&dir), 0, "{path}");
        assert!(!std::path::Path::new("A1.csv").exists() && !std::path::Path::new(" .csv").exists());
    }
    // Once approved, the run writes the file.
    let step = json!({"command": "file.exportCsv", "params": {"path": in_dir}});
    put(&mut s, json!({"name": "Export", "inputs": [{"name": "Input"}], "steps": [step], "permissions": ["file.exportCsv"]}));
    s.execute("macro.run", json!({"name": "Export", "approve": true, "inputs": "B2"})).unwrap();
    assert_eq!(files(&dir), 1);
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
    s.execute("macro.run", json!({"name": "Import", "approve": true})).unwrap();
    assert_eq!(s.doc().unwrap().wb.sheets.len(), 2);
    s.execute("edit.undo", json!({})).unwrap();
    s.execute("macro.run", json!({"name": "Import"})).unwrap();
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
