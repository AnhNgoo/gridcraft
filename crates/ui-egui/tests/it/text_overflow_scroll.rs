//! Overflowing text stays visible in its empty neighbours when its own cell is scrolled out (#1613).

use egui::{Rect, Shape};
use gridcraft_engine::Session;
use gridcraft_engine::core::CellRef;
use gridcraft_ui_egui::{SheetApp, grid::Geo};
use serde_json::json;

type Harness = egui_kittest::Harness<'static, SheetApp>;

const LEFT: &str = "This is a very long left-aligned text that overflows into many empty neighbouring cells";
const RIGHT: &str = "This is a very long right-aligned text that overflows into many empty neighbouring cells";

fn harness(setup: impl FnOnce(&mut Session)) -> Harness {
    let mut s = Session::new();
    s.new_workbook();
    setup(&mut s);
    s.execute("selection.set", json!({"range": "A20"})).unwrap();
    let mut h = crate::common::builder().with_size(egui::vec2(1200.0, 800.0)).build_ui_state(
        |ui, app: &mut SheetApp| {
            app.logic(ui.ctx());
            app.ui(ui);
        },
        crate::common::app(s),
    );
    h.run_steps(4);
    h
}

/// Scroll so that `col` is the first column of the scrolled area.
fn scroll_to_col(h: &mut Harness, col: u32) {
    let app = h.state_mut();
    let sheet = app.session.active().unwrap().wb.active().unwrap();
    let fc = sheet.freeze.map(|f| f.1).unwrap_or(0);
    let x = (sheet.col_left(col) - sheet.col_left(fc)) as f32;
    app.view_mut().unwrap().scroll.x = x;
    h.run_steps(4);
}

/// Clip rects of the painted text shapes whose text is `text`.
fn text_clips(h: &Harness, text: &str) -> Vec<Rect> {
    h.output()
        .shapes
        .iter()
        .filter_map(|s| match &s.shape {
            Shape::Text(t) if t.galley.job.text == text => Some(s.clip_rect),
            _ => None,
        })
        .collect()
}

fn cell_rect(h: &Harness, cell: &str) -> Rect {
    let app = h.state();
    let sheet = app.session.active().unwrap().wb.active().unwrap();
    let geo = Geo::new(sheet, app.grid.rect.unwrap(), app.view().scroll);
    geo.cell_rect(sheet, CellRef::parse(cell).unwrap())
}

#[test]
fn overflow_text_stays_visible_when_source_cell_scrolls_out_left() {
    let mut h = harness(|s| {
        s.execute("cell.set", json!({"cell": "B1", "input": LEFT})).unwrap();
    });
    assert!(!text_clips(&h, LEFT).is_empty(), "B1 is drawn before scrolling");
    scroll_to_col(&mut h, 2);
    let c1 = cell_rect(&h, "C1");
    let clips = text_clips(&h, LEFT);
    assert!(clips.iter().any(|r| r.intersects(c1.shrink(1.0))), "B1's overflow is still drawn into C1 after scrolling: {clips:?}");
}

#[test]
fn right_aligned_overflow_stays_visible_when_source_cell_is_right_of_view() {
    let mut h = harness(|s| {
        s.execute("cell.set", json!({"cell": "Z1", "input": RIGHT})).unwrap();
        s.execute("home.formatCells", json!({"range": "Z1", "style": {"align": {"h": "Right"}}})).unwrap();
    });
    // Scroll so that Z1 starts just past the right edge of the cell area.
    {
        let app = h.state_mut();
        let sheet = app.session.active().unwrap().wb.active().unwrap();
        let geo = Geo::new(sheet, app.grid.rect.unwrap(), app.view().scroll);
        let x = sheet.col_left(25) as f32 - (geo.cells.width() - geo.frozen_w) / geo.z - 2.0;
        app.view_mut().unwrap().scroll.x = x;
    }
    h.run_steps(4);
    let (y1, z1, cells) = {
        let app = h.state();
        let sheet = app.session.active().unwrap().wb.active().unwrap();
        let geo = Geo::new(sheet, app.grid.rect.unwrap(), app.view().scroll);
        (geo.cell_rect(sheet, CellRef::parse("Y1").unwrap()), geo.cell_rect(sheet, CellRef::parse("Z1").unwrap()), geo.cells)
    };
    assert!(z1.left() >= cells.right(), "Z1 is out of view on the right ({z1:?} vs {cells:?})");
    let visible = y1.intersect(cells).shrink(1.0);
    let clips = text_clips(&h, RIGHT);
    assert!(clips.iter().any(|r| r.intersects(visible)), "Z1's overflow is drawn into Y1: {clips:?} vs {visible:?}");
}

#[test]
fn scrolled_overflow_does_not_draw_over_frozen_columns() {
    let mut h = harness(|s| {
        s.execute("cell.set", json!({"cell": "C1", "input": LEFT})).unwrap();
        s.execute("view.freezePanes", json!({"cell": "B2"})).unwrap();
    });
    scroll_to_col(&mut h, 4);
    let a1 = cell_rect(&h, "A1");
    let e1 = cell_rect(&h, "E1");
    let clips = text_clips(&h, LEFT);
    assert!(clips.iter().any(|r| r.intersects(e1.shrink(1.0))), "C1's overflow reaches E1: {clips:?}");
    assert!(clips.iter().all(|r| !r.intersects(a1.shrink(1.0))), "C1's overflow stays out of frozen column A: {clips:?}");
}

/// Issue #2752: fixed-height wrapped cells must not paint
/// a fraction of a text line against the next row's border.
#[test]
fn wrapped_text_in_fixed_height_row_shows_only_complete_lines() {
    const TEXT: &str = "Row 3 target. This is intentionally long text that must wrap \
         across many lines within a narrow column. It must never \
         paint over the next row.";

    let h = harness(|s| {
        s.execute("home.columnWidth", json!({"cols": "B:B", "chars": 12})).unwrap();

        s.execute("cell.set", json!({"cell": "B3", "input": TEXT})).unwrap();

        s.execute("home.wrapText", json!({"range": "B3", "on": true})).unwrap();

        s.execute("home.formatCells", json!({"range": "B3", "style": {"align": {"v": "Top"}}})).unwrap();

        // Apply the custom height after Wrap Text so AutoFit
        // cannot expand the row for this scenario.
        s.execute("home.rowHeight", json!({"rows": "3:3", "height": 62.67})).unwrap();
    });

    let cell = cell_rect(&h, "B3");

    let shapes: Vec<_> = h
        .output()
        .shapes
        .iter()
        .filter_map(|shape| match &shape.shape {
            Shape::Text(t) if t.galley.job.text == TEXT => Some((t, shape.clip_rect)),
            _ => None,
        })
        .collect();

    assert_eq!(shapes.len(), 1, "expected one painted wrapped text shape");

    let (text, clip) = shapes[0];

    assert!(text.galley.rows.len() > 4, "test must produce multiple wrapped lines");

    assert!(text.galley.size().y > cell.height(), "wrapped text must exceed the fixed row height");

    assert!(clip.top() >= cell.top() - 0.5 && clip.bottom() <= cell.bottom() + 0.5, "text clip must stay within B3: {clip:?} vs {cell:?}");

    let mut full_rows_visible = 0;

    for (index, row) in text.galley.rows.iter().enumerate() {
        let top = text.pos.y + row.rect().top();
        let bottom = text.pos.y + row.rect().bottom();
        let full_height = bottom - top;

        let visible_height = (bottom.min(clip.bottom()) - top.max(clip.top())).max(0.0);

        let partly_visible = visible_height > 0.5 && visible_height < full_height - 0.5;

        assert!(
            !partly_visible,
            "wrapped line {} is partially visible: \
             line=[{top:.2}, {bottom:.2}], clip={clip:?}",
            index + 1
        );

        if visible_height >= full_height - 0.5 {
            full_rows_visible += 1;
        }
    }

    assert!(full_rows_visible >= 2, "the clipping fix must still show complete text lines");
}

/// Issue #2752: a row shorter than a text line must not
/// render a fragment of wrapped text.
#[test]
fn wrapped_text_in_tiny_row_has_no_partial_lines() {
    const TEXT: &str = "This wrapped text should not be partly painted.";

    let h = harness(|s| {
        s.execute("home.columnWidth", json!({"cols": "B:B", "chars": 12})).unwrap();

        s.execute("cell.set", json!({"cell": "B3", "input": TEXT})).unwrap();

        s.execute("home.wrapText", json!({"range": "B3", "on": true})).unwrap();

        s.execute("home.formatCells", json!({"range": "B3", "style": {"align": {"v": "Top"}}})).unwrap();

        s.execute("home.rowHeight", json!({"rows": "3:3", "height": 5})).unwrap();
    });

    let cell = cell_rect(&h, "B3");

    let shapes: Vec<_> = h
        .output()
        .shapes
        .iter()
        .filter_map(|shape| match &shape.shape {
            Shape::Text(t) if t.galley.job.text == TEXT => Some((t, shape.clip_rect)),
            _ => None,
        })
        .collect();

    assert_eq!(shapes.len(), 1, "expected the wrapped text shape");

    let (text, clip) = shapes[0];

    assert!(text.galley.rows.len() > 1, "text must wrap into multiple lines");

    assert!(cell.height() < text.galley.rows[0].rect().height(), "test setup must produce a row shorter than one line");

    assert!(clip.top() >= cell.top() - 0.5 && clip.bottom() <= cell.bottom() + 0.5, "text clip must remain inside its cell");

    assert!(clip.height() <= 0.5, "no partial line should be visible: clip={clip:?}");
}

/// Issue #2752: scrolling through a wrapped cell may clip at
/// the viewport edge, but must not change its row-bottom limit.
#[test]
fn wrapped_text_clipping_survives_partial_vertical_scroll() {
    const TEXT: &str = "Wrapped viewport regression text with enough words \
         to occupy many lines in a narrow spreadsheet column.";

    let mut h = harness(|s| {
        s.execute("home.columnWidth", json!({"cols": "B:B", "chars": 12})).unwrap();
        s.execute("cell.set", json!({"cell": "B3", "input": TEXT})).unwrap();
        s.execute("home.wrapText", json!({"range": "B3", "on": true})).unwrap();
        s.execute("home.formatCells", json!({"range": "B3", "style": {"align": {"v": "Top"}}})).unwrap();
        s.execute("home.rowHeight", json!({"rows": "3:3", "height": 76})).unwrap();
    });

    let original_cell = cell_rect(&h, "B3");
    let original_clip = text_clips(&h, TEXT);
    assert_eq!(original_clip.len(), 1);

    {
        let app = h.state_mut();
        let sheet = app.session.active().unwrap().wb.active().unwrap();
        let scroll = sheet.row_top(2) as f32 + 8.0;
        app.view_mut().unwrap().scroll.y = scroll;
    }

    h.run_steps(4);

    let cell = cell_rect(&h, "B3");
    let app = h.state();
    let sheet = app.session.active().unwrap().wb.active().unwrap();
    let geo = Geo::new(sheet, app.grid.rect.unwrap(), app.view().scroll);
    let viewport = geo.cells;

    let shapes: Vec<_> = h
        .output()
        .shapes
        .iter()
        .filter_map(|shape| match &shape.shape {
            Shape::Text(t) if t.galley.job.text == TEXT => Some((t, shape.clip_rect)),
            _ => None,
        })
        .collect();

    assert_eq!(shapes.len(), 1, "B3 must remain visible after scrolling");

    let (text, clip) = shapes[0];

    assert!(clip.top() > cell.top() + 1.0, "test must scroll through B3: clip={clip:?}, cell={cell:?}");

    assert!(clip.top() >= viewport.top() - 0.5, "text must not paint above viewport: {clip:?} vs {viewport:?}");

    assert!(clip.bottom() <= cell.bottom() + 0.5, "text must stay within B3: {clip:?} vs {cell:?}");

    let full_line_bottoms: Vec<f32> =
        text.galley.rows.iter().map(|row| text.pos.y + row.rect().bottom()).filter(|bottom| *bottom <= cell.bottom() + 0.01).collect();

    let expected_bottom = full_line_bottoms.last().copied().expect("at least one wrapped line must fit B3");

    assert!(
        (clip.bottom() - expected_bottom).abs() <= 0.5,
        "scrolling must preserve complete-line clipping: \
         clip={clip:?}, expected_bottom={expected_bottom:.2}"
    );

    assert!(original_cell.height() > 0.0 && original_clip[0].height() > 0.0, "unscrolled B3 must display text");
}
