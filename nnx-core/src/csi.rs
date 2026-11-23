//! CSI text export of the analytical model: ETABS E2K and SAFE F2K, lengths in mm and forces in kN.
//!
//! `material` is the NAME of a material the user defines in the analysis program: no strength or modulus is written or assumed.
//! Loads go to the patterns `SERVICE` / `ULTIMATE` (the ultimate one only exists when the user entered a load factor).
//!
//! Limits: written from the documented table layout of these text formats and NOT imported into ETABS/SAFE here (neither program
//! is installed). Import the file, read the program's log and compare the counts in the header comment.

use crate::analytical::{Analytical, Member, Surface};
use std::fmt::Write;

/// Which CSI text format to write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Csi {
    /// ETABS interactive-database text (`$ STORIES`, `$ POINT COORDINATES`, ... tables).
    E2k,
    /// SAFE `TABLE:` text (`Joint=... CoordSys=...` records).
    F2k,
}

fn num(v: f64) -> String {
    let s = format!("{:.3}", if v.abs() < 5e-4 { 0.0 } else { v });
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

fn q(s: &str) -> String {
    s.replace('"', "'")
}

fn pattern_names(a: &Analytical) -> Vec<&'static str> {
    ["SERVICE", "ULTIMATE"].into_iter().filter(|p| a.loads.iter().any(|l| (*p == "ULTIMATE") == l.ultimate_kn.is_some())).collect()
}

pub fn write(a: &Analytical, kind: Csi, title: &str, material: &str) -> Result<String, String> {
    if material.trim().is_empty() {
        return Err("material name is missing: the file references a material you define in the program, none is invented".into());
    }
    if material.chars().any(|c| c == '"' || c.is_control()) {
        return Err("material name must not contain quotes or control characters".into());
    }
    if a.members.is_empty() && a.surfaces.is_empty() {
        return Err("the analytical model is empty".into());
    }
    let mut zs: Vec<f64> = a.nodes.iter().map(|p| p.z).chain(a.surfaces.iter().map(|s| s.corners[0][2])).collect();
    zs.sort_by(|p, r| p.total_cmp(r));
    zs.dedup_by(|p, r| (*p - *r).abs() < 1.0);
    let story_of = |z: f64| zs.iter().position(|v| (v - z).abs() < 1.0).unwrap_or(0);
    let story_name = |i: usize| if i == 0 { "BASE".to_string() } else { format!("STORY{i}") };
    let mut plan: Vec<(f64, f64)> = Vec::new();
    let mut pid = |x: f64, y: f64| -> usize {
        plan.iter().position(|p| (p.0 - x).abs() < 1.0 && (p.1 - y).abs() < 1.0).unwrap_or_else(|| {
            plan.push((x, y));
            plan.len() - 1
        })
    };
    let node_pt: Vec<usize> = a.nodes.iter().map(|p| pid(p.x, p.y)).collect();
    let surf_pts: Vec<[usize; 4]> = a.surfaces.iter().map(|s| [0, 1, 2, 3].map(|k| pid(s.corners[k][0], s.corners[k][1]))).collect();
    // (name, width, depth): the section name is built from the dimensions, so equal sections share one definition.
    let fsec = |m: &Member| (format!("{}{}X{}", if m.kind == "column" { "C" } else { "B" }, num(m.width_mm), num(m.depth_mm)), num(m.width_mm), num(m.depth_mm));
    let ssec = |s: &Surface| (format!("S{}", num(s.thickness_mm)), num(s.thickness_mm));
    let mut frame_secs: Vec<(String, String, String)> = a.members.iter().map(fsec).collect();
    frame_secs.sort();
    frame_secs.dedup();
    let mut shell_secs: Vec<(String, String)> = a.surfaces.iter().map(ssec).collect();
    shell_secs.sort();
    shell_secs.dedup();
    let mat = q(material);
    let mut o = String::new();
    let counts = format!("{} points, {} frames, {} areas, {} loads", plan.len(), a.members.len(), a.surfaces.len(), a.loads.len());
    match kind {
        Csi::E2k => {
            let _ = writeln!(o, "$ File NanoArch export (not imported in ETABS by NanoArch: {counts})\n\n$ CONTROLS\n  UNITS  \"KN\"  \"MM\"\n  TITLE2  \"{}\"\n\n$ STORIES - IN SEQUENCE FROM TOP", q(title));
            for i in (1..zs.len()).rev() {
                let _ = writeln!(o, "  STORY \"{}\"  HEIGHT {}", story_name(i), num(zs[i] - zs[i - 1]));
            }
            let _ = writeln!(o, "  STORY \"BASE\"  ELEVATION {}\n\n$ GRIDS\n  COORDSYSTEM \"GLOBAL\"  TYPE \"CARTESIAN\"  XORIGIN 0  YORIGIN 0  ROTATION 0", num(zs[0]));
            for (i, x) in a.grid_x.iter().enumerate() {
                let _ = writeln!(o, "  GRID \"GLOBAL\"  LABEL \"X{}\"  DIR \"X\"  COORD {}  GRIDHIDE \"No\"  BUBBLELOC \"End\"", i + 1, num(*x));
            }
            for (i, y) in a.grid_y.iter().enumerate() {
                let _ = writeln!(o, "  GRID \"GLOBAL\"  LABEL \"Y{}\"  DIR \"Y\"  COORD {}  GRIDHIDE \"No\"  BUBBLELOC \"End\"", i + 1, num(*y));
            }
            let _ = writeln!(o, "\n$ MATERIAL PROPERTIES\n  (define material \"{mat}\" in ETABS: no strength or modulus is written)\n\n$ FRAME SECTIONS");
            for (s, b, d) in &frame_secs {
                let _ = writeln!(o, "  FRAMESECTION  \"{s}\"  MATERIAL \"{mat}\"  SHAPE \"Concrete Rectangular\"  D {d}  B {b}");
            }
            let _ = writeln!(o, "\n$ SHELL PROPERTIES");
            for (s, t) in &shell_secs {
                let _ = writeln!(o, "  SHELLPROP  \"{s}\"  PROPTYPE \"Slab\"  MATERIAL \"{mat}\"  MODELINGTYPE \"ShellThin\"  SLABTYPE \"Slab\"  THICKNESS {t}");
            }
            let _ = writeln!(o, "\n$ POINT COORDINATES");
            for (i, p) in plan.iter().enumerate() {
                let _ = writeln!(o, "  POINT \"P{}\"  {} {}", i + 1, num(p.0), num(p.1));
            }
            let _ = writeln!(o, "\n$ LINE CONNECTIVITIES");
            for m in &a.members {
                let col = m.kind == "column";
                let span = if col { story_of(a.nodes[m.b].z).saturating_sub(story_of(a.nodes[m.a].z)) } else { 0 };
                let _ = writeln!(o, "  LINE  \"{}\"  {}  \"P{}\"  \"P{}\"  {span}", q(&m.name), if col { "COLUMN" } else { "BEAM" }, node_pt[m.a] + 1, node_pt[m.b] + 1);
            }
            let _ = writeln!(o, "\n$ AREA CONNECTIVITIES");
            for (s, p) in a.surfaces.iter().zip(&surf_pts) {
                let _ = writeln!(o, "  AREA  \"{}\"  FLOOR  4  \"P{}\"  \"P{}\"  \"P{}\"  \"P{}\"  0  0  0  0", q(&s.name), p[0] + 1, p[1] + 1, p[2] + 1, p[3] + 1);
            }
            let _ = writeln!(o, "\n$ POINT ASSIGNS");
            for (i, p) in a.nodes.iter().enumerate().filter(|(_, p)| p.support) {
                let _ = writeln!(o, "  POINTASSIGN  \"P{}\"  \"{}\"  RESTRAINT \"UX UY UZ RX RY RZ\"", node_pt[i] + 1, story_name(story_of(p.z)));
            }
            let _ = writeln!(o, "\n$ LINE ASSIGNS");
            for m in &a.members {
                let z = if m.kind == "column" { a.nodes[m.b].z } else { a.nodes[m.a].z };
                let _ = writeln!(o, "  LINEASSIGN  \"{}\"  \"{}\"  SECTION \"{}\"", q(&m.name), story_name(story_of(z)), fsec(m).0);
            }
            let _ = writeln!(o, "\n$ AREA ASSIGNS");
            for s in &a.surfaces {
                let _ = writeln!(o, "  AREAASSIGN  \"{}\"  \"{}\"  SECTION \"{}\"", q(&s.name), story_name(story_of(s.corners[0][2])), ssec(s).0);
            }
            let _ = writeln!(o, "\n$ LOAD PATTERNS");
            for pat in pattern_names(a) {
                let _ = writeln!(o, "  LOADPATTERN \"{pat}\"  TYPE \"Other\"  SELFWEIGHT 0");
            }
            let _ = writeln!(o, "\n$ POINT OBJECT LOADS");
            for l in &a.loads {
                let (pat, kn) = l.ultimate_kn.map_or(("SERVICE", l.service_kn), |u| ("ULTIMATE", u));
                let _ = writeln!(o, "  POINTLOAD  \"P{}\"  \"{}\"  TYPE \"FORCE\"  LC \"{pat}\"  FZ {}", node_pt[l.node] + 1, story_name(story_of(a.nodes[l.node].z)), num(-kn));
            }
            o.push_str("\n  END\n$ END OF MODEL FILE\n");
        }
        Csi::F2k => {
            let _ = writeln!(o, "File NanoArch export (not imported in SAFE by NanoArch: {counts})\n\nTABLE:  \"PROGRAM CONTROL\"\n   ProgramName=SAFE   Units=\"KN, mm, C\"   ModelName=\"{}\"\n", q(title));
            let _ = writeln!(o, "TABLE:  \"MATERIAL PROPERTIES 01 - GENERAL\"\n   Material=\"{mat}\"   Type=Concrete\n\nTABLE:  \"FRAME SECTION PROPERTIES 01 - GENERAL\"");
            for (s, b, d) in &frame_secs {
                let _ = writeln!(o, "   SectionName=\"{s}\"   Material=\"{mat}\"   Shape=Rectangular   t3={d}   t2={b}");
            }
            let _ = writeln!(o, "\nTABLE:  \"SLAB PROPERTIES 01 - GENERAL\"");
            for (s, t) in &shell_secs {
                let _ = writeln!(o, "   Slab=\"{s}\"   Type=Slab   Material=\"{mat}\"   Thickness={t}");
            }
            let _ = writeln!(o, "\nTABLE:  \"JOINT COORDINATES\"");
            for (i, p) in a.nodes.iter().enumerate() {
                let _ = writeln!(o, "   Joint={}   CoordSys=GLOBAL   CoordType=Cartesian   XorR={}   Y={}   Z={}", i + 1, num(p.x), num(p.y), num(p.z));
            }
            let _ = writeln!(o, "\nTABLE:  \"CONNECTIVITY - FRAME\"");
            for (i, m) in a.members.iter().enumerate() {
                let _ = writeln!(o, "   Frame={}   JointI={}   JointJ={}   Label=\"{}\"", i + 1, m.a + 1, m.b + 1, q(&m.name));
            }
            let _ = writeln!(o, "\nTABLE:  \"FRAME SECTION ASSIGNMENTS\"");
            for (i, m) in a.members.iter().enumerate() {
                let _ = writeln!(o, "   Frame={}   AnalSect=\"{}\"", i + 1, fsec(m).0);
            }
            let _ = writeln!(o, "\nTABLE:  \"JOINT RESTRAINT ASSIGNMENTS\"");
            for (i, _) in a.nodes.iter().enumerate().filter(|(_, p)| p.support) {
                let _ = writeln!(o, "   Joint={}   U1=Yes   U2=Yes   U3=Yes   R1=Yes   R2=Yes   R3=Yes", i + 1);
            }
            let _ = writeln!(o, "\nTABLE:  \"LOAD PATTERN DEFINITIONS\"");
            for pat in pattern_names(a) {
                let _ = writeln!(o, "   LoadPat={pat}   DesignType=Other   SelfWtMult=0");
            }
            let _ = writeln!(o, "\nTABLE:  \"JOINT LOADS - FORCE\"");
            for l in &a.loads {
                let (pat, kn) = l.ultimate_kn.map_or(("SERVICE", l.service_kn), |u| ("ULTIMATE", u));
                let _ = writeln!(o, "   Joint={}   LoadPat={pat}   CoordSys=GLOBAL   F1=0   F2=0   F3={}", l.node + 1, num(-kn));
            }
            let _ = writeln!(o, "\nTABLE:  \"CONNECTIVITY - AREA\"");
            for (i, (s, p)) in a.surfaces.iter().zip(&surf_pts).enumerate() {
                let _ = writeln!(o, "   Area={}   NumJoints=4   Label=\"{}\"   Slab=\"{}\"   Pts={},{},{},{}", i + 1, q(&s.name), ssec(s).0, p[0] + 1, p[1] + 1, p[2] + 1, p[3] + 1);
            }
            o.push_str("\nEND TABLE DATA\n");
        }
    }
    Ok(o)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analytical::{Load, Node};

    fn sample() -> Analytical {
        let nd = |nm: &str, x, y, z, s| Node { name: nm.into(), x, y, z, support: s };
        Analytical {
            nodes: vec![nd("N1", 0.0, 0.0, 0.0, true), nd("N2", 0.0, 0.0, 3000.0, false), nd("N3", 4000.0, 0.0, 3000.0, false)],
            members: vec![
                Member { name: "C1".into(), kind: "column", a: 0, b: 1, width_mm: 300.0, depth_mm: 400.0 },
                Member { name: "B1".into(), kind: "beam", a: 1, b: 2, width_mm: 250.0, depth_mm: 500.0 },
            ],
            surfaces: vec![Surface { name: "S1".into(), corners: [[0.0, 0.0, 3000.0], [4000.0, 0.0, 3000.0], [4000.0, 4000.0, 3000.0], [0.0, 4000.0, 3000.0]], thickness_mm: 150.0 }],
            loads: vec![Load { node: 1, column: "C1".into(), service_kn: 500.0, ultimate_kn: Some(700.0) }],
            grid_x: vec![0.0, 4000.0],
            grid_y: vec![0.0],
            notes: vec![],
        }
    }

    #[test]
    fn e2k_has_tables_and_no_invented_material() {
        let t = write(&sample(), Csi::E2k, "T", "CONC").unwrap();
        for k in ["$ STORIES", "STORY \"STORY1\"  HEIGHT 3000", "STORY \"BASE\"  ELEVATION 0", "LINE  \"C1\"  COLUMN  \"P1\"  \"P1\"  1", "LINE  \"B1\"  BEAM", "SHAPE \"Concrete Rectangular\"  D 400  B 300", "THICKNESS 150", "RESTRAINT", "FZ -700", "LC \"ULTIMATE\""] {
            assert!(t.contains(k), "missing {k}\n{t}");
        }
        assert!(!t.contains("LOADPATTERN \"SERVICE\""));
        assert!(!t.contains("FC ") && !t.contains("MODULUS "));
    }

    #[test]
    fn f2k_joints_frames_and_loads() {
        let t = write(&sample(), Csi::F2k, "T", "CONC").unwrap();
        for k in ["Joint=3   CoordSys=GLOBAL   CoordType=Cartesian   XorR=4000", "Frame=2   JointI=2   JointJ=3", "U3=Yes", "F3=-700", "Area=1   NumJoints=4"] {
            assert!(t.contains(k), "missing {k}\n{t}");
        }
    }

    #[test]
    fn missing_material_or_empty_model_is_refused_by_name() {
        assert!(write(&sample(), Csi::E2k, "T", " ").unwrap_err().contains("material name is missing"));
        assert!(write(&sample(), Csi::F2k, "T", "a\"b").is_err());
        assert!(write(&Analytical::default(), Csi::E2k, "T", "C").is_err());
    }
}
