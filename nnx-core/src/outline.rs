//! The project outliner: the hierarchy `Site -> Levels -> Spaces / Envelope / Structural / MEP -> Schedule and Execution`, built from the
//! model and the project and nothing else.
//!
//! Every node names the model elements it stands for (`ids`): a leaf is one element, a group is all the elements under it. The page uses
//! that to highlight the same thing in the 2D plan, the 3D view and the tables, and a node that belongs to a tab (the schedule, a cost
//! tier, a payment milestone, a QA/QC stage ...) carries a `goto` to open it. The outliner holds no figure of its own.

use crate::building;
use crate::model::{Id, Model};
use crate::project::NanoArchProject;
use crate::typed::{Space, Wall};
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize)]
pub struct Goto {
    pub tab: &'static str,
    pub section: Option<&'static str>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Node {
    pub key: String,
    pub kind: String,
    pub en: String,
    pub ar: String,
    /// The elements this node stands for (itself for a leaf, everything below it for a group).
    pub ids: Vec<String>,
    pub element: Option<String>,
    /// Level index for a level node and everything below it.
    pub level: Option<i64>,
    pub goto: Option<Goto>,
    /// Number of leaves below.
    pub count: usize,
    pub children: Vec<Node>,
}

/// One scheduled activity with the elements it builds.
#[derive(Debug, Clone)]
pub struct OutlineTask {
    pub key: String,
    pub name: String,
    pub phase: String,
    pub start: String,
    pub finish: String,
    pub elements: Vec<String>,
}

fn node(key: impl Into<String>, kind: &str, en: impl Into<String>, ar: impl Into<String>) -> Node {
    Node { key: key.into(), kind: kind.into(), en: en.into(), ar: ar.into(), ids: Vec::new(), element: None, level: None, goto: None, count: 0, children: Vec::new() }
}

fn goto(mut n: Node, tab: &'static str, section: Option<&'static str>) -> Node {
    n.goto = Some(Goto { tab, section });
    n
}

fn group(key: String, kind: &str, en: &str, ar: &str, children: Vec<Node>) -> Option<Node> {
    if children.is_empty() {
        return None;
    }
    let mut n = node(key, kind, en, ar);
    n.children = children;
    Some(rollup(n))
}

/// Fills `ids` and `count` of a node from its children.
fn rollup(mut n: Node) -> Node {
    if !n.children.is_empty() {
        n.ids = n.children.iter().flat_map(|c| c.ids.iter().cloned()).collect();
        n.ids.sort();
        n.ids.dedup();
        n.count = n.children.iter().map(|c| c.count.max(1)).sum();
    }
    n
}

fn leaf(id: Id, kind: &str, label: String) -> Node {
    let mut n = node(id.to_string(), kind, label.clone(), label);
    n.ids = vec![id.to_string()];
    n.element = Some(id.to_string());
    n.count = 1;
    n
}

pub fn build(model: &Model, project: Option<&NanoArchProject>, labels: &BTreeMap<String, String>, tasks: Option<&[OutlineTask]>) -> Node {
    let title = project.map(|p| p.meta.title.trim().to_string()).filter(|t| !t.is_empty()).unwrap_or_else(|| "NanoArch project".into());
    let area = project.and_then(NanoArchProject::plot_area_m2);
    let mut site = node("site", "Site", format!("Site — {title}{}", area.map_or(String::new(), |a| format!(" ({a:.0} m²)"))), format!("الموقع — {title}{}", area.map_or(String::new(), |a| format!(" ({a:.0} م²)"))));

    // Levels, lowest first; elements with no level belong to the lowest (or to a single implicit one).
    let lv = building::levels(model);
    let mut levels: Vec<(i64, Option<Id>, String)> = lv
        .iter()
        .map(|(i, id)| (*i, Some(*id), model.get(id).and_then(|e| e.props.get("name")).and_then(|v| v.as_str()).map(str::to_string).filter(|n| !n.trim().is_empty()).unwrap_or_else(|| format!("Level {}", i + 1))))
        .collect();
    if levels.is_empty() {
        levels.push((0, None, "Level 1".into()));
    }
    let lowest = levels[0].1;
    let level_of = |lid: Option<Id>| -> usize { lid.and_then(|l| levels.iter().position(|x| x.1 == Some(l))).unwrap_or_else(|| levels.iter().position(|x| x.1 == lowest).unwrap_or(0)) };

    let mut per: Vec<[Vec<Node>; 5]> = levels.iter().map(|_| Default::default()).collect();
    let mut spaces: Vec<(usize, String, Node)> = Vec::new();
    for e in model.elements.values() {
        if e.kind == "Project" || e.kind == "Level" {
            continue;
        }
        let li = level_of(e.level_id);
        let known = labels.get(&e.id.to_string()).cloned();
        let (slot, label) = match e.kind.as_str() {
            "Space" => {
                let key = e.props.get("requirementKey").and_then(|v| v.as_str()).unwrap_or("room").to_string();
                let name = project.and_then(|p| p.labels.get(&key)).cloned().unwrap_or_else(|| key.clone());
                let a = e.typed::<Space>().map(|s| s.area).unwrap_or(0.0);
                spaces.push((li, key.clone(), leaf(e.id, "Space", format!("{name} · {a:.1} m²"))));
                continue;
            }
            "Wall" => (1, e.typed::<Wall>().map(|w| format!("Wall {}{}", w.room_a, w.room_b.map_or(String::new(), |b| format!(" / {b}")))).unwrap_or_else(|_| "Wall".into())),
            "Door" | "Window" => (1, known.unwrap_or_else(|| e.kind.clone())),
            "Column" | "Beam" | "Slab" | "Footing" | "TieBeam" => (2, known.unwrap_or_else(|| e.kind.clone())),
            "MepPoint" => (3, format!("{} {}", e.props.get("role").and_then(|v| v.as_str()).unwrap_or("point"), e.props.get("room").and_then(|v| v.as_str()).unwrap_or(""))),
            "MepRun" => (3, format!("{} run", e.props.get("system").and_then(|v| v.as_str()).unwrap_or("MEP"))),
            "Grid" => (4, format!("Grid {}", e.props.get("name").and_then(|v| v.as_str()).unwrap_or(""))),
            "Dimension" => (4, format!("Dimension {} mm", (e.props.get("b").and_then(|v| v.as_i64()).unwrap_or(0) - e.props.get("a").and_then(|v| v.as_i64()).unwrap_or(0)).abs())),
            "Tag" => (4, "Room tag".to_string()),
            other => (4, other.to_string()),
        };
        per[li][slot].push(leaf(e.id, &e.kind, label.trim().to_string()));
    }
    spaces.sort_by(|a, b| (a.0, &a.1).cmp(&(b.0, &b.1)));
    for (li, _, n) in spaces {
        per[li][0].push(n);
    }

    for (li, (index, lid, name)) in levels.iter().enumerate() {
        let k = |s: &str| format!("lvl{index}:{s}");
        let [spaces, mut env, mut st, mut mep, mut other] = std::mem::take(&mut per[li]);
        for v in [&mut env, &mut st, &mut mep, &mut other] {
            v.sort_by(|a, b| (&a.kind, &a.en).cmp(&(&b.kind, &b.en)));
        }
        let children: Vec<Node> =
            [group(k("spaces"), "Spaces", "Spaces", "الفراغات", spaces), group(k("envelope"), "Envelope", "Envelope", "الغلاف", env), group(k("structural"), "Structural", "Structural", "الإنشائي", st), group(k("mep"), "MEP", "MEP", "الكهروميكانيك", mep), group(k("other"), "Other", "Other", "أخرى", other)].into_iter().flatten().collect();
        let mut n = node(lid.map_or_else(|| format!("lvl{index}"), |l| l.to_string()), "Level", format!("{name}{}", if *index == 0 { " (ground)" } else { "" }), format!("{name}{}", if *index == 0 { " (الأرضي)" } else { "" }));
        n.children = children;
        let mut n = rollup(n);
        n.level = Some(*index);
        for c in &mut n.children {
            c.level = Some(*index);
        }
        if n.children.is_empty() {
            n.count = 0;
        }
        site.children.push(n);
    }

    // Schedule and execution.
    let mut exe = node("exec", "Execution", "Schedule and execution", "الجدول والتنفيذ");
    if let Some(ts) = tasks {
        let mut sched = goto(node("sched", "Schedule", "4D schedule", "الجدول الزمني 4D"), "sched", None);
        for phase in crate::schedule::PHASES {
            let kids: Vec<Node> = ts
                .iter()
                .filter(|t| t.phase == phase)
                .map(|t| {
                    let mut n = goto(node(format!("task:{}", t.key), "Task", format!("{} · {} → {}", t.name, t.start, t.finish), format!("{} · {} → {}", t.name, t.start, t.finish)), "sched", None);
                    n.ids = t.elements.clone();
                    n.count = 1;
                    n
                })
                .collect();
            if let Some(g) = group(format!("phase:{phase}"), "Phase", phase, phase, kids) {
                sched.children.push(g);
            }
        }
        exe.children.push(rollup(sched));
    } else {
        exe.children.push(goto(node("sched", "Schedule", "4D schedule (not computed yet)", "الجدول الزمني 4D (لم يُحسب بعد)"), "sched", None));
    }
    if let Some(p) = project {
        let x = &p.meta.execution;
        let ex = |key: &str, kind: &str, en: &str, ar: &str, sec: &'static str, kids: Vec<Node>| {
            let mut n = goto(node(key, kind, en, ar), "ex", Some(sec));
            n.children = kids;
            n.count = n.children.len().max(1);
            n
        };
        let mut cost_kids = Vec::new();
        for t in crate::costtier::Tier::ALL {
            let lines = x.cost.lines.iter().filter(|l| l.tier == t).count();
            if lines > 0 || x.cost.selected == Some(t) {
                let (en, ar) = t.label();
                cost_kids.push(goto(node(format!("tier:{}", t.id()), "Tier", format!("{en} ({lines})"), format!("{ar} ({lines})")), "ex", Some("cost")));
            }
        }
        exe.children.push(ex("cost", "Cost", "Cost tiers", "تكلفة المستويات", "cost", cost_kids));
        exe.children.push(ex("tender", "Tender", "Tender pack", "حزمة المناقصة", "tender", Vec::new()));
        let pay: Vec<Node> = x.payment.milestones.iter().map(|m| goto(node(format!("ms:{}", m.id), "Milestone", m.name.clone(), m.name.clone()), "ex", Some("pay"))).collect();
        exe.children.push(ex("pay", "Payments", "Payments", "الدفعات", "pay", pay));
        let proc: Vec<Node> = x.procurement.items.iter().map(|i| goto(node(format!("lead:{}", i.id), "LongLead", i.name.clone(), i.name.clone()), "ex", Some("proc"))).collect();
        exe.children.push(ex("proc", "Procurement", "Long-lead procurement", "المشتريات طويلة الأجل", "proc", proc));
        let qa: Vec<Node> = crate::qaqc::STAGES.iter().map(|s| goto(node(format!("qa:{}", s.id), "QaStage", s.en, s.ar), "ex", Some("qa"))).collect();
        exe.children.push(ex("qa", "QA", "QA/QC", "فحص الجودة", "qa", qa));
        if x.roi.enabled || p.meta.building_type == Some(crate::project::BuildingType::ApartmentBuilding) {
            exe.children.push(ex("roi", "Roi", "Apartment ROI", "عائد العمارة", "roi", Vec::new()));
        }
        let snaps: Vec<Node> = p.snapshots.iter().map(|s| goto(node(format!("snap:{}", s.id), "Snapshot", s.name.clone(), s.name.clone()), "vr", None)).collect();
        let mut v = goto(node("versions", "Versions", "Versions", "الإصدارات"), "vr", None);
        v.children = snaps;
        v.count = v.children.len().max(1);
        exe.children.push(v);
        let ovs: Vec<Node> = p.overlays.iter().map(|o| goto(node(format!("ov:{}", o.id), "Overlay", format!("{}{}", o.name, if o.enabled { "" } else { " (off)" }), format!("{}{}", o.name, if o.enabled { "" } else { " (معطّل)" })), "ov", None)).collect();
        let mut o = goto(node("overlays", "Overlays", "Municipal overlays", "قواعد البلدية"), "ov", None);
        o.children = ovs;
        o.count = o.children.len().max(1);
        exe.children.push(o);
    }
    let exe = rollup(exe);
    site.children.push(exe);
    let mut site = rollup(site);
    site.count = site.children.iter().map(|c| c.count).sum();
    site
}
