# Macros

> **Last updated:** 2026-10-25 · **Change:** minor (recording: kept steps, the input as it was, text that stays text, size limit, parameters refused; saving needs `replace: true` to replace; kept macros count and keep their names; earlier: recording macros and the management commands)

GridCraft macros are typed, declarative lists of commands, not VBA. Every step runs a GridCraft
command (the same ids the ribbon, CLI and MCP use) or one of three bounded control steps. A macro
declares the inputs it works on, runs as one undo step, rolls all workbook changes back on an
error (a file it already wrote stays), checks where its results will land before it writes them,
and does not touch files without permission.
Agents run macros with `macro.run` like any other command.

This page describes the engine and its commands. The macro interface (a Macros list, a recorder
button and a step editor) is not part of it yet.


## A macro

```json
{
  "name": "Totals",
  "description": "Adds a total next to the amounts",
  "inputs": [{"name": "Input", "header": true, "cols": [2, 2], "colTypes": ["text", "number"]}],
  "output": {"anchor": "Input.right(2).cell(1,2)", "onConflict": "ask"},
  "steps": [
    {"type": "command", "id": "cell.set", "params": {"cell": "Output", "input": "Total"}},
    {"type": "command", "id": "cell.set", "params": {"cell": "Output.offset(1,0)", "input": "=SUM({{Input.data.col(2)}})"}},
    {"type": "repeat", "over": "Input.data", "of": "rows", "max": 1000, "body": [
      {"type": "if", "cell": "Row.cell(1,2)", "op": "gt", "value": 100,
       "then": [{"type": "command", "id": "home.bold", "params": {"range": "Row"}}]}
    ]},
    {"type": "stop", "message": "Done."}
  ],
  "permissions": []
}
```

- **Steps**: `command` (`id`, `params`), `repeat` (each row or column of `over`, at most `max`
  times), `if` (`cell` compared with `value` by `eq`, `ne`, `gt`, `ge`, `lt`, `le`, `empty`,
  `notEmpty` or `contains`; `then` and `else` lists) and `stop` (`message`; `rollback: true`
  undoes the run). Every step can carry `enabled`, `breakpoint` and `note`. The short form
  `{"command": "cell.set", "params": {…}}` is accepted for a command step.
- **Inputs**: `kind` (`cell` or `range`), `rows` and `cols` as `[min, max]` (0 for no maximum;
  rows exclude the header), `header`, `colTypes` (`number`, `text`, `date`, `bool`, `any`),
  `nonEmpty` and `defaultRef`. These declare what a range must look like for the macro to work
  on it. An input's name is letters, digits and `_`, starting with a letter, and is neither a
  token of its own (`Output`, `Row`, `Column`, `Name`) nor something that reads as a cell
  reference (`Q1`), so a step written with plain A1 text keeps meaning the cell. The declaration
  is checked as a whole: neither `rows` nor `cols` may have a least above its most, a `cell` input
  has one row and one column (`[1, 1]`, or the default `[1, 0]`), no header and at most one column
  type, and no input lists more column types than it allows columns. A run binds each input
  to the range it is given, its default, or (for the first input) the selection, and refuses
  with messages that name the cells: "Input column 2 (Amount) must hold numbers; C4 holds text."
- **Limits**: 1,000 macros a workbook, 10,000 steps a macro, Repeat and If nested 4 deep, and 64
  inputs a macro. Names (of macros and inputs) and command ids are at most 255 characters;
  descriptions, step notes, Stop messages and If values (as JSON) 4,096; references and tokens
  (`output.anchor`, `defaultRef`, a Repeat's `over`, an If's `cell`) 1,024; a shortcut 64. A
  macro lists at most 256 permissions, each a command id, and an input at most 16,384 column
  types. One-line fields hold no line breaks or other control characters, and a command id is
  letters, digits, `.` and `_`. A command step's `params` are bounded only by the size of the
  whole macro (4,194,304 characters of JSON). A macro read from a file that breaks any of these is
  left out with a warning; it is never changed to fit. When a macro runs, a Repeat whose range has
  more rows or columns than its `max` is refused, and a run executes at most 100,000 steps (each
  pass through a Repeat counts), passes its commands at most 256 MB of parameters, all steps
  together, and takes at most 60 seconds, all three shared by the footprint dry runs and the run
  itself (`timeLimit` to change the time; the web build has no clock, so there only the step and
  parameter limits apply).

## Tokens

References in step parameters can be tokens, worked out when the step runs:

| Token | Means |
|---|---|
| `Input`, or another input's name | the range the input is bound to |
| `Output` | the output rule's anchor |
| `Row`, `Column` | the current row or column of the Repeat around the step |
| `Name:Total` | a defined name, or a table's data |

Methods narrow or move a range and chain: `col(2)` or `col("Amount")`, `row(1)`, `cols(2..3)`,
`rows(2..)`, `cell(r,c)` (relative to the top-left cell; `cell(0,1)` is the cell above),
`offset(r,c)`, `resize(h,w)`, `data` (without the header row), `below(n)` and `right(n)`.
`col("Amount")` looks the name up in an input's header row, directly or after `data`; on a range
without a header row it is refused. A defined name can hold periods: `Name:Sales.Q1` is the name
`Sales.Q1`, and `Name:Sales.Q1.cell(1,1)` its first cell, because the longest name or table that
exists is used and methods follow it.

Write a token as the whole value of a reference parameter (`cell`, `range`, `target`, keys ending
in `Cell` or `Range`…), as `{"$ref": "Input.col(2)"}` anywhere, or as `{{Input.col(2)}}` inside
any string, such as a formula; `{{{{` in a string is a literal `{{`, and `{"$text": "Output"}`
anywhere is the text as it is, never read as a token or for `{{…}}` (a defined name called
`Output`, or `Dear {{name}}`). Under the general keys that
other commands use for plain words (`to`, `from`, `at`, `area`, `source`, `target`,
`destination`, `location`), a bare `Row` or `Column` stays text (a replacement word, a pivot
area); write `Row.col(2)`, `{{Row}}` or `{"$ref": "Row"}` there. Plain A1 references stay
absolute; a `Name:` anchor follows inserted rows. A reference with a sheet name is never a token,
so `Output!A1` is a cell on a sheet called Output, even when a token or input has that name.
`Input.data` on a range that is only a header row, and `Name:` of a table with no data rows, are
refused rather than read as the header.

## Running

`macro.run {name, inputs?, onConflict?, overwrite?, approve?, timeLimit?}`:

1. **Policy.** File commands other than Properties, exports and printing (such as Open, Save
   and Close), app, window (including New Window, Hide and Unhide), automate and macro commands,
   and Undo and Redo, never run in a macro. File › Properties runs, and so do exports and
   printing (`file.exportPdf`, `exportCsv`, `exportHtml`, `print`, `printPreview`, `saveBytes`)
   that only produce bytes. Reading or writing a file (`data.getData`, `data.fromTextCsv`,
   `insert.picture` or `pageLayout.background` with a `path`, or an export or print with a
   `path`) needs the command in the macro's `permissions`. Any `path` other than `null` counts,
   including a blank one and a token, and the policy is checked again on the parameters as they
   are when the step runs. A macro that touches files, or that came with a workbook opened from
   a file, runs once the person approves it: the call fails with `approve: …` and runs with
   `approve: true`. Approval lasts for the session, until the macro changes. Every step other
   than an approved file step runs inside the file sandbox that scripts use, so no other
   parameter reaches a file either.
2. **Footprint.** Unless the conflict rule is `overwrite`, the macro first runs in dry-run mode on
   the real workbook, and everything is put back. Cells it would write outside its inputs that
   hold data, and sheets and objects it would delete, are conflicts, handled by `onConflict` (or
   the macro's `output.onConflict`): `ask` (the call fails with `confirm: …`; `overwrite: true`
   answers it), `fail`, `overwrite`, `shiftDown` or `shiftRight` (Insert Cells makes room, then
   the footprint is checked again). Steps that touch files, and Solver, end the dry run early;
   the footprint then says it is partial. Unless the rule is `overwrite`, the run also checks
   every step as it goes, since the footprint can miss a write: it is partial, or a value such as
   RAND or NOW sends the run another way than the dry run. A step that writes over cells, sheets
   or objects that held data when the run started stops the run and rolls the workbook back (a
   file already written stays): it asks (`confirm: …`) under `ask` and fails otherwise, since
   shifting can't make room that late. Moving cells that hold data, as inserting or deleting rows
   or cells above them does, counts as writing over them, in the footprint and in this check:
   both compare cells by place.
3. **Run.** One undo step, "Run Macro: Totals". Any error rolls the whole run back and names the
   step: `Step 3.1 (cell.set): …` (paths are 1-based; `2.1` is inside a Repeat, `4.then.1` inside
   an If). The message ends "Nothing was changed.", or names the files the run already wrote,
   which stay. An input the macro doesn't have, an input that isn't reference text, and
   `onConflict`, `overwrite`, `approve` or `timeLimit` of the wrong kind are refused rather than
   ignored. When a step inserts, deletes or moves sheets, rows, columns or cells, the inputs, the
   output and the range of a Repeat in progress move with their cells, as references in formulas
   do; a step that deletes one of them, or changes the size of a range a Repeat is going over,
   stops the run.

`macro.check {name, inputs?, onConflict?, overwrite?, timeLimit?}` reports the bound inputs, the
output anchor, any approval needed and the footprint, and puts back everything the dry run
changed. `ok` says whether `macro.run` with the same parameters would go ahead without a question;
when it wouldn't, `errors` says why: the run would fail at a step, a Stop step rolls it back, it
would write over data under `ask` or `fail`, shifting can't make room, or the run would go past
the step or parameter limit counting the dry runs before it. The random numbers RAND gives move
on, as after any recalculation.

## Debugging

`macro.debug {action, …}` with `start` (`name`, `inputs`, `breakpoints`), `step`, `continue`,
`runTo` (`step`: a path), `stop` (`keep: true` keeps the changes) and `state`. `breakpoints` can
be given with any action; `breakpointsOnly: true` makes the list replace the steps' own flags. The
run stays one undo step; while it is open, edits of the workbooks it touched (its own, and any
other its steps changed) are refused with "Finish or stop the macro first", also when another
command makes them (stopping Record Actions saves a script), and other workbooks stay
editable. Each answer gives the next step with its parameters as they would run, the parameters
of the step that just ran, and the cells changed so far.

## Recording

`macro.record` toggles recording. Starting takes `relative` (default true; `false` for absolute
references); stopping takes `name` and `description`. A name that another macro has, a kept one
included, gets a suffix, such as `Tidy (2)`, so stopping never replaces a macro. `cancel: true`
ends a recording without saving it, the way out of one whose macro can't be saved. A parameter
the call would ignore is refused: `name` or `description` when starting, `relative` when
stopping, `stop: false` while recording, `stop`, `relative`, `name` or `description` beside
`cancel: true`, and one of the wrong kind. The macro is saved in the workbook where recording started, even if another one is active;
if that workbook was closed, stopping ends the recording without saving it. When saving the
recording fails (a name the check refuses, a workbook at its macro limit, a macro being debugged
in that workbook), recording goes on, so stopping again keeps what was recorded. The person
recorded the macro, so in a workbook opened from a file it runs without the approval a macro that
came with the file needs, until it changes.

A recording keeps up to 100,000 commands and 64 MB of them as JSON, and a macro holds up to
10,000 steps and 4,194,304 characters of JSON. The steps past any of these limits, and steps
recorded in another workbook, are left out and counted in `dropped`.

With relative references the selection when recording starts is the example input: references in
and near it on its sheet become tokens (on another sheet `C4` stays that sheet's `C4`), and the
input's shape, header and column types are inferred from it as it was when recording started,
before the recorded steps changed it. Recorded text that playback would read as a token stays
text: a reference parameter that reads as one (a defined name called `Output`) is written
`{"$text": "Output"}`, and `{{` in a string is written `{{{{`. Toggles such as Bold are recorded
with the state they set (`"on": true`), also inside a script run, so playing them back sets them
rather than flipping them.

Commands a macro may not run are left out and listed in `skipped`, also from the steps of a macro
or script run while recording. A macro run while recording is replaced by its steps when it has
no inputs, no output and only plain commands (otherwise it is left out and listed), and an
Automate script run by the steps it ran, without a step that only opened a dialog (a menu-style
call without its parameters). Both are kept as they ran, so editing or deleting the macro or
script before the recording stops changes nothing; a run whose steps did not fit in the
recording's 64 MB is left out and listed.

Undo while recording works as in Excel: `edit.undo` is left out (a macro never runs it), and the
step it undid stays in the recording, so playing the recording back makes that edit again.

## Other commands and storage

`macro.list`, `macro.get`, `macro.save {macro, replace?}`, `macro.delete` and `macro.rename
{name, to}`. Saving under a name a macro has is refused unless `replace: true` is passed, and
renaming to one is refused; names compare ignoring the case of the letters A to Z. A renamed macro is checked
against every limit again, its size included. Saving, renaming and deleting are undoable.

Macros are saved in the workbook: as a field in GridCraft's JSON format, and in xlsx as hidden
defined names (`_gc_macro_<n>_<k>`, the macro's JSON split into string literals within Excel's
limits), which other spreadsheet applications keep but do not run. A visible name of that shape
is someone's own name: opening a file leaves it alone, and saving stores the macros under indices
it does not use.

Each macro carries its format version, `v` (1 in this version; a missing `v` reads as 1). When a
file is opened, a macro in a newer format (a `v` above 1) is kept unread, with a warning: this
version can't show or run it, and saving writes it back unchanged, so the newer
version gets it back whole. It is never read in part. Kept macros count toward the 1,000-macro
limit after the readable ones, and one too large for an xlsx file is left out. They count when a
macro is added too (saving, or stopping a recording, is refused once readable and kept macros
together reach 1,000, and the message says how many are kept), so a save holds no more macros
than the next open keeps. Their names are taken: saving or renaming a macro to one is refused, and
a recording gets a suffix. A macro that is
damaged or over the limits is left out with one warning, and the rest of the workbook and its
other macros open; a macro that was left out is not in the saved file.

## Automate scripts and VBA

Automate scripts (`automate.recordActions`, `newScript`, `allScripts`, `runScript`,
`deleteScript`) are an older, separate feature. They keep their commands, parameters, storage
(`_sc_scripts`) and results, and View › Macros (`view.macros`) still lists them. Macro commands
neither read nor change their storage; a recording keeps the steps a script run ran while it
recorded. A workbook can hold a script and a macro with the same name.

A VBA project in an `.xlsm` or `.xltm` file is preserved on save and never run. GridCraft macros
are stored apart from it.
