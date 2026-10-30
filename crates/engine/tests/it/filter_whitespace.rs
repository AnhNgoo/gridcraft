use gridcraft_engine::Session;
use serde_json::json;

#[test]
fn autofilter_ignores_surrounding_spaces_2911() {
    let mut s = Session::new();
    s.new_workbook();

    s.execute(
        "range.setValues",
        json!({
            "range": "A1",
            "parse": false,
            "values": [
                ["Name"],
                ["Card"],
                ["Card "],
                [" Card"],
                ["  Card  "],
                ["card"],
                ["CARD"],
                ["Cardx"],
                ["Ca rd"],
                [" "],
                [""]
            ]
        }),
    )
    .unwrap();

    s.execute("data.filter", json!({"range": "A1:A11"})).unwrap();

    for value in ["Card", " Card", "Card "] {
        let result = s.execute("data.filterBy", json!({"column": "A", "values": [value]})).unwrap();

        assert_eq!(result["hiddenRows"], json!(4), "{value:?}");

        let sh = s.doc().unwrap().wb.active().unwrap();

        for row in 1..=6 {
            assert!(!sh.is_row_hidden(row), "row {row} should be visible for {value:?}");
        }

        for row in 7..=10 {
            assert!(sh.is_row_hidden(row), "row {row} should be hidden for {value:?}");
        }
    }

    for value in ["Card", " Card", "Card "] {
        let result = s
            .execute(
                "data.filterBy",
                json!({
                    "column": "A",
                    "custom": {"op": "=", "value": value}
                }),
            )
            .unwrap();

        assert_eq!(result["hiddenRows"], json!(4), "{value:?}");

        let sh = s.doc().unwrap().wb.active().unwrap();

        for row in 1..=6 {
            assert!(!sh.is_row_hidden(row), "{value:?}: row {row}");
        }

        for row in 7..=10 {
            assert!(sh.is_row_hidden(row), "{value:?}: row {row}");
        }
    }
}
