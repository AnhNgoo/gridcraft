//! Form dialogs open ready to type, as in Excel: Alt+H, O, W then a number and Enter sets the
//! column width without clicking into the field first.

use egui::{Event, Key, vec2};
use egui_kittest::kittest::Queryable;
use gridcraft_engine::Session;
use gridcraft_engine::core::CellRef;
use gridcraft_ui_egui::SheetApp;
use serde_json::json;

fn harness() -> egui_kittest::Harness<'static, SheetApp> {
    let mut session = Session::new();
    session.new_workbook();
    session.execute("selection.set", json!({"cell": "C3"})).unwrap();
    let mut h = crate::common::builder().with_size(vec2(1200.0, 800.0)).build_ui_state(
        |ui, app: &mut SheetApp| {
            app.logic(&ui.ctx().clone());
            app.ui(ui);
        },
        crate::common::app(session),
    );
    h.run_steps(6);
    h
}

fn type_and_enter(h: &mut egui_kittest::Harness<'static, SheetApp>, text: &str) {
    for c in text.chars() {
        h.input_mut().events.push(Event::Text(c.to_string()));
        h.run_steps(1);
    }
    h.key_press(Key::Enter);
    h.run_steps(6);
}

#[test]
fn column_width_opens_with_the_width_ready_to_type() {
    let mut h = harness();
    h.state_mut().open_dialog("columnWidth", json!({}));
    h.run_steps(6);
    type_and_enter(&mut h, "12");
    assert!(h.state().dialog.is_none(), "Enter confirmed the dialog");
    let chars = {
        let d = h.state().session.active().unwrap();
        let sheet = d.wb.active().unwrap();
        gridcraft_engine::cmd::format::points_to_chars(sheet.col_width(CellRef::parse("C3").unwrap().col) as f64)
    };
    assert!((chars - 12.0).abs() < 0.2, "column C is 12 characters wide, got {chars}");
}

#[test]
fn row_height_opens_with_the_height_ready_to_type() {
    let mut h = harness();
    h.state_mut().open_dialog("rowHeight", json!({}));
    h.run_steps(6);
    let shown = h.state().dialog.as_ref().and_then(|d| d.values.get("points")).and_then(|v| v.as_f64());
    assert!(shown.is_some_and(|v| (v - 15.0).abs() < 0.01), "a default row opens at 15 points, got {shown:?}");
    type_and_enter(&mut h, "30");
    assert!(h.state().dialog.is_none(), "Enter confirmed the dialog");
    let height = {
        let d = h.state().session.active().unwrap();
        d.wb.active().unwrap().row_height(CellRef::parse("C3").unwrap().row)
    };
    assert!((height as f64 - 40.0).abs() < 0.6, "row 3 is 30 points (40 pixels) high, got {height}");
}

#[test]
fn command_search_arrow_and_page_keys_move_the_selection_and_keep_the_search_focused() {
    let mut h = harness();
    h.state_mut().open_dialog("commandSearch", json!({}));
    h.run_steps(6);
    h.input_mut().events.push(Event::Text("a".into()));
    h.run_steps(3);
    assert_eq!(h.state().dialog.as_ref().map(|d| d.list_index), Some(0));
    for _ in 0..2 {
        h.key_press(Key::ArrowDown);
        h.run_steps(2);
    }
    assert_eq!(h.state().dialog.as_ref().map(|d| d.list_index), Some(2), "two ArrowDown presses");
    h.key_press(Key::PageDown);
    h.run_steps(2);
    let after_page = h.state().dialog.as_ref().map(|d| d.list_index).unwrap_or(0);
    assert!(after_page > 2, "PageDown moved past the third row, got {after_page}");
    assert!(h.ctx.memory(|m| m.focused()).is_some(), "the search field kept focus");
}

/// Checks platform-specific labels for the Bold and Filter shortcuts.
#[test]
fn command_search_shows_shortcuts_for_current_platform() {
    let mut h = harness();
    h.state_mut().open_dialog("commandSearch", json!({}));
    h.run_steps(6);

    h.state_mut().dialog.as_mut().unwrap().search = "bold".into();
    h.run_steps(3);

    let bold = if h.ctx.os() == egui::os::OperatingSystem::Mac { "Bold  (⌘B)" } else { "Bold  (Ctrl+B)" };
    h.get_by_label_contains(bold);

    h.state_mut().dialog.as_mut().unwrap().search = "filter".into();
    h.run_steps(3);

    let filter = if h.ctx.os() == egui::os::OperatingSystem::Mac { "Filter  (⌘⇧F)" } else { "Filter  (Ctrl+Shift+L)" };
    h.get_by_label_contains(filter);
}

/// Checks that Paste Special shows a usable shortcut after copying a cell.
#[test]
fn command_search_shows_paste_special_shortcut() {
    let mut h = harness();

    h.state_mut().session.execute("cell.set", json!({"cell": "C3", "input": "copied"})).unwrap();
    h.state_mut().session.execute("edit.copy", json!({"range": "C3"})).unwrap();

    h.state_mut().open_dialog("commandSearch", json!({}));
    h.run_steps(6);

    h.state_mut().dialog.as_mut().unwrap().search = "pasteSpecial".into();
    h.run_steps(3);

    let expected = if h.ctx.os() == egui::os::OperatingSystem::Mac { "Paste Special…  (Ctrl+Cmd+V)" } else { "Paste Special…  (Ctrl+Alt+V)" };

    h.get_by_label_contains(expected);
}

/// Checks both OS mappings on one host, including commands with conflicting keys.
#[test]
fn command_search_uses_runtime_os_and_verified_shortcuts() {
    use egui::os::OperatingSystem;

    for (os, bold, filter, flash, paste, left, center) in [
        (
            OperatingSystem::Windows,
            "Bold  (Ctrl+B)",
            "Filter  (Ctrl+Shift+L)",
            "Flash Fill  (Ctrl+E)",
            "Paste Special…  (Ctrl+Alt+V)",
            "Align Left    Home",
            "Center    Home",
        ),
        (
            OperatingSystem::Mac,
            "Bold  (⌘B)",
            "Filter  (⌘⇧F)",
            "Flash Fill  (Control+E)",
            "Paste Special…  (Ctrl+Cmd+V)",
            "Align Left  (⌘L)",
            "Center  (⌘E)",
        ),
    ] {
        let mut h = harness();
        h.ctx.set_os(os);
        h.run_steps(2);

        h.state_mut().session.execute("cell.set", json!({"cell": "C3", "input": "copied"})).unwrap();
        h.state_mut().session.execute("edit.copy", json!({"range": "C3"})).unwrap();

        h.state_mut().open_dialog("commandSearch", json!({}));
        h.run_steps(6);

        for (query, expected) in [
            ("bold", bold),
            ("filter", filter),
            ("edit.flashFill", flash),
            ("edit.pasteSpecial", paste),
            ("home.alignLeft", left),
            ("home.alignCenter", center),
        ] {
            h.state_mut().dialog.as_mut().unwrap().search = query.into();
            h.run_steps(3);
            h.get_by_label_contains(expected);
        }
    }
}
