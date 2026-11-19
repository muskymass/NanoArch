//! Structural analytical export for analysis programs (ETABS, SAFE, Robot): gridlines, column nodes, analytical beam and column
//! centrelines, slab boundaries and the column loads, as DXF (a flat drawing with the data as layers and text) and as IFC4 structural
//! analysis view (`IfcStructuralAnalysisModel` with point connections, curve members, surface members and point actions).
//!
//! What it holds: positions and sections from the model; the SERVICE load of a column is the take-down the model holds
//! (`axialServiceKn`), the ULTIMATE load is that times YOUR load factor (without one only the service load is written). Nodes are at
//! the slab level of each storey, columns run between the nodes of consecutive levels, ground-level column bases are the supports.
//!
//! Limits: the IFC file was written to the IFC4 schema's entity layouts and its structure (references, attribute counts) is checked
//! by the tests; it has NOT been opened in a third-party viewer here, so import it into your analysis program and check.

use crate::ifc::{ifc_guid, step_str};
use crate::model::Model;
use crate::model3d;
use crate::typed::{Beam, Column, Slab, Typed};
use serde::Serialize;
use std::collections::BTreeMap;
use std::fmt::Write;

#[derive(Debug, Clone, Serialize)]
pub struct Node {
    pub name: String,
    /// Millimetres.
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub support: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Member {
    pub name: String,
    /// `beam` or `column`.
    pub kind: &'static str,
    pub a: usize,
    pub b: usize,
    pub width_mm: f64,
    pub depth_mm: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Surface {
    pub name: String,
    pub corners: [[f64; 3]; 4],
    pub thickness_mm: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Load {
    pub node: usize,
    pub column: String,
    pub service_kn: f64,
    pub ultimate_kn: Option<f64>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Analytical {
    pub nodes: Vec<Node>,
    pub members: Vec<Member>,
    pub surfaces: Vec<Surface>,
    pub loads: Vec<Load>,
    pub grid_x: Vec<f64>,
    pub grid_y: Vec<f64>,
    pub notes: Vec<String>,
}

fn node_at(nodes: &mut Vec<Node>, x: f64, y: f64, z: f64, tol: f64) -> usize {
    if let Some(i) = nodes.iter().position(|n| (n.x - x).abs() <= tol && (n.y - y).abs() <= tol && (n.z - z).abs() <= 1.0) {
        return i;
    }
    nodes.push(Node { name: format!("N{}", nodes.len() + 1), x, y, z, support: false });
    nodes.len() - 1
}

pub fn build(model: &Model, load_factor: Option<f64>) -> Result<Analytical, String> {
    let scene = model3d::build(model);
    let levels = &scene.levels;
    let z_of = |lv: Option<crate::model::Id>| -> (f64, f64) {
        let idx = lv.and_then(|l| crate::building::levels(model).iter().find(|x| x.1 == l).map(|x| x.0));
        idx.and_then(|i| levels.iter().find(|l| l.index == i)).map_or((0.0, scene.storey_height * 1000.0), |l| (l.z0 * 1000.0, l.storey * 1000.0))
    };
    let names = crate::structx::schedule_names(model, None);
    let mut a = Analytical::default();
    let tol = 150.0;
    let mut cols: Vec<(crate::model::Id, Column, Option<crate::model::Id>)> = model.elements.values().filter(|e| e.kind == Column::KIND).filter_map(|e| Some((e.id, e.typed::<Column>().ok()?, e.level_id))).collect();
    if cols.is_empty() {
        return Err("the model has no columns: sync the structure first".into());
    }
    cols.sort_by_key(|(_, c, l)| (z_of(*l).0 as i64, c.y, c.x));
    let lowest = cols.iter().map(|(_, _, l)| z_of(*l).0 as i64).min().unwrap_or(0);
    for (id, c, lv) in &cols {
        let (z0, storey) = z_of(*lv);
        let top = node_at(&mut a.nodes, c.x as f64, c.y as f64, z0 + storey, tol);
        let base = node_at(&mut a.nodes, c.x as f64, c.y as f64, z0, tol);
        if z0 as i64 == lowest {
            a.nodes[base].support = true;
        }
        let nm = names.get(&id.to_string()).cloned().unwrap_or_else(|| id.to_string());
        a.members.push(Member { name: nm.clone(), kind: "column", a: base, b: top, width_mm: c.b as f64, depth_mm: c.h as f64 });
        if let Some(kn) = c.axial_service_kn {
            a.loads.push(Load { node: top, column: nm, service_kn: kn, ultimate_kn: load_factor.map(|f| (kn * f * 10.0).round() / 10.0) });
        }
        a.grid_x.push(c.x as f64);
        a.grid_y.push(c.y as f64);
    }
    for e in model.elements.values().filter(|e| e.kind == Beam::KIND) {
        let Ok(b) = e.typed::<Beam>() else { continue };
        let (z0, storey) = z_of(e.level_id);
        let z = z0 + storey;
        let (p, q) = (node_at(&mut a.nodes, b.x1 as f64, b.y1 as f64, z, tol), node_at(&mut a.nodes, b.x2 as f64, b.y2 as f64, z, tol));
        if p != q {
            a.members.push(Member { name: names.get(&e.id.to_string()).cloned().unwrap_or_else(|| e.id.to_string()), kind: "beam", a: p, b: q, width_mm: b.b as f64, depth_mm: b.h as f64 });
        }
    }
    for e in model.elements.values().filter(|e| e.kind == Slab::KIND) {
        let Ok(s) = e.typed::<Slab>() else { continue };
        let (z0, storey) = z_of(e.level_id);
        let z = z0 + storey;
        let (x0, y0, x1, y1) = (s.x as f64, s.y as f64, (s.x + s.w) as f64, (s.y + s.h) as f64);
        a.surfaces.push(Surface { name: names.get(&e.id.to_string()).cloned().unwrap_or_else(|| e.id.to_string()), corners: [[x0, y0, z], [x1, y0, z], [x1, y1, z], [x0, y1, z]], thickness_mm: s.thickness as f64 });
    }
    a.members.sort_by(|p, q| p.name.cmp(&q.name));
    a.surfaces.sort_by(|p, q| p.name.cmp(&q.name));
    for g in [&mut a.grid_x, &mut a.grid_y] {
        g.sort_by(|p, q| p.total_cmp(q));
        g.dedup_by(|p, q| (*p - *q).abs() <= tol);
    }
    if load_factor.is_none() {
        a.notes.push("no load factor entered: only the service loads are exported".into());
    }
    if a.loads.is_empty() {
        a.notes.push("the columns hold no axial load: enter the floor loads and sync the structure to get them".into());
    }
    Ok(a)
}

// ------------------------------------------------------------------ DXF

fn g(o: &mut String, code: i32, v: impl std::fmt::Display) {
    let _ = write!(o, "{code}\n{v}\n");
}

fn n(v: f64) -> String {
    let s = format!("{:.3}", if v.abs() < 5e-4 { 0.0 } else { v });
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

const LAYERS: [(&str, i32); 7] = [("S-GRID", 8), ("S-NODE", 1), ("S-COLUMN", 5), ("S-BEAM", 3), ("S-SLAB", 4), ("S-LOAD", 6), ("S-SUPPORT", 2)];

pub fn dxf(a: &Analytical, title: &str) -> String {
    let mut o = String::new();
    g(&mut o, 0, "SECTION");
    g(&mut o, 2, "HEADER");
    g(&mut o, 9, "$ACADVER");
    g(&mut o, 1, "AC1009");
    g(&mut o, 9, "$INSUNITS");
    g(&mut o, 70, 4);
    g(&mut o, 0, "ENDSEC");
    g(&mut o, 0, "SECTION");
    g(&mut o, 2, "TABLES");
    g(&mut o, 0, "TABLE");
    g(&mut o, 2, "LAYER");
    g(&mut o, 70, LAYERS.len());
    for (name, color) in LAYERS {
        g(&mut o, 0, "LAYER");
        g(&mut o, 2, name);
        g(&mut o, 70, 0);
        g(&mut o, 62, color);
        g(&mut o, 6, "CONTINUOUS");
    }
    g(&mut o, 0, "ENDTAB");
    g(&mut o, 0, "ENDSEC");
    g(&mut o, 0, "SECTION");
    g(&mut o, 2, "ENTITIES");
    let line = |o: &mut String, layer: &str, p: [f64; 3], q: [f64; 3]| {
        g(o, 0, "LINE");
        g(o, 8, layer);
        for (k, v) in [(10, p[0]), (20, p[1]), (30, p[2]), (11, q[0]), (21, q[1]), (31, q[2])] {
            g(o, k, n(v));
        }
    };
    let text = |o: &mut String, layer: &str, p: [f64; 3], h: f64, s: &str| {
        g(o, 0, "TEXT");
        g(o, 8, layer);
        for (k, v) in [(10, p[0]), (20, p[1]), (30, p[2])] {
            g(o, k, n(v));
        }
        g(o, 40, n(h));
        g(o, 1, crate::dxf::dxf_text(s));
    };
    let (minx, maxx) = (a.nodes.iter().map(|p| p.x).fold(f64::MAX, f64::min), a.nodes.iter().map(|p| p.x).fold(f64::MIN, f64::max));
    let (miny, maxy) = (a.nodes.iter().map(|p| p.y).fold(f64::MAX, f64::min), a.nodes.iter().map(|p| p.y).fold(f64::MIN, f64::max));
    let ext = 1500.0;
    for (i, x) in a.grid_x.iter().enumerate() {
        line(&mut o, "S-GRID", [*x, miny - ext, 0.0], [*x, maxy + ext, 0.0]);
        text(&mut o, "S-GRID", [*x, miny - ext - 400.0, 0.0], 300.0, &format!("X{}", i + 1));
    }
    for (i, y) in a.grid_y.iter().enumerate() {
        line(&mut o, "S-GRID", [minx - ext, *y, 0.0], [maxx + ext, *y, 0.0]);
        text(&mut o, "S-GRID", [minx - ext - 600.0, *y, 0.0], 300.0, &format!("Y{}", i + 1));
    }
    for (i, p) in a.nodes.iter().enumerate() {
        g(&mut o, 0, "POINT");
        g(&mut o, 8, if p.support { "S-SUPPORT" } else { "S-NODE" });
        for (k, v) in [(10, p.x), (20, p.y), (30, p.z)] {
            g(&mut o, k, n(v));
        }
        g(&mut o, 0, "CIRCLE");
        g(&mut o, 8, if p.support { "S-SUPPORT" } else { "S-NODE" });
        for (k, v) in [(10, p.x), (20, p.y), (30, p.z), (40, 120.0)] {
            g(&mut o, k, n(v));
        }
        text(&mut o, "S-NODE", [p.x + 150.0, p.y + 150.0, p.z], 120.0, &format!("{} z={}", p.name, n(p.z)));
        let _ = i;
    }
    for m in &a.members {
        let (p, q) = (&a.nodes[m.a], &a.nodes[m.b]);
        line(&mut o, if m.kind == "beam" { "S-BEAM" } else { "S-COLUMN" }, [p.x, p.y, p.z], [q.x, q.y, q.z]);
        text(&mut o, if m.kind == "beam" { "S-BEAM" } else { "S-COLUMN" }, [(p.x + q.x) / 2.0, (p.y + q.y) / 2.0, (p.z + q.z) / 2.0], 120.0, &format!("{} {}x{}", m.name, n(m.width_mm), n(m.depth_mm)));
    }
    for s in &a.surfaces {
        g(&mut o, 0, "POLYLINE");
        g(&mut o, 8, "S-SLAB");
        g(&mut o, 66, 1);
        g(&mut o, 70, 9);
        for c in s.corners {
            g(&mut o, 0, "VERTEX");
            g(&mut o, 8, "S-SLAB");
            for (k, v) in [(10, c[0]), (20, c[1]), (30, c[2])] {
                g(&mut o, k, n(v));
            }
            g(&mut o, 70, 32);
        }
        g(&mut o, 0, "SEQEND");
        g(&mut o, 8, "S-SLAB");
        let c = s.corners;
        text(&mut o, "S-SLAB", [(c[0][0] + c[2][0]) / 2.0, (c[0][1] + c[2][1]) / 2.0, c[0][2]], 150.0, &format!("{} t={}", s.name, n(s.thickness_mm)));
    }
    for l in &a.loads {
        let p = &a.nodes[l.node];
        let u = l.ultimate_kn.map_or(String::new(), |u| format!(" Pu={u} kN"));
        text(&mut o, "S-LOAD", [p.x + 150.0, p.y - 250.0, p.z], 120.0, &format!("{} Ps={} kN{u}", l.column, n(l.service_kn)));
    }
    text(&mut o, "S-GRID", [minx, miny - ext - 1200.0, 0.0], 250.0, title);
    g(&mut o, 0, "ENDSEC");
    g(&mut o, 0, "EOF");
    o
}

// ------------------------------------------------------------------ IFC

struct W(Vec<String>);

impl W {
    fn add(&mut self, s: String) -> usize {
        self.0.push(s);
        self.0.len()
    }
}

fn real(v: f64) -> String {
    let t = format!("{:.3}", if v.abs() < 5e-4 { 0.0 } else { v });
    let t = t.trim_end_matches('0');
    if t.ends_with('.') { format!("{t}0").replace(".0", ".") } else { t.to_string() }
}

fn guid(seed: &str) -> String {
    let mut h1: u64 = 0xcbf2_9ce4_8422_2325;
    let mut h2: u64 = 0x8422_2325_cbf2_9ce4 ^ 0x9e37_79b9_7f4a_7c15;
    for b in seed.bytes() {
        h1 = (h1 ^ b as u64).wrapping_mul(0x0000_0100_0000_01b3);
        h2 = (h2 ^ b as u64).wrapping_mul(0x0000_0100_0000_01b3);
    }
    let mut bytes = [0u8; 16];
    bytes[..8].copy_from_slice(&h1.to_be_bytes());
    bytes[8..].copy_from_slice(&h2.to_be_bytes());
    ifc_guid(bytes)
}

pub fn ifc(a: &Analytical, project: &str, timestamp: &str) -> String {
    let mut w = W(Vec::new());
    let p0 = w.add("IFCCARTESIANPOINT((0.,0.,0.))".into());
    let axis0 = w.add(format!("IFCAXIS2PLACEMENT3D(#{p0},$,$)"));
    let ctx = w.add(format!("IFCGEOMETRICREPRESENTATIONCONTEXT($,'Model',3,1.E-05,#{axis0},$)"));
    let u_len = w.add("IFCSIUNIT(*,.LENGTHUNIT.,.MILLI.,.METRE.)".into());
    let u_force = w.add("IFCSIUNIT(*,.FORCEUNIT.,.KILO.,.NEWTON.)".into());
    let units = w.add(format!("IFCUNITASSIGNMENT((#{u_len},#{u_force}))"));
    let proj = w.add(format!("IFCPROJECT('{}',$,{},$,$,$,$,(#{ctx}),#{units})", guid("project"), step_str(project)));
    let _ = proj;
    let model_id_placeholder = w.0.len() + 1;
    let analysis = w.add(format!("IFCSTRUCTURALANALYSISMODEL('{}',$,{},$,$,.LOADING_3D.,$,$,$,$)", guid("analysis"), step_str(&format!("{project} analytical model"))));
    debug_assert_eq!(analysis, model_id_placeholder);
    let placement = |w: &mut W, x: f64, y: f64, z: f64| {
        let p = w.add(format!("IFCCARTESIANPOINT(({},{},{}))", real(x), real(y), real(z)));
        let ax = w.add(format!("IFCAXIS2PLACEMENT3D(#{p},$,$)"));
        w.add(format!("IFCLOCALPLACEMENT($,#{ax})"))
    };
    let shape = |w: &mut W, kind: &str, item: usize| {
        let r = w.add(format!("IFCTOPOLOGYREPRESENTATION(#{ctx},'Reference','{kind}',(#{item}))"));
        w.add(format!("IFCPRODUCTDEFINITIONSHAPE($,$,(#{r}))"))
    };
    let mut items: Vec<usize> = Vec::new();
    let mut node_conn: Vec<usize> = Vec::new();
    let mut node_vertex: Vec<usize> = Vec::new();
    for nd in &a.nodes {
        let pt = w.add(format!("IFCCARTESIANPOINT(({},{},{}))", real(nd.x), real(nd.y), real(nd.z)));
        let v = w.add(format!("IFCVERTEXPOINT(#{pt})"));
        let pl = placement(&mut w, nd.x, nd.y, nd.z);
        let sh = shape(&mut w, "Vertex", v);
        let c = w.add(format!("IFCSTRUCTURALPOINTCONNECTION('{}',$,{},$,{},#{pl},#{sh},$,$)", guid(&format!("node:{}", nd.name)), step_str(&nd.name), if nd.support { "'support'" } else { "'node'" }));
        node_conn.push(c);
        node_vertex.push(v);
        items.push(c);
    }
    for m in &a.members {
        let (p, q) = (&a.nodes[m.a], &a.nodes[m.b]);
        let e = w.add(format!("IFCEDGE(#{},#{})", node_vertex[m.a], node_vertex[m.b]));
        let pl = placement(&mut w, p.x, p.y, p.z);
        let sh = shape(&mut w, "Edge", e);
        let dir = w.add(if m.kind == "beam" { "IFCDIRECTION((0.,0.,1.))".into() } else { "IFCDIRECTION((1.,0.,0.))".into() });
        let c = w.add(format!("IFCSTRUCTURALCURVEMEMBER('{}',$,{},$,{},#{pl},#{sh},.RIGID_JOINED_MEMBER.,#{dir})", guid(&format!("member:{}", m.name)), step_str(&m.name), step_str(&format!("{} {}x{} mm", m.kind, real(m.width_mm), real(m.depth_mm)))));
        let _ = q;
        for (k, conn) in [(0, node_conn[m.a]), (1, node_conn[m.b])] {
            w.add(format!("IFCRELCONNECTSSTRUCTURALMEMBER('{}',$,$,$,#{c},#{conn},$,$,$,$)", guid(&format!("rel:{}:{k}", m.name))));
        }
        items.push(c);
    }
    for s in &a.surfaces {
        let pts: Vec<usize> = s.corners.iter().map(|c| w.add(format!("IFCCARTESIANPOINT(({},{},{}))", real(c[0]), real(c[1]), real(c[2])))).collect();
        let lp = w.add(format!("IFCPOLYLOOP(({}))", pts.iter().map(|p| format!("#{p}")).collect::<Vec<_>>().join(",")));
        let bound = w.add(format!("IFCFACEOUTERBOUND(#{lp},.T.)"));
        let face = w.add(format!("IFCFACE((#{bound}))"));
        let pl = placement(&mut w, s.corners[0][0], s.corners[0][1], s.corners[0][2]);
        let sh = shape(&mut w, "Face", face);
        let m = w.add(format!("IFCSTRUCTURALSURFACEMEMBER('{}',$,{},$,$,#{pl},#{sh},.SHELL.,{})", guid(&format!("slab:{}", s.name)), step_str(&s.name), real(s.thickness_mm)));
        items.push(m);
    }
    let list = |v: &[usize]| v.iter().map(|i| format!("#{i}")).collect::<Vec<_>>().join(",");
    w.add(format!("IFCRELASSIGNSTOGROUP('{}',$,$,$,({}),$,#{analysis})", guid("assign:items"), list(&items)));
    if !a.loads.is_empty() {
        let group = w.add(format!("IFCSTRUCTURALLOADGROUP('{}',$,'Column loads',$,$,.LOAD_CASE.,.PERMANENT_G.,.DEAD_LOAD_G.,$,$)", guid("loadgroup")));
        let mut acts = Vec::new();
        for l in &a.loads {
            let nd = &a.nodes[l.node];
            let kn = l.ultimate_kn.unwrap_or(l.service_kn);
            let load = w.add(format!("IFCSTRUCTURALLOADSINGLEFORCE({},0.,0.,{},$,$,$)", step_str(&format!("{} {}", l.column, if l.ultimate_kn.is_some() { "ultimate" } else { "service" })), real(-kn)));
            let pl = placement(&mut w, nd.x, nd.y, nd.z);
            let act = w.add(format!("IFCSTRUCTURALPOINTACTION('{}',$,{},$,$,#{pl},$,#{load},.GLOBAL_COORDS.,.F.)", guid(&format!("act:{}", l.column)), step_str(&l.column)));
            w.add(format!("IFCRELCONNECTSSTRUCTURALACTIVITY('{}',$,$,$,#{},#{act})", guid(&format!("relact:{}", l.column)), node_conn[l.node]));
            acts.push(act);
        }
        w.add(format!("IFCRELASSIGNSTOGROUP('{}',$,$,$,({}),$,#{group})", guid("assign:loads"), list(&acts)));
    }
    let mut o = String::new();
    let _ = writeln!(o, "ISO-10303-21;\nHEADER;\nFILE_DESCRIPTION(('ViewDefinition [StructuralAnalysisView]'),'2;1');\nFILE_NAME({},{},('NanoArch'),(''),'NanoArch','NanoArch','');\nFILE_SCHEMA(('IFC4'));\nENDSEC;\nDATA;", step_str(&format!("{project}-analytical.ifc")), step_str(timestamp));
    for (i, e) in w.0.iter().enumerate() {
        let _ = writeln!(o, "#{}={e};", i + 1);
    }
    o.push_str("ENDSEC;\nEND-ISO-10303-21;\n");
    let _ = BTreeMap::<u8, u8>::new();
    o
}
