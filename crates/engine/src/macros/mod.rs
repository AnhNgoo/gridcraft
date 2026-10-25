//! GridCraft macros: typed, declarative command lists, not VBA. The model is plain data in
//! `gridcraft_model::Macro`; this module reads and writes it and keeps it within its limits.
//!
//! Storage: the JSON format keeps `Workbook::macros` as a field. In xlsx each macro is one item of
//! GridCraft's hidden workbook data ([`crate::cmd::stash`]), `_gc_macro_<n>_<k>`, written when the
//! file is saved and taken back out of the names when it is opened, so the names never linger in
//! the open document. Scripts made with Automate › New Script or Record Actions are a separate,
//! older feature with their own storage; macros neither read nor change them.

use std::collections::{BTreeMap, HashMap, HashSet};

use gridcraft_model::{DefinedName, Macro, Workbook};
use serde_json::{Value as Json, json};

use crate::cmd::stash;

pub mod policy;
pub mod resolve;
pub mod run;

/// The hidden-data key prefix of a stored macro (`macro_<n>`).
const MACRO_KEY: &str = "macro_";

/// Reads a macro from JSON, as `macro.save` and files give it. A command step may also use the
/// short form `{"command": "cell.set", "params": {…}}`.
pub fn parse_macro(v: &Json) -> Result<Macro, String> {
    // A newer format is refused before its steps are read, so the message says why.
    if let Some(found) = v.get("v").and_then(Json::as_u64) {
        gridcraft_model::check_version(found)?;
    }
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
    check(&m)?;
    Ok(m)
}

/// Every check a macro must pass before it is kept in a workbook: the model's limits
/// ([`Macro::check_limits`]) and the size an xlsx file can hold. Every path that adds a macro,
/// from JSON, from a file or built in code, goes through this one check.
pub fn check(m: &Macro) -> Result<(), String> {
    m.check_limits()?;
    check_size(m)
}

/// Refuses a macro too large to keep in an xlsx file ([`stash::MAX_ITEM_CHARS`] characters of
/// JSON), so a macro's size never makes a save fail. (A clashing visible name doesn't either:
/// [`with_names`] stores the macro under another index.)
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
    for st in list {
        let Some(o) = st.as_object_mut() else { return Err("Every step must be an object.".into()) };
        if !o.contains_key("type")
            && let Some(id) = o.remove("command")
        {
            o.insert("type".into(), json!("command"));
            o.insert("id".into(), id);
        }
        // A step with a body is a Repeat or an If: one at the top is nested 1 deep.
        let nested = ["body", "then", "else"].iter().any(|key| o.contains_key(*key));
        if nested && level > gridcraft_model::MAX_MACRO_DEPTH {
            return Err(format!("Repeat and If steps can nest at most {} deep.", gridcraft_model::MAX_MACRO_DEPTH));
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

/// A macro's content as canonical JSON text (object keys sorted): approval to run a macro holds
/// until the macro changes. It is the text itself rather than a hash, so no two different macros
/// share it.
pub fn fingerprint(m: &Macro) -> String {
    macro_json(m).to_string()
}

/// A copy of `wb` for xlsx with each macro in hidden names (`_gc_macro_<n>_<k>`) instead of the
/// field, or `None` when there is nothing to change. The macros keep their order; the index `n`
/// is this build's choice, so it steps around any a person's own visible name already uses.
pub fn with_names(wb: &Workbook) -> crate::Result<Option<Workbook>> {
    // Only the numbered items are this build's: a hidden `_gc_macro_<x>_<k>` name with some
    // other key stays, as `load` leaves it.
    let stored = |n: &DefinedName| macro_item(n).is_some_and(|(_, i)| i.is_some());
    if wb.macros.is_empty() && wb.macros_unread.is_empty() && !wb.names.iter().any(stored) {
        return Ok(None);
    }
    // A macro that skipped `check` (built in code) fails the save, as the JSON save does, rather
    // than being written and dropped the next time the file opens.
    for m in wb.macros.iter() {
        check(m).map_err(crate::EngineError::Other)?;
    }
    let mut out = wb.clone();
    // One pass drops the items of the last save, and one writes the macros (a file can hold a
    // thousand macros and many thousands of names).
    out.names.retain(|n| !stored(n));
    // A visible workbook-scope name of the storage shape is someone's own, and `set_many` won't
    // write a hidden one of the same name beside it. Such a name (`_GC_MACRO_1_0`, or one a
    // person defined after the file opened) blocks its item's key, in any chunk, so the macros
    // take the free indices from 0 up. The keys compare as the stash reads them, so this skips
    // exactly what `set_many` would refuse, and no arithmetic depends on a number from the file.
    let blocked: HashSet<String> = out.names.iter().filter(|n| !n.hidden && n.scope.is_none()).filter_map(|n| stash::item_key(&n.name)).collect();
    let free = (0..=u64::MAX).map(|i| format!("{MACRO_KEY}{i}")).filter(|key| !blocked.contains(key));
    // Macros in a newer format follow the readable ones, written back as the file held them.
    let texts = wb.macros.iter().map(|m| macro_json(m).to_string()).chain(wb.macros_unread.iter().map(Json::to_string));
    let items: Vec<(String, String)> = texts.zip(free).map(|(text, key)| (key, text)).collect();
    stash::set_many(&mut out, &items)?;
    // An empty list of its own, not a copy of the shared one cleared.
    out.macros = gridcraft_model::Macros::default();
    out.macros_unread = std::sync::Arc::default();
    Ok(Some(out))
}

/// The JSON format's bytes for `wb`: the workbook, with the macros in a newer format put back into
/// the `macros` list as the file held them.
pub fn json_bytes(wb: &Workbook) -> crate::Result<Vec<u8>> {
    // Every path that adds a macro checks it, so this refuses only one that skipped the check, as
    // the xlsx save does, instead of writing a macro the next open would drop.
    for m in &wb.macros {
        check(m).map_err(crate::EngineError::Other)?;
    }
    let bytes = if wb.macros_unread.is_empty() {
        serde_json::to_vec_pretty(wb)
    } else {
        serde_json::to_value(wb).and_then(|mut v| {
            if let Some(list) = v.as_object_mut().map(|o| o.entry("macros").or_insert_with(|| Json::Array(vec![]))).and_then(Json::as_array_mut) {
                list.extend(wb.macros_unread.iter().cloned());
            }
            serde_json::to_vec_pretty(&v)
        })
    };
    bytes.map_err(|e| crate::EngineError::Other(e.to_string()))
}

/// Whether `v` is a macro in a newer format than this build reads.
fn is_newer(v: &Json) -> bool {
    v.get("v").and_then(Json::as_u64).is_some_and(|found| found > u64::from(gridcraft_model::MACRO_VERSION))
}

/// Keeps a macro in a newer format unread, if it is small enough to save back
/// ([`stash::MAX_ITEM_CHARS`] characters of JSON, the size every xlsx save can write).
fn keep_unread(unread: &mut Vec<Json>, v: Json, bad: &mut usize) {
    if v.to_string().chars().count() <= stash::MAX_ITEM_CHARS {
        unread.push(v);
    } else {
        *bad = bad.saturating_add(1);
    }
}

/// After a file is opened: moves macros out of hidden names into `Workbook::macros`, keeps
/// macros in a newer format unread (`Workbook::macros_unread`), and drops macros that are damaged
/// or over the limits. Returns a warning for what was kept unread and one for what was dropped.
pub fn load(wb: &mut Workbook) -> Vec<String> {
    let mut warnings = Vec::new();
    let mut bad = 0usize;
    let mut unread = Vec::new();
    // Entries of the JSON format's field, each read on its own so a damaged one drops alone.
    let read = std::mem::take(&mut wb.macros_read);
    bad = bad.saturating_add(read.len().saturating_sub(gridcraft_model::MAX_MACROS));
    for v in read.into_iter().take(gridcraft_model::MAX_MACROS) {
        if is_newer(&v) {
            keep_unread(&mut unread, v, &mut bad);
            continue;
        }
        match parse_macro(&v) {
            Ok(m) => wb.macros.push(m),
            Err(_) => bad += 1,
        }
    }
    // One pass takes the hidden macro names out of the workbook, grouped by macro in saved order.
    let mut stored: BTreeMap<(u64, String), Vec<DefinedName>> = BTreeMap::new();
    let mut kept = Vec::new();
    for n in std::mem::take(&mut wb.names) {
        match macro_item(&n) {
            None => kept.push(n),
            Some((key, Some(i))) => stored.entry((i, key)).or_default().push(n),
            // A `macro_` item that isn't numbered isn't one this build wrote: it stays as it is,
            // unread, and a save keeps it.
            Some((_, None)) => kept.push(n),
        }
    }
    wb.names = kept;
    // Past the macro limit, the rest are counted without being read.
    bad = bad.saturating_add(stored.len().saturating_sub(gridcraft_model::MAX_MACROS));
    let mut found = Vec::new();
    let mut item = Workbook::new();
    for ((_, key), names) in stored.into_iter().take(gridcraft_model::MAX_MACROS) {
        item.names = names;
        let v = stash::get_json(&item, &key);
        if let Some(v) = v.as_ref().filter(|v| is_newer(v)) {
            keep_unread(&mut unread, v.clone(), &mut bad);
            continue;
        }
        match v.ok_or_else(|| "unreadable".to_string()).and_then(|v| parse_macro(&v)) {
            Ok(m) => found.push(m),
            Err(_) => bad += 1,
        }
    }
    // All of them go through the same limits, and a clashing name gets a suffix.
    let mut all = std::mem::take(&mut wb.macros);
    all.extend(found);
    let mut taken = Taken::default();
    for m in all {
        if wb.macros.len() >= gridcraft_model::MAX_MACROS {
            bad += 1;
            continue;
        }
        let mut m = m;
        let checked = m.check_limits().and_then(|()| {
            m.name = taken.unique(m.name.trim());
            // The macro as it is kept, renamed, must still pass, or the next save would lose it.
            check(&m)
        });
        match checked {
            Ok(()) => {
                taken.insert(&m.name);
                wb.macros.push(m);
            }
            Err(_) => bad += 1,
        }
    }
    // Readable and unread macros together stay within the limit, the readable ones first.
    let room = gridcraft_model::MAX_MACROS.saturating_sub(wb.macros.len());
    bad = bad.saturating_add(unread.len().saturating_sub(room));
    unread.truncate(room);
    if !unread.is_empty() {
        let one = unread.len() == 1;
        warnings.push(format!(
            "{} macro{} in the workbook {} made by a newer version of GridCraft: this version can't show or run {}, and saving keeps {} unchanged.",
            unread.len(),
            if one { "" } else { "s" },
            if one { "was" } else { "were" },
            if one { "it" } else { "them" },
            if one { "it" } else { "them" },
        ));
    }
    wb.macros_unread = std::sync::Arc::new(unread);
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

/// The item key (`macro_<n>`, lower-cased) of a hidden macro name, and `n` when it is a number.
fn macro_item(n: &DefinedName) -> Option<(String, Option<u64>)> {
    // Only a hidden workbook-scope name is macro storage: a person's own visible name that
    // happens to look like one stays a name.
    let key = stash::item_key(&n.name).filter(|k| n.hidden && n.scope.is_none() && k.starts_with(MACRO_KEY))?;
    let i = key.strip_prefix(MACRO_KEY).and_then(|i| i.parse().ok());
    Some((key, i))
}

/// `name`, or `name (2)`, `name (3)`… if a macro already has it. A free name comes back as it is;
/// a renamed one is shortened to make room for the suffix, so it stays within
/// [`gridcraft_model::MAX_MACRO_NAME`] characters. The result still goes through [`check`].
pub fn unique_name(wb: &Workbook, name: &str) -> String {
    let mut taken = Taken::default();
    for m in &wb.macros {
        taken.insert(&m.name);
    }
    taken.unique(name)
}

/// Macro names in use, compared as [`find`] compares them, and the next suffix to try for each
/// shortened name and suffix width, so naming many clashing macros takes near-linear work.
#[derive(Default)]
struct Taken {
    names: HashSet<String>,
    next: HashMap<(String, usize), u64>,
    /// Name lookups made, for tests.
    probes: usize,
}

impl Taken {
    fn key(name: &str) -> String {
        name.trim().to_ascii_lowercase()
    }

    fn insert(&mut self, name: &str) {
        self.names.insert(Taken::key(name));
    }

    fn contains(&mut self, name: &str) -> bool {
        self.probes = self.probes.saturating_add(1);
        self.names.contains(&Taken::key(name))
    }

    /// The first of `name`, `name (2)`, `name (3)`… not in use.
    fn unique(&mut self, name: &str) -> String {
        let name = if name.is_empty() { "Macro" } else { name };
        if !self.contains(name) {
            return name.to_string();
        }
        let mut n: u64 = 2;
        loop {
            let suffix = format!(" ({n})");
            let keep = gridcraft_model::MAX_MACRO_NAME.saturating_sub(suffix.chars().count());
            let base: String = name.chars().take(keep).collect();
            let base = base.trim_end();
            // Every candidate for this base and suffix width below `next` is in use already
            // (names are only added), so the search goes on from there.
            let key = (Taken::key(base), suffix.len());
            let next = self.next.get(&key).copied().unwrap_or(n);
            if next > n {
                n = next;
                continue;
            }
            let Some(after) = n.checked_add(1) else { return name.to_string() };
            self.next.insert(key, after);
            let candidate = format!("{base}{suffix}");
            if !self.contains(&candidate) {
                return candidate;
            }
            n = after;
        }
    }
}

#[cfg(test)]
mod tests;
