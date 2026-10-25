# Macros

> **Last updated:** 2026-10-24 · **Change:** minor (macro tokens)

GridCraft macros are typed, declarative lists of commands, not VBA. Every step runs a GridCraft
command (the same ids the ribbon, CLI and MCP use) or one of three bounded control steps. A macro
declares the inputs it works on.

This page describes the macro format, the tokens its steps use and how macros are saved in a
workbook. The engine has no command that runs macros in this version.

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
  type, and no input lists more column types than it allows columns. This version stores and
  checks the declaration; it does not bind inputs or run macros.
- **Limits**: 1,000 macros a workbook, 10,000 steps a macro, Repeat and If nested 4 deep, and 64
  inputs a macro. Names (of macros and inputs) and command ids are at most 255 characters;
  descriptions, step notes, Stop messages and If values (as JSON) 4,096; references and tokens
  (`output.anchor`, `defaultRef`, a Repeat's `over`, an If's `cell`) 1,024; a shortcut 64. A
  macro lists at most 256 permissions, each a command id, and an input at most 16,384 column
  types. One-line fields hold no line breaks or other control characters, and a command id is
  letters, digits, `.` and `_`. A command step's `params` are bounded only by the size of the
  whole macro (4,194,304 characters of JSON). How many times a Repeat runs (`max`) is the
  runner's to bound. A macro read from a file that breaks any of these is left out with a
  warning; it is never changed to fit.

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
any string, such as a formula; `{{{{` in a string is a literal `{{`. Under the general keys that
other commands use for plain words (`to`, `from`, `at`, `area`, `source`, `target`,
`destination`, `location`), a bare `Row` or `Column` stays text (a replacement word, a pivot
area); write `Row.col(2)`, `{{Row}}` or `{"$ref": "Row"}` there. Plain A1 references stay
absolute; a `Name:` anchor follows inserted rows. A reference with a sheet name is never a token,
so `Output!A1` is a cell on a sheet called Output, even when a token or input has that name.
`Input.data` on a range that is only a header row, and `Name:` of a table with no data rows, are
refused rather than read as the header.

## Storage

Macros are saved in the workbook: as a field in GridCraft's JSON format, and in xlsx as hidden
defined names (`_gc_macro_<n>_<k>`, the macro's JSON split into string literals within Excel's
limits), which other spreadsheet applications keep but do not run. A visible name of that shape
is someone's own name: opening a file leaves it alone, and saving stores the macros under indices
it does not use.

Each macro carries its format version, `v` (1 in this version; a missing `v` reads as 1). When a
file is opened, a macro in a newer format (a `v` above 1) is kept unread, with a warning: this
version can't show or run it, and saving writes it back unchanged, so the newer
version gets it back whole. It is never read in part. Kept macros count toward the 1,000-macro
limit after the readable ones, and one too large for an xlsx file is left out. A macro that is
damaged or over the limits is left out with one warning, and the rest of the workbook and its
other macros open; a macro that was left out is not in the saved file.

## Automate scripts and VBA

Automate scripts (`automate.recordActions`, `newScript`, `allScripts`, `runScript`,
`deleteScript`) are an older, separate feature. They keep their commands, parameters, storage
(`_sc_scripts`) and results, and View › Macros (`view.macros`) still lists them. Macros neither
read nor change them; a workbook can hold a script and a macro with the same name.

A VBA project in an `.xlsm` or `.xltm` file is preserved on save and never run. GridCraft macros
are stored apart from it.
