//! IFC4 export (ISO 10303-21, schema `IFC4`) of the model.
//!
//! Everything in the file comes from the model; nothing is invented:
//! - Project, one site, one building, one `IfcBuildingStorey` per level at its derived elevation (a model without
//!   levels has one storey, "Ground floor", at 0).
//! - A wall is ONE `IfcWall` (a box along its centre line) with one `IfcOpeningElement` per door and window
//!   (`IfcRelVoidsElement`), filled by an `IfcDoor` or `IfcWindow` (`IfcRelFillsElement`).
//! - Slabs, columns, beams, footings, rooms (`IfcSpace`) and MEP points map to `IfcSlab`, `IfcColumn`, `IfcBeam`,
//!   `IfcFooting`, `IfcSpace`, `IfcLightFixture`, `IfcOutlet`, `IfcElectricDistributionBoard`, `IfcSanitaryTerminal`
//!   and `IfcUnitaryEquipment`.
//! - Geometry is extruded rectangles (axis-aligned boxes), in millimetres, each placed relative to its storey.
//! - Every product carries a property set `NanoArch` with its model id, source and lock state, so it can be traced
//!   back; GUIDs are derived from the element ids, so exporting the same model twice gives the same GUIDs.
//! - Footing thickness is a fixed VISUAL value (the application designs no footing depth) and is flagged as such.
//!
//! Strings are written in ASCII: other characters (Arabic names) use the STEP `\X2\...\X0\` escape.

use crate::model::{Element, Id, Model, Source};
use crate::model3d;
use crate::typed::{Column, Door, Feature, Footing, MepPoint, Slab, Space, Typed, Wall, Window};
use std::collections::BTreeMap;

pub struct IfcOpts {
    pub project_name: String,
    pub author: String,
    /// ISO 8601 date-time written to the file header (supplied by the caller so the export stays deterministic).
    pub timestamp: String,
    /// Display names of rooms by requirement key (`LongName` of an `IfcSpace`).
    pub labels: BTreeMap<String, String>,
}

#[derive(Debug, Clone)]
pub struct IfcOut {
    pub text: String,
    /// Number of products written, by IFC class.
    pub counts: BTreeMap<String, usize>,
}

// ------------------------------------------------------------------ STEP helpers

/// STEP string literal: ASCII only, apostrophes doubled, backslashes doubled, other characters as `\X2\hhhh\X0\`.
pub fn step_str(s: &str) -> String {
    let mut out = String::from("'");
    let mut wide: Vec<u16> = Vec::new();
    let flush = |out: &mut String, wide: &mut Vec<u16>| {
        if !wide.is_empty() {
            out.push_str("\\X2\\");
            for u in wide.iter() {
                out.push_str(&format!("{u:04X}"));
            }
            out.push_str("\\X0\\");
            wide.clear();
        }
    };
    for ch in s.chars() {
        if (' '..='~').contains(&ch) {
            flush(&mut out, &mut wide);
            match ch {
                '\'' => out.push_str("''"),
                '\\' => out.push_str("\\\\"),
                c => out.push(c),
            }
        } else {
            let mut buf = [0u16; 2];
            wide.extend_from_slice(ch.encode_utf16(&mut buf));
        }
    }
    flush(&mut out, &mut wide);
    out.push('\'');
    out
}

/// STEP real: always with a decimal point.
fn real(v: f64) -> String {
    let v = if v.abs() < 5e-4 { 0.0 } else { v };
    format!("{v:.3}").trim_end_matches('0').to_string()
}

/// The 22-character IFC GUID of 128 bits (the standard base-64 compression: 2 bits, then 21 groups of 6).
pub fn ifc_guid(bytes: [u8; 16]) -> String {
    const C: &[u8; 64] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz_$";
    let n = u128::from_be_bytes(bytes);
    let mut s = String::with_capacity(22);
    s.push(C[((n >> 126) & 3) as usize] as char);
    for i in 0..21 {
        s.push(C[((n >> (120 - 6 * i)) & 63) as usize] as char);
    }
    s
}

fn guid_of_id(id: &Id) -> String {
    match uuid::Uuid::parse_str(&id.to_string()) {
        Ok(u) => ifc_guid(*u.as_bytes()),
        Err(_) => guid_of_seed(&id.to_string()),
    }
}

/// A deterministic GUID for structural entities (project, relations ...) from a seed string.
fn guid_of_seed(seed: &str) -> String {
    let fnv = |basis: u64| -> u64 {
        let mut h = basis;
        for b in seed.bytes() {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        h
    };
    let (a, b) = (fnv(0xcbf2_9ce4_8422_2325), fnv(0x8422_2325_cbf2_9ce4 ^ 0x9e37_79b9_7f4a_7c15));
    let mut bytes = [0u8; 16];
    bytes[..8].copy_from_slice(&a.to_be_bytes());
    bytes[8..].copy_from_slice(&b.to_be_bytes());
    ifc_guid(bytes)
}

enum Pv {
    Label(String),
    Ident(String),
    Real(f64),
    Bool(bool),
}

struct W {
    entries: Vec<String>,
    origin_axis: usize,
    z_dir: usize,
    sub_ctx: usize,
}

impl W {
    fn add(&mut self, body: String) -> usize {
        self.entries.push(body);
        self.entries.len()
    }
    fn pt(&mut self, x: f64, y: f64, z: f64) -> usize {
        self.add(format!("IFCCARTESIANPOINT(({},{},{}))", real(x), real(y), real(z)))
    }
    fn axis(&mut self, x: f64, y: f64, z: f64) -> usize {
        let p = self.pt(x, y, z);
        self.add(format!("IFCAXIS2PLACEMENT3D(#{p},$,$)"))
    }
    fn place(&mut self, rel: Option<usize>, x: f64, y: f64, z: f64) -> usize {
        let a = self.axis(x, y, z);
        self.add(format!("IFCLOCALPLACEMENT({},#{a})", rel.map_or("$".to_string(), |r| format!("#{r}"))))
    }
    /// Body representation of an axis-aligned box `dx x dy x dz` whose lower corner is the placement origin.
    fn body(&mut self, dx: f64, dy: f64, dz: f64) -> usize {
        let c = self.add(format!("IFCCARTESIANPOINT(({},{}))", real(dx / 2.0), real(dy / 2.0)));
        let a2 = self.add(format!("IFCAXIS2PLACEMENT2D(#{c},$)"));
        let prof = self.add(format!("IFCRECTANGLEPROFILEDEF(.AREA.,$,#{a2},{},{})", real(dx), real(dy)));
        let solid = self.add(format!("IFCEXTRUDEDAREASOLID(#{prof},#{},#{},{})", self.origin_axis, self.z_dir, real(dz)));
        let rep = self.add(format!("IFCSHAPEREPRESENTATION(#{},'Body','SweptSolid',(#{solid}))", self.sub_ctx));
        self.add(format!("IFCPRODUCTDEFINITIONSHAPE($,$,(#{rep}))"))
    }
    fn pset(&mut self, name: &str, props: &[(&str, Pv)], target: usize) {
        let mut ids = Vec::new();
        for (k, v) in props {
            let val = match v {
                Pv::Label(s) => format!("IFCLABEL({})", step_str(s)),
                Pv::Ident(s) => format!("IFCIDENTIFIER({})", step_str(s)),
                Pv::Real(r) => format!("IFCREAL({})", real(*r)),
                Pv::Bool(b) => format!("IFCBOOLEAN({})", if *b { ".T." } else { ".F." }),
            };
            ids.push(self.add(format!("IFCPROPERTYSINGLEVALUE({},$,{val},$)", step_str(k))));
        }
        let list = ids.iter().map(|i| format!("#{i}")).collect::<Vec<_>>().join(",");
        let g = guid_of_seed(&format!("pset:{name}:{target}"));
        let ps = self.add(format!("IFCPROPERTYSET('{g}',$,{},$,({list}))", step_str(name)));
        let g2 = guid_of_seed(&format!("relprops:{name}:{target}"));
        self.add(format!("IFCRELDEFINESBYPROPERTIES('{g2}',$,$,$,(#{target}),#{ps})"));
    }
    fn quantity(&mut self, name: &str, items: &[(&str, &str, f64)], target: usize) {
        let mut ids = Vec::new();
        for (k, kind, v) in items {
            ids.push(self.add(format!("IFCQUANTITY{kind}({},$,$,{},$)", step_str(k), real(*v))));
        }
        let list = ids.iter().map(|i| format!("#{i}")).collect::<Vec<_>>().join(",");
        let g = guid_of_seed(&format!("qto:{name}:{target}"));
        let q = self.add(format!("IFCELEMENTQUANTITY('{g}',$,{},$,'NanoArch',({list}))", step_str(name)));
        let g2 = guid_of_seed(&format!("relqto:{name}:{target}"));
        self.add(format!("IFCRELDEFINESBYPROPERTIES('{g2}',$,$,$,(#{target}),#{q})"));
    }
}

fn source_name(e: &Element) -> &'static str {
    match e.provenance.source {
        Source::Generated => "Generated",
        _ => "User",
    }
}

/// The traceability property set every product carries, plus its own properties.
fn trace(w: &mut W, e: &Element, id: usize, extra: Vec<(&str, Pv)>) {
    let mut props = vec![("ModelId", Pv::Ident(e.id.to_string())), ("Source", Pv::Label(source_name(e).into())), ("Locked", Pv::Bool(e.provenance.locked))];
    // The construction phase only when the element has one: an unset phase is left out, not written as a value.
    if let Some(p) = e.props.get("phase").and_then(|v| v.as_str()) {
        props.push(("Phase", Pv::Label(p.into())));
    }
    props.extend(extra);
    w.pset("NanoArch", &props, id);
}

struct Storey {
    id: usize,
    placement: usize,
    z0: f64,
    height: f64,
    products: Vec<usize>,
    spaces: Vec<usize>,
}

fn aggregate(w: &mut W, seed: &str, parent: usize, kids: &[usize]) {
    let list = kids.iter().map(|k| format!("#{k}")).collect::<Vec<_>>().join(",");
    w.add(format!("IFCRELAGGREGATES('{}',$,$,$,#{parent},({list}))", guid_of_seed(seed)));
}

// ------------------------------------------------------------------ the export

pub fn export_ifc(model: &Model, opts: &IfcOpts) -> Result<IfcOut, String> {
    if !model.elements.values().any(|e| e.kind == Space::KIND || e.kind == Wall::KIND) {
        return Err("nothing to export: the model has no rooms or walls".into());
    }
    let scene = model3d::build(model);
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();

    let mut w = W { entries: Vec::new(), origin_axis: 0, z_dir: 0, sub_ctx: 0 };
    // Units and contexts.
    let o = w.pt(0.0, 0.0, 0.0);
    w.origin_axis = w.add(format!("IFCAXIS2PLACEMENT3D(#{o},$,$)"));
    w.z_dir = w.add("IFCDIRECTION((0.,0.,1.))".into());
    let ctx = w.add(format!("IFCGEOMETRICREPRESENTATIONCONTEXT($,'Model',3,1.E-05,#{},$)", w.origin_axis));
    w.sub_ctx = w.add(format!("IFCGEOMETRICREPRESENTATIONSUBCONTEXT('Body','Model',*,*,*,*,#{ctx},$,.MODEL_VIEW.,$)"));
    let u_len = w.add("IFCSIUNIT(*,.LENGTHUNIT.,.MILLI.,.METRE.)".into());
    let u_area = w.add("IFCSIUNIT(*,.AREAUNIT.,$,.SQUARE_METRE.)".into());
    let u_vol = w.add("IFCSIUNIT(*,.VOLUMEUNIT.,$,.CUBIC_METRE.)".into());
    let u_ang = w.add("IFCSIUNIT(*,.PLANEANGLEUNIT.,$,.RADIAN.)".into());
    let units = w.add(format!("IFCUNITASSIGNMENT((#{u_len},#{u_area},#{u_vol},#{u_ang}))"));
    let project = w.add(format!("IFCPROJECT('{}',$,{},$,$,$,$,(#{ctx}),#{units})", guid_of_seed("project"), step_str(&opts.project_name)));

    // Spatial structure.
    let site_pl = w.place(None, 0.0, 0.0, 0.0);
    let site = w.add(format!("IFCSITE('{}',$,'Site',$,$,#{site_pl},$,$,.ELEMENT.,$,$,$,$,$)", guid_of_seed("site")));
    let bld_pl = w.place(Some(site_pl), 0.0, 0.0, 0.0);
    let building = w.add(format!("IFCBUILDING('{}',$,{},$,$,#{bld_pl},$,$,.ELEMENT.,$,$,$)", guid_of_seed("building"), step_str(&opts.project_name)));
    aggregate(&mut w, "agg:project", project, &[site]);
    aggregate(&mut w, "agg:site", site, &[building]);

    // One storey per level (or a single one for a model without levels).
    let mut storeys: BTreeMap<i64, Storey> = BTreeMap::new();
    let level_list: Vec<(i64, String, f64, f64)> = if scene.levels.is_empty() { vec![(0, "Ground floor".into(), 0.0, scene.storey_height)] } else { scene.levels.iter().map(|l| (l.index, l.name.clone(), l.z0, l.storey)).collect() };
    for (idx, name, z0, height) in &level_list {
        let pl = w.place(Some(bld_pl), 0.0, 0.0, z0 * 1000.0);
        let id = w.add(format!("IFCBUILDINGSTOREY('{}',$,{},$,$,#{pl},$,$,.ELEMENT.,{})", guid_of_seed(&format!("storey:{idx}")), step_str(name), real(z0 * 1000.0)));
        storeys.insert(*idx, Storey { id, placement: pl, z0: *z0, height: *height, products: Vec::new(), spaces: Vec::new() });
    }
    let storey_ids: Vec<usize> = storeys.values().map(|s| s.id).collect();
    aggregate(&mut w, "agg:building", building, &storey_ids);
    let first = *storeys.keys().next().expect("at least one storey");
    let top = *storeys.keys().next_back().expect("at least one storey");
    let level_of = |e: &Element| -> i64 { e.level_id.and_then(|l| scene.levels.iter().find(|i| i.id == l.to_string()).map(|i| i.index)).unwrap_or(first) };

    // Walls with their openings; doors and windows fill them.
    let mut opening_of: BTreeMap<String, usize> = BTreeMap::new();
    for e in model.elements.values().filter(|e| e.kind == Wall::KIND) {
        let Ok(wall) = e.typed::<Wall>() else { continue };
        let lvl = level_of(e);
        let st_pl = storeys[&lvl].placement;
        let t = wall.thickness as f64;
        let (x0, y0, dx, dy) = if wall.horizontal() { (wall.start() as f64, wall.centre_line() as f64 - t / 2.0, wall.length as f64, t) } else { (wall.centre_line() as f64 - t / 2.0, wall.start() as f64, t, wall.length as f64) };
        let pl = w.place(Some(st_pl), x0, y0, 0.0);
        let shape = w.body(dx, dy, wall.height as f64);
        let name = format!("Wall {}", if wall.is_exterior { "exterior" } else { "interior" });
        let wid = w.add(format!("IFCWALL('{}',$,{},$,$,#{pl},#{shape},$,.NOTDEFINED.)", guid_of_id(&e.id), step_str(&name)));
        *counts.entry("IfcWall".into()).or_default() += 1;
        w.pset("Pset_WallCommon", &[("IsExternal", Pv::Bool(wall.is_exterior))], wid);
        trace(&mut w, e, wid, vec![("RoomA", Pv::Label(wall.room_a.clone())), ("RoomB", Pv::Label(wall.room_b.clone().unwrap_or_default()))]);
        w.quantity("Qto_WallBaseQuantities", &[("Length", "LENGTH", wall.length as f64), ("Width", "LENGTH", t), ("Height", "LENGTH", wall.height as f64)], wid);
        storeys.get_mut(&lvl).expect("storey").products.push(wid);

        for op in model.elements.values().filter(|o| o.host_id == Some(e.id)) {
            let (center, width, sill, head, label) = if let Ok(d) = op.typed::<Door>() {
                (d.offset_center, d.width, 0, d.height, "Door")
            } else if let Ok(win) = op.typed::<Window>() {
                (win.offset_center, win.width, win.sill_height, win.sill_height + win.height, "Window")
            } else {
                continue;
            };
            let c = wall.start() + center;
            let (ox, oy, odx, ody) = if wall.horizontal() { ((c - width / 2) as f64, y0, width as f64, dy) } else { (x0, (c - width / 2) as f64, dx, width as f64) };
            let opl = w.place(Some(st_pl), ox, oy, sill as f64);
            let oshape = w.body(odx, ody, (head - sill) as f64);
            let oid = w.add(format!("IFCOPENINGELEMENT('{}',$,{},$,$,#{opl},#{oshape},$,.OPENING.)", guid_of_seed(&format!("opening:{}", op.id)), step_str(&format!("{label} opening"))));
            w.add(format!("IFCRELVOIDSELEMENT('{}',$,$,$,#{wid},#{oid})", guid_of_seed(&format!("void:{}", op.id))));
            opening_of.insert(op.id.to_string(), oid);
        }
    }

    // Everything else from the 3D geometry (one box per element), placed relative to its storey.
    for b in scene.boxes.iter().filter(|b| matches!(b.kind, "Slab" | "Column" | "Beam" | "Footing" | "Door" | "Window" | "MepPoint")) {
        let Some(e) = model.elements.values().find(|e| e.id.to_string() == b.id) else { continue };
        let lvl = if storeys.contains_key(&b.level.unwrap_or(first)) { b.level.unwrap_or(first) } else { first };
        let (st_pl, z0) = (storeys[&lvl].placement, storeys[&lvl].z0);
        let (dx, dy, dz) = ((b.max[0] - b.min[0]) * 1000.0, (b.max[1] - b.min[1]) * 1000.0, (b.max[2] - b.min[2]) * 1000.0);
        let pl = w.place(Some(st_pl), b.min[0] * 1000.0, b.min[1] * 1000.0, (b.min[2] - z0) * 1000.0);
        let shape = w.body(dx, dy, dz);
        let g = guid_of_id(&e.id);
        let (class, id) = match b.kind {
            "Slab" => {
                let kind = if lvl == top { ".ROOF." } else { ".FLOOR." };
                let id = w.add(format!("IFCSLAB('{g}',$,'Slab',$,$,#{pl},#{shape},$,{kind})"));
                let extra = e.typed::<Slab>().map(|s| vec![("ThicknessMm", Pv::Real(s.thickness as f64)), ("Room", Pv::Label(s.room))]).unwrap_or_default();
                trace(&mut w, e, id, extra);
                ("IfcSlab", id)
            }
            "Column" => {
                let id = w.add(format!("IFCCOLUMN('{g}',$,'Column',$,$,#{pl},#{shape},$,.COLUMN.)"));
                let mut extra = Vec::new();
                if let Ok(c) = e.typed::<Column>() {
                    extra.push(("GridRef", Pv::Label(c.grid_ref)));
                    if let Some(kn) = c.axial_service_kn {
                        extra.push(("AxialServiceKn", Pv::Real(kn)));
                    }
                }
                trace(&mut w, e, id, extra);
                ("IfcColumn", id)
            }
            "Beam" => {
                let id = w.add(format!("IFCBEAM('{g}',$,'Beam',$,$,#{pl},#{shape},$,.BEAM.)"));
                trace(&mut w, e, id, Vec::new());
                ("IfcBeam", id)
            }
            "Footing" => {
                let id = w.add(format!("IFCFOOTING('{g}',$,'Footing',$,$,#{pl},#{shape},$,.PAD_FOOTING.)"));
                let mut extra = vec![("ThicknessIsVisualOnly", Pv::Bool(true))];
                if let Ok(f) = e.typed::<Footing>() {
                    extra.push(("ServiceLoadKn", Pv::Real(f.service_load_kn)));
                    extra.push(("BearingKpa", Pv::Real(f.bearing_kpa)));
                }
                trace(&mut w, e, id, extra);
                ("IfcFooting", id)
            }
            "Door" | "Window" => {
                let door = b.kind == "Door";
                let (h, wd) = if door { e.typed::<Door>().map(|d| (d.height, d.width)).unwrap_or((0, 0)) } else { e.typed::<Window>().map(|d| (d.height, d.width)).unwrap_or((0, 0)) };
                let (step, name, kind) = if door { ("IFCDOOR", "Door", ".DOOR.") } else { ("IFCWINDOW", "Window", ".WINDOW.") };
                let id = w.add(format!("{step}('{g}',$,'{name}',$,$,#{pl},#{shape},$,{},{},{kind},.NOTDEFINED.,$)", real(h as f64), real(wd as f64)));
                trace(&mut w, e, id, Vec::new());
                if let Some(op) = opening_of.get(&b.id) {
                    w.add(format!("IFCRELFILLSELEMENT('{}',$,$,$,#{op},#{id})", guid_of_seed(&format!("fill:{}", e.id))));
                }
                (if door { "IfcDoor" } else { "IfcWindow" }, id)
            }
            _ => {
                let Ok(p) = e.typed::<MepPoint>() else { continue };
                let (cls, step, name) = match p.role.as_str() {
                    "light" => ("IfcLightFixture", "IFCLIGHTFIXTURE", "Light"),
                    "socket" => ("IfcOutlet", "IFCOUTLET", "Socket"),
                    "panel" => ("IfcElectricDistributionBoard", "IFCELECTRICDISTRIBUTIONBOARD", "Distribution board"),
                    "fixture" => ("IfcSanitaryTerminal", "IFCSANITARYTERMINAL", "Fixture"),
                    _ => ("IfcUnitaryEquipment", "IFCUNITARYEQUIPMENT", "Air-conditioning unit"),
                };
                let nm = p.label.clone().map_or(name.to_string(), |l| format!("{name} {l}"));
                let id = w.add(format!("{step}('{g}',$,{},$,$,#{pl},#{shape},$,.NOTDEFINED.)", step_str(&nm)));
                let mut extra = vec![("Role", Pv::Label(p.role.clone())), ("Room", Pv::Label(p.room.clone()))];
                if let Some(wt) = p.watts {
                    extra.push(("Watts", Pv::Real(wt)));
                }
                if let Some(c) = p.circuit.clone() {
                    extra.push(("Circuit", Pv::Label(c)));
                }
                trace(&mut w, e, id, extra);
                (cls, id)
            }
        };
        *counts.entry(class.into()).or_default() += 1;
        storeys.get_mut(&lvl).expect("storey").products.push(id);
    }

    // Named design features (joint, transfer beam, light well, shafts, lift, landings ...) as building element proxies on their
    // storey. The drawing gives their plan geometry only, so each is a footprint of a nominal 100 mm height, flagged visual only.
    // A line is the band of its width when it is axis-aligned, else its bounding box; a point is a 200 mm square (also flagged).
    for e in model.elements.values().filter(|e| e.kind == Feature::KIND) {
        let Ok(f) = e.typed::<Feature>() else { continue };
        let lvl = level_of(e);
        let st_pl = storeys[&lvl].placement;
        let (x0, y0, dx, dy, approx) = match f.shape.as_str() {
            "rect" => (f.x1.min(f.x2), f.y1.min(f.y2), (f.x2 - f.x1).abs(), (f.y2 - f.y1).abs(), false),
            "line" if f.y1 == f.y2 && f.width > 0 => (f.x1.min(f.x2), f.y1 - f.width / 2, (f.x2 - f.x1).abs(), f.width, false),
            "line" if f.x1 == f.x2 && f.width > 0 => (f.x1 - f.width / 2, f.y1.min(f.y2), f.width, (f.y2 - f.y1).abs(), false),
            "line" => (f.x1.min(f.x2), f.y1.min(f.y2), (f.x2 - f.x1).abs().max(1), (f.y2 - f.y1).abs().max(1), true),
            _ => (f.x1 - 100, f.y1 - 100, 200, 200, true),
        };
        let pl = w.place(Some(st_pl), x0 as f64, y0 as f64, 0.0);
        let shape = w.body(dx.max(1) as f64, dy.max(1) as f64, 100.0);
        let label = f.name.clone().unwrap_or_else(|| f.role.clone());
        let id = w.add(format!("IFCBUILDINGELEMENTPROXY('{}',$,{},$,{},#{pl},#{shape},$,.NOTDEFINED.)", guid_of_id(&e.id), step_str(&label), step_str(&f.role)));
        *counts.entry("IfcBuildingElementProxy".into()).or_default() += 1;
        let mut extra = vec![("Role", Pv::Label(f.role.clone())), ("PlanShape", Pv::Label(f.shape.clone())), ("HeightIsVisualOnly", Pv::Bool(true)), ("PlanGeometryApproximated", Pv::Bool(approx))];
        if f.width > 0 {
            extra.push(("WidthMm", Pv::Real(f.width as f64)));
        }
        if let Some(n) = f.note.clone() {
            extra.push(("Note", Pv::Label(n)));
        }
        if let Some(r) = f.room.clone() {
            extra.push(("Room", Pv::Label(r)));
        }
        trace(&mut w, e, id, extra);
        storeys.get_mut(&lvl).expect("storey").products.push(id);
    }

    // Rooms as IfcSpace over the storey height.
    for e in model.elements.values().filter(|e| e.kind == Space::KIND) {
        let Ok(sp) = e.typed::<Space>() else { continue };
        let lvl = level_of(e);
        let (st_pl, height) = (storeys[&lvl].placement, storeys[&lvl].height);
        let pl = w.place(Some(st_pl), sp.x as f64, sp.y as f64, 0.0);
        let shape = w.body(sp.w as f64, sp.h as f64, height * 1000.0);
        let long = crate::annot::room_name(&sp, &opts.labels).unwrap_or_default();
        let id = w.add(format!("IFCSPACE('{}',$,{},$,$,#{pl},#{shape},{},.ELEMENT.,.INTERNAL.,$)", guid_of_id(&e.id), step_str(&sp.requirement_key), if long.is_empty() { "$".to_string() } else { step_str(&long) }));
        *counts.entry("IfcSpace".into()).or_default() += 1;
        trace(&mut w, e, id, vec![("Function", Pv::Label(sp.func.clone()))]);
        w.quantity("Qto_SpaceBaseQuantities", &[("NetFloorArea", "AREA", sp.area)], id);
        storeys.get_mut(&lvl).expect("storey").spaces.push(id);
    }

    // Containment: products in their storey, rooms aggregated under it.
    let all: Vec<(i64, usize, Vec<usize>, Vec<usize>)> = storeys.iter().map(|(i, s)| (*i, s.id, s.products.clone(), s.spaces.clone())).collect();
    for (idx, sid, products, spaces) in all {
        if !products.is_empty() {
            let list = products.iter().map(|p| format!("#{p}")).collect::<Vec<_>>().join(",");
            w.add(format!("IFCRELCONTAINEDINSPATIALSTRUCTURE('{}',$,$,$,({list}),#{sid})", guid_of_seed(&format!("contain:{idx}"))));
        }
        if !spaces.is_empty() {
            aggregate(&mut w, &format!("agg:spaces:{idx}"), sid, &spaces);
        }
    }

    let mut text = String::new();
    text.push_str("ISO-10303-21;\nHEADER;\n");
    text.push_str("FILE_DESCRIPTION(('ViewDefinition [CoordinationView]'),'2;1');\n");
    text.push_str(&format!("FILE_NAME({},{},({}),('NanoNexus'),'NanoArch','NanoArch','');\n", step_str("model.ifc"), step_str(&opts.timestamp), step_str(&opts.author)));
    text.push_str("FILE_SCHEMA(('IFC4'));\nENDSEC;\nDATA;\n");
    for (i, body) in w.entries.iter().enumerate() {
        text.push_str(&format!("#{}={};\n", i + 1, body));
    }
    text.push_str("ENDSEC;\nEND-ISO-10303-21;\n");
    Ok(IfcOut { text, counts })
}
