# floorplan-bim audit and what NanoArch took from it (Milestone 8)

Read-only audit of `C:\Projects\floorplan-bim` at commit `215df4a` (Next.js / React / Three.js, in-browser data layer or Supabase;
`src/engine` is a pure TypeScript model, `src/components/workspace` the screens). Nothing in that project was changed.

## 1. What floorplan-bim has

| Area | Capabilities (source) |
|---|---|
| Tools | select, wall (line, rectangle, arc), door, window, section, grid, dimension, room separator, room, move, copy, rotate, mirror, trim to corner, align, split, tag (`workspace/types.ts`) |
| Snapping and guides | wall/grid endpoints, crossings, midpoints, perpendicular foot from the start point, along a grid line, a grid that follows the zoom, alignment with the start point, Shift = 45° with the length snapped (`workspace/snap.ts`) |
| Drafting without a pointer | keyboard cursor stepped along the grid, Enter acts as a click (`kbcursor.ts`) |
| Typed input | exact length while drawing, temporary dimensions on the selection, typed values in the inspector with the reason when refused (`TempDimensions.tsx`, `typed-dims.ts`) |
| Selection | click, Shift add, window box (left to right) and crossing box (right to left) (`pick.ts`) |
| Placement | door and window find the wall under the pointer, offset snapped to 50 mm, centred near the middle, kept inside the wall (`placement.ts`, `openings.ts`) |
| Families and types | wall types with compound layers and a location line, door and window types, a loadable catalogue (`families.ts`, `typeedit.ts`, `wall-types.ts`) |
| Inspector | wall: type, location line, mark, room bounding, dimensions; door/window: mark, type, distance along and from the end, sill, type size, fit status; room: name, number, department, area, perimeter, status (`Properties.tsx`) |
| Canvas | wheel zoom, pan, `[` `]` zoom, Home = fit, view range, underlay and tracing (`PlanCanvas.tsx`, `view-range.ts`, `trace.ts`) |
| 3D | orbit, pan, zoom, picking, section box, phases (`View3D.tsx`, `camera3d.ts`, `section-box.ts`) |
| Documentation | elevations, sections, sheets, schedules, legends, areas, DXF/IFC/PDF, code check, thermal, phases, units (`engine/*`) |
| Product | accounts, projects, Supabase and Stripe, browser storage (`data/*`, `server/*`) |

## 2. Mapped to NanoArch v1.0.0

| floorplan-bim | NanoArch before | Decision |
|---|---|---|
| Free wall / door / window drafting | none: the plan was generated, and edited only by moving partitions, resizing rooms and changing an opening's size | **ported** (orthogonal walls, doors, windows) |
| Snapping (end, crossing, middle, perpendicular, grid, alignment, ortho) | none | **ported** in `nnx-core::draft::snap` |
| Rubber band, typed length | none | **ported** (page draws, backend snaps) |
| Window / crossing box select, Shift multi-select | single click | **ported** |
| Move, copy, rotate, mirror | none | **ported** (quarter turns; doors and windows go with their wall) |
| Trim to corner, align, split | none | **ported** |
| Placement of openings, 50 mm step, edge clearance | opening width / position edit only | **ported** |
| Inspector with typed edits | read-only facts + width and offset | **ported** (wall length, thickness, height; opening width, height, sill, distance from either end; column section) |
| Wheel zoom, pan, fit | none in 2D | **ported** |
| Undo / redo, persistence | `Document` stack and `.nnx` | **kept**: every drafted edit is one `Document` step, stored in the project file, not in browser storage |
| Elevations, sections, schedules, quantities, DXF/IFC/PDF | present | already covered, not touched |
| Code check, thermal, area report | SBC checks, U-values, certificate, overlays | already covered, not touched |
| 3D orbit, pick | present, plus walk, X-ray, tour | covered; selection is now shared with the plan and outliner |
| Arc and non-orthogonal walls | the typed `Wall` is horizontal or vertical | **not ported**: it would break the rules, quantities, 3D and structure that read walls; a wall at another angle is refused in words |
| Wall types with compound layers, location line | thermal layers live in the compliance inputs | **not ported**: a second source of truth for the same layers |
| Grid lines, persistent dimensions, tags | structural grid optimizer only | **not ported**: new element kinds with their own drawing, quantities and exports; next candidates |
| Phases, sheets, underlay tracing, section box | PDF sheets exist | **not ported**: large, and not needed for drafting |
| Keyboard cursor | none | **not ported**: shortcuts, typed length and Enter cover the common cases |
| Accounts, Supabase, Stripe, `localStorage` | local desktop app | **not applicable** |

## 3. How it was integrated

- `nnx-core/src/draft.rs`: pure functions that read the model and return `Command`s. The backend commits each result with the structure sync as ONE
  undoable step; the strict typed check refuses anything invalid. Drafted elements are `Source::User`.
- Nothing is invented: a wall's thickness and height and an opening's size and sill are entered by the user (the page offers the values most common on the
  level as a starting point and says so); a drafted wall's rooms and exterior flag, and the rooms a door connects, are derived from the rooms' rectangles.
- Generated walls, doors and windows are still rebuilt by partition and room edits, but what the user drafted is left alone. An opening placed in a wall
  that such an edit would rebuild makes the edit refuse, naming it.
- The plan's frame includes walls and columns drawn beyond the rooms, so nothing drafted outside the building is cut off.
- The selection is the shared set of the outliner, the 2D plan, the 3D view and the property panel.
- `scripts/e2e-draft.js` drives the real window with real mouse and keyboard events.
