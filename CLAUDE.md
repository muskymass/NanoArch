# NanoArch: project rules

This file overrides the Flutter/Dart parts of `C:\Projects\CLAUDE.md`. NanoArch is a **Rust** Windows desktop
application. Riverpod, Drift, go_router and the Dart verification loop do not apply here. The workspace rules that
still apply: zero placeholders, root-cause fixes, evidence in every hand-off, secrets never in source, Conventional
Commits, no `git add -A`.

## Architecture
- `nnx-core/` (crate `nnx-core`): the engine and the only place with project logic. Model, commands, undo/redo
  (`Document`), layout, walls, structure, MEP, insulation, QTO, BOQ, schedule, rules, DXF/IFC/XLSX export, persistence.
- `src/` (crate `nanoarch-app`): a thin shell. `tao` window + `wry` WebView2 hosting `assets/index.html`.
  `backend.rs` turns JSON commands from the page into engine calls. No engineering logic lives here.
- The page never computes engineering values. It sends inputs and draws what the backend returns.

## Single Source of Truth
- `NanoArchProject` (in `nnx-core`) is the one owner of everything a project contains: metadata, plot, brief,
  structural/MEP/BOQ inputs and the model. The app keeps no parallel copy of any of it.
- Every change, whether an input, a setback, a room edit or a wall edit, goes through `Document`'s command stack, so
  `undo()`, `redo()`, `is_dirty()` and the `.nnx` file always agree.

## No silent fallbacks
- A missing value is reported by name and the calculation that needs it does not run. Nothing invents a code, load,
  soil, price or productivity value.
- No `unwrap_or(<engineering value>)`, no swallowed errors, no `// ignore`. Failures come back as messages the user can
  act on, in Arabic and English.

## Saudi Building Code
- Every statutory value in code or data carries its source: code, edition, clause/table, page of the PDF read.
- References live in `C:\Projects\البناء\الكود السعودي\` (SBC 201, SBC 303, the SBC 1101-1102 contractor guide,
  the 1443 implementing regulations, the inspectors' checklist, and the Ministry's residential-building guidelines).
- A value that was not read from one of those files, or from a source opened and dated in the session, is `null`
  (unset) and its rule is reported as skipped, never as passed. The rule engine (`nnx-core/src/rules.rs`) already
  works this way: keep it.
- Municipal setbacks and coverage ratios are set by each municipality's own regulations: they are user inputs with a
  stated source, not constants.
- The output is a preliminary check. It does not replace a licensed engineer or the approving authority.

## Verification (every change, in order)
```
cd C:\Projects\NanoArch\nnx-core ; cargo test ; cargo clippy --all-targets
cd C:\Projects\NanoArch        ; cargo test ; cargo clippy --all-targets
```
Zero warnings introduced, zero failures. Report the real result lines. Stated confidence never exceeds 70% for code
that was not run.

## Git
- Never `git add -A` or `git add .`. Stage by directory or file name.
- `target/`, `dist/*.exe`, `*.nnx` are ignored. Commit only when asked, except a checkpoint before a structural change.
