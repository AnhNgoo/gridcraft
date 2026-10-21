//! GridCraft macros: typed, declarative command lists, not VBA. The model is plain data in
//! `gridcraft_model::Macro`; this module reads and writes it and keeps it within its limits.
//!
//! Storage: the JSON format keeps `Workbook::macros` as a field. In xlsx each macro is one item of
//! GridCraft's hidden workbook data ([`crate::cmd::stash`]), `_gc_macro_<n>_<k>`, written when the
//! file is saved and taken back out of the names when it is opened, so the names never linger in
//! the open document. Scripts made with Automate › New Script or Record Actions are a separate,
//! older feature with their own storage; macros neither read nor change them.

use gridcraft_model::{Macro, Workbook};
use serde_json::{Value as Json, json};

use crate::cmd::stash;

/// The hidden-data key prefix of a stored macro (`macro_<n>`).
const MACRO_KEY: &str = "macro_";

/// Reads a macro from JSON, as `macro.save` and files give it. A command step may also use the
/// short form `{"command": "cell.set", "params": {…}}`.
pub fn parse_macro(v: &Json) -> Result<Macro, String> {
    let mut v = v.clone();
    if let Some(steps) = v.get_mut("steps") {
        normalize_steps(steps, 1)?;
    }
    if v.get("v").is_none()
        && let Some(o) = v.as_object_mut()
    {
        o.insert("v".into(), json!(gridcraft_model::MACRO_VERSION));
    }
    let mut m: Macro = serde_json::from_value(v).map_err(|e| format!("This isn't a valid macro: {e}"))?;
    m.name = m.name.trim().to_string();
    m.check_limits()?;
    check_size(&m)?;
    Ok(m)
}

/// Refuses a macro too large to keep in an xlsx file ([`stash::MAX_ITEM_CHARS`] characters of
/// JSON), so saving a workbook never fails because of one of its macros.
fn check_size(m: &Macro) -> Result<(), String> {
    if macro_json(m).to_string().chars().count() > stash::MAX_ITEM_CHARS {
        return Err(format!("This macro is too large to keep in the workbook ({} characters at most).", stash::MAX_ITEM_CHARS));
    }
    Ok(())
}

/// Rewrites the short step form into the tagged one, in place, refusing nesting past the limit
/// before the steps are deserialized.
fn normalize_steps(steps: &mut Json, level: usize) -> Result<(), String> {
    let Some(list) = steps.as_array_mut() else { return Err("`steps` must be a list.".into()) };
    if level > gridcraft_model::MAX_MACRO_DEPTH {
        return Err(format!("Repeat and If steps can nest at most {} deep.", gridcraft_model::MAX_MACRO_DEPTH));
    }
    for st in list {
        let Some(o) = st.as_object_mut() else { return Err("Every step must be an object.".into()) };
        if !o.contains_key("type")
            && let Some(id) = o.remove("command")
        {
            o.insert("type".into(), json!("command"));
            o.insert("id".into(), id);
        }
        for key in ["body", "then", "else"] {
            if let Some(body) = o.get_mut(key) {
                normalize_steps(body, level + 1)?;
            }
        }
    }
    Ok(())
}

/// A macro as JSON (the shape `macro.get` returns and `macro.save` takes).
pub fn macro_json(m: &Macro) -> Json {
    serde_json::to_value(m).unwrap_or(Json::Null)
}

/// The index of the macro called `name` (case-insensitive).
pub fn find(wb: &Workbook, name: &str) -> Option<usize> {
    let name = name.trim();
    wb.macros.iter().position(|m| m.name.eq_ignore_ascii_case(name))
}

/// A copy of `wb` for xlsx with each macro in hidden names (`_gc_macro_<n>_<k>`) instead of the
/// field, or `None` when there is nothing to change.
pub fn with_names(wb: &Workbook) -> crate::Result<Option<Workbook>> {
    let stale = !stash::keys(wb, MACRO_KEY).is_empty();
    if wb.macros.is_empty() && !stale {
        return Ok(None);
    }
    let mut out = wb.clone();
    for key in stash::keys(&out, MACRO_KEY) {
        stash::remove(&mut out, &key);
    }
    for (i, m) in wb.macros.iter().enumerate() {
        stash::set(&mut out, &format!("{MACRO_KEY}{i}"), &macro_json(m).to_string())?;
    }
    out.macros.clear();
    Ok(Some(out))
}

/// After a file is opened: moves macros out of hidden names into `Workbook::macros` and drops
/// macros that are damaged or over the limits. Returns a warning for what was dropped.
pub fn load(wb: &mut Workbook) -> Vec<String> {
    let mut warnings = Vec::new();
    let mut bad = 0usize;
    // Stored macros, in their saved order.
    let mut keyed: Vec<(u64, String)> =
        stash::keys(wb, MACRO_KEY).into_iter().filter_map(|k| k.strip_prefix(MACRO_KEY).and_then(|n| n.parse().ok()).map(|n| (n, k))).collect();
    keyed.sort();
    let mut found = Vec::new();
    for (_, key) in &keyed {
        // Past the macro limit, the rest are counted without being read.
        if found.len() >= gridcraft_model::MAX_MACROS {
            bad += 1;
            continue;
        }
        match stash::get_json(wb, key).ok_or_else(|| "unreadable".to_string()).and_then(|v| parse_macro(&v)) {
            Ok(m) => found.push(m),
            Err(_) => bad += 1,
        }
    }
    for key in stash::keys(wb, MACRO_KEY) {
        stash::remove(wb, &key);
    }
    // Macros that came as a field (the JSON format) go through the same checks.
    let mut all = std::mem::take(&mut wb.macros);
    all.extend(found);
    for m in all {
        if wb.macros.len() >= gridcraft_model::MAX_MACROS {
            bad += 1;
            continue;
        }
        match m.check_limits().and_then(|()| check_size(&m)) {
            Ok(()) => {
                let mut m = m;
                m.name = unique_name(wb, m.name.trim());
                wb.macros.push(m);
            }
            Err(_) => bad += 1,
        }
    }
    if bad > 0 {
        warnings.push(format!(
            "{bad} macro{} in the workbook couldn't be opened: {} damaged or over the limits ({} macros, {} steps each, Repeat and If nested {} deep).",
            if bad == 1 { "" } else { "s" },
            if bad == 1 { "it is" } else { "they are" },
            gridcraft_model::MAX_MACROS,
            gridcraft_model::MAX_MACRO_STEPS,
            gridcraft_model::MAX_MACRO_DEPTH
        ));
    }
    warnings
}

/// `name`, or `name (2)`, `name (3)`… if a macro already has it.
pub fn unique_name(wb: &Workbook, name: &str) -> String {
    let name = if name.is_empty() { "Macro" } else { name };
    if find(wb, name).is_none() {
        return name.to_string();
    }
    (2..).map(|n| format!("{name} ({n})")).find(|n| find(wb, n).is_none()).unwrap_or_else(|| name.to_string())
}

#[cfg(test)]
mod tests;
