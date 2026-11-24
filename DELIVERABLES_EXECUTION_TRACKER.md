# Deliverables Execution Tracker (Pre-Construction + Supervision)

Legend: `[ ]` pending, `[~]` active, `[x]` done and verified by a real command, `[E]` already existed before this task (module named), `[?]` needs a user input or a source file the application is not allowed to invent.

Ground rules taken from `CLAUDE.md`: nothing invents a code limit, load, soil, price or rate. A value the user must supply is reported by name and the figure that needs it is not computed. Statutory values need a source read from `C:\Projects\البناء\الكود السعودي\`; the prompt's SBC clause numbers (301/304/601/602/801) are NOT taken from the prompt, they are inputs with a stated source.

## Step 1 — Inspection
- [x] Repository, `nnx-core` (209 modules), `src/backend.rs` command pattern, `assets/index.html` read. Pattern: module in `nnx-core` with an `Inputs` struct stored under `meta.*` (undo/redo/dirty come free), `report()` -> `html()` / `workbook()`, backend commands `X_report` + `X_set`.

## Module A — Data schema and calculation engines
- [x] A1 Geotechnical data model (q_allowable, groundwater depth, soil layers, recommended foundation, report reference) + cross-check against `structure.bearing_kpa` — `nnx-core/src/geotech.rs`, command `geotech`, tab section «Geotechnical»
- [E] A1b Topographic survey: plot boundary import `plot_import.rs` (DXF + E/N survey); WGS84 lat/long deliberately rejected there (no datum assumption)
- [E] A2 Structural calculation note: `calcreport.rs`, `sizing.rs`, `lateral.rs`, `punching.rs`, `deepfound.rs`, `rebar.rs`
- [E] A2 MEP calculation: `mep.rs`, `mepx.rs`, `panels.rs`, `cables.rs`, `hydraulics.rs`, `firesys.rs`, `pumps.rs`, `report.rs`
- [?] A2 Load combinations, wind/seismic parameters, thermal U-value limits: values must be read from SBC 301/304/601/602 files and entered with source; not hard-coded
- [E] A3 QTO / BOQ: `qto.rs`, `boq.rs`, `costtier.rs`, `tender.rs`, `bids.rs`
- [E] A3 Technical specifications: `tender.rs` already prints the specification text of each BOQ item with its source; no separate generator was added (it would duplicate it)
- [x] A3 Draft construction contract + payment/retention/penalty/warranty terms — `nnx-core/src/contract.rs`, command `contract_draft` (terms user-entered, marked DRAFT, missing terms named)

## Module B — Pre-construction
- [E] B1 IFC-style sheet set with title block, stamp, index: `permitpack.rs`, `printdoc.rs`, `dxf.rs`, `ifc.rs`
- [E] B1 Structural/MEP sheets: `rebardraw.rs`, `stairdetail.rs`, `ductpath.rs`, `routing.rs`, `lighting.rs`
- [E] B2 Tender pack, SBC compliance certificate, permit support: `tender.rs`, `certificate.rs`, `compliance.rs`, `permitpack.rs`
- [?] B2 Balady / RCJY portal formats and latent-defects-insurance (TIC) technical requirements: no source file for either in `الكود السعودي`; not invented
- [x] B3 Dossier folder export (`01_Survey_Geotech` … `08_CloseOut_AsBuilt`) — `bundle::build_dossier`, command `dossier_export`; a part that cannot be built is listed in `skipped` with the reason (e.g. `03_Calculations` until sizing inputs exist, `05_Permits` until the engineer's details are given)

## Module C — Supervision and QA/QC
- [x] C1 Site handover minutes (coordinates, benchmark, signatories) — `nnx-core/src/supervision.rs`
- [x] C1 MAR + shop-drawing logs with Approved / Approved with comments / Rejected
- [x] C2 WIR with mandatory hold points per stage (approval refused while a hold point is unsigned or absent)
- [x] C2 MIR checked against an approved MAR
- [x] C2 Lab tests: concrete cubes (age from dates, flagged when not 7/28; ratio to design f'c), Proctor compaction, pressure tests; verdict only against an acceptance value entered WITH its source, otherwise `unchecked`
- [E] C2 Stage checklists: `qaqc.rs`
- [x] C3 Progress reports with planned/actual S-curve (inline SVG), manpower, equipment, blockers, photo references
- [x] C3 MOM, RFI (sheet / element reference as text), NCR with corrective action and overdue detection. RFI markup: a room-key reference is marked on the AS-BUILT plan; other references stay text
- [E] C4 Milestone payments and progress certificates: `payment.rs`, `progress.rs`
- [x] C4 Quantity-based IPC (cumulative BOQ quantities, previous certificates, retention, VAT, deductions, approved VOs by %) — `nnx-core/src/ipc.rs`; workbook has live formulas
- [x] C4 Variation orders with cost and time impact (approved only are totalled). The approved days now move the baseline schedule finish (table `vo_schedule`, calendar days; no baseline schedule → not computed and reported)
- [x] C5 Snag list by room (room keys validated against the brief). Snags and open RFIs are marked on the room plan of each level inside the AS-BUILT set (`supervision::issues_plan_svg`; an RFI whose reference is not a room key, and a snag on a room not in the model, are listed as unplaced, never positioned)
- [E] C5 As-built deviation check: `asbuilt.rs`
- [x] C5 Close-out: TOC/completion checklist, warranty register with expiry, O&M manual list, readiness blockers
- [x] C5 "AS-BUILT" stamped plan set + approved VOs + measurement count — command `asbuilt_sheets`. Field redlines (measured vs design positions, coloured by the user's tolerance) are drawn on each level plan inside the set; none is made until a row gives an x or y

## Export and localization
- [E] XLSX writer with formulas, RTL sheets: `xlsx.rs`; PDF is produced by the WebView2 shell from HTML (Arabic shaping by the web engine) — no new PDF engine
- [x] Bilingual HTML + XLSX for every new register (tested in en and ar)
- [x] Backend commands + one «Deliverables» tab (sections: Overview, Pre-construction, Geotechnical, Handover, MAR, Shop, WIR, MIR, Lab, NCR, RFI, MOM, VO, Progress, IPC, Snags, Close-out, Contract) in `assets/index.html`. It follows the page's existing light/dark tab UI; the navy/gold/cyan-steel ribbon in the request does not exist in this codebase and was not invented

## Verification
- [x] `cd nnx-core ; cargo test ; cargo clippy --all-targets` → 957 passed, 0 failed; clippy: no warnings
- [x] `cargo test ; cargo clippy --all-targets` (root) → 233 passed, 0 failed; clippy: no warnings
- [x] End-to-end: backend test `supervision_registers_ipc_contract_and_the_full_dossier_are_built_from_one_project` + `node scripts/e2e-deliverables.js <dir>` drives the real window: 21/21 checks
- [x] FEATURES.md updated (no README.md exists at the root)
