//! Automatic furniture arrangement with real dimensions, to test whether a room can hold what it is meant to hold.
//!
//! Every size is an input: the room (mm), each piece (width, depth, count) and the smallest clear walkway. Nothing is assumed;
//! the walkway limit has no default and its absence is reported by name. The arrangement is a deterministic depth-first search
//! over positions that sit flush against a wall or against another piece (or one walkway away from it), trying both
//! orientations of a rotatable piece, south-west first. A candidate position is accepted only when
//!
//! - the piece lies inside the room and overlaps no other piece,
//! - every open gap to a wall or to another piece is at least the walkway (touching leaves no gap and is allowed),
//! - the piece stays out of each door's clear zone (`door_clear_mm` deep, the door's width wide), and
//! - a piece marked `against_wall` touches a wall.
//!
//! A complete arrangement is then checked for reachability on a 50 mm grid: a square of the walkway's size must be able to travel
//! from each door's landing to a side of every piece. Without doors that step is reported `unchecked`, not passed. The share of
//! the room the pieces cover is compared with `max_occupancy_share` when it is given.
//!
//! It is a geometric test of footprints, not an interior design and not a code check: it does not model door swings beyond the clear
//! zone you enter, wall openings, or how a piece is used. "No arrangement found" means this search found none, not that none exists.

use serde::{Deserialize, Serialize};

const GRID_MM: i64 = 50;
const MAX_SIDE_MM: i64 = 20_000;
const MAX_CANDIDATE_CHECKS: u64 = 3_000_000;
const MAX_LEAF_CHECKS: u32 = 100;
const MAX_INSTANCES: usize = 40;

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct RoomDims {
    pub w: i64,
    pub h: i64,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct PieceSpec {
    pub key: String,
    #[serde(default)]
    pub name: Option<String>,
    pub w_mm: i64,
    pub d_mm: i64,
    #[serde(default)]
    pub count: Option<u32>,
    /// The piece must touch a wall (a bed head, a wardrobe, a counter).
    #[serde(default)]
    pub against_wall: bool,
    /// May be turned a quarter turn. `true` when left out.
    #[serde(default)]
    pub rotatable: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct DoorSpec {
    /// `south` (y = 0), `north` (y = h), `west` (x = 0) or `east` (x = w).
    pub wall: String,
    /// Distance from the wall's start (x = 0 or y = 0) to the near jamb.
    pub offset_mm: i64,
    pub width_mm: i64,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct FitSpec {
    pub room: Option<RoomDims>,
    #[serde(default)]
    pub pieces: Vec<PieceSpec>,
    pub min_walkway_mm: Option<i64>,
    #[serde(default)]
    pub doors: Vec<DoorSpec>,
    /// Depth of the clear zone inside each door. Required when doors are given.
    pub door_clear_mm: Option<i64>,
    /// Largest share (0–1) of the room the footprints may cover.
    pub max_occupancy_share: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Placement {
    /// Instance key: the piece key, with `#n` when the piece has several.
    pub key: String,
    pub name: String,
    pub x_mm: i64,
    pub y_mm: i64,
    pub w_mm: i64,
    pub d_mm: i64,
    pub rotated: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Finding {
    pub ar: String,
    pub en: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FitReport {
    /// `fits`, `no_arrangement_found` or `search_limit`.
    pub status: String,
    pub room_area_m2: f64,
    pub pieces_area_m2: f64,
    pub occupancy_share: f64,
    pub free_area_m2: f64,
    pub min_walkway_mm: i64,
    /// The full arrangement when `status` is `fits`, otherwise the most pieces the search managed to place.
    pub placements: Vec<Placement>,
    pub unplaced: Vec<String>,
    /// `pass`, `fail` or `unchecked` (no doors given, or no complete arrangement).
    pub access_check: String,
    /// `pass`, `fail` or `unchecked` (no `max_occupancy_share`).
    pub occupancy_check: String,
    pub findings: Vec<Finding>,
}

#[derive(Debug, Clone)]
struct Item {
    key: String,
    name: String,
    w: i64,
    d: i64,
    against: bool,
    rotatable: bool,
    /// The previous instance is the same piece: positions must increase, which removes mirrored duplicates.
    same_as_prev: bool,
}

#[derive(Debug, Clone, Copy)]
struct Placed {
    item: usize,
    x: i64,
    y: i64,
    w: i64,
    d: i64,
    rotated: bool,
}

#[derive(Debug, Clone, Copy)]
struct Zone {
    x0: i64,
    y0: i64,
    x1: i64,
    y1: i64,
}

struct Search<'a> {
    w: i64,
    h: i64,
    walk: i64,
    items: &'a [Item],
    zones: &'a [Zone],
    doors: bool,
    placed: Vec<Placed>,
    best: Vec<Placed>,
    checks: u64,
    leaves: u32,
    limit_hit: bool,
    found: Option<Vec<Placed>>,
}

fn open_overlap(a0: i64, a1: i64, b0: i64, b1: i64) -> bool {
    a0 < b1 && b0 < a1
}

impl Search<'_> {
    fn valid(&self, i: usize, x: i64, y: i64, w: i64, d: i64) -> bool {
        let it = &self.items[i];
        if x < 0 || y < 0 || x + w > self.w || y + d > self.h {
            return false;
        }
        let gaps = [x, self.w - (x + w), y, self.h - (y + d)];
        if it.against && !gaps.contains(&0) {
            return false;
        }
        if gaps.iter().any(|g| *g > 0 && *g < self.walk) {
            return false;
        }
        if self.zones.iter().any(|z| open_overlap(x, x + w, z.x0, z.x1) && open_overlap(y, y + d, z.y0, z.y1)) {
            return false;
        }
        for p in &self.placed {
            let ox = open_overlap(x, x + w, p.x, p.x + p.w);
            let oy = open_overlap(y, y + d, p.y, p.y + p.d);
            if ox && oy {
                return false;
            }
            if ox {
                let g = if y >= p.y + p.d { y - (p.y + p.d) } else { p.y - (y + d) };
                if g > 0 && g < self.walk {
                    return false;
                }
            } else if oy {
                let g = if x >= p.x + p.w { x - (p.x + p.w) } else { p.x - (x + w) };
                if g > 0 && g < self.walk {
                    return false;
                }
            }
        }
        true
    }

    fn candidates(&self, w: i64, d: i64) -> (Vec<i64>, Vec<i64>) {
        let mut xs = vec![0, self.w - w];
        let mut ys = vec![0, self.h - d];
        for p in &self.placed {
            xs.extend([p.x, p.x + p.w, p.x + p.w + self.walk, p.x - w, p.x - w - self.walk, p.x + p.w - w]);
            ys.extend([p.y, p.y + p.d, p.y + p.d + self.walk, p.y - d, p.y - d - self.walk, p.y + p.d - d]);
        }
        xs.retain(|v| *v >= 0 && *v + w <= self.w);
        ys.retain(|v| *v >= 0 && *v + d <= self.h);
        xs.sort_unstable();
        xs.dedup();
        ys.sort_unstable();
        ys.dedup();
        (xs, ys)
    }

    fn rec(&mut self, i: usize) -> bool {
        if self.placed.len() > self.best.len() {
            self.best = self.placed.clone();
        }
        if i == self.items.len() {
            self.leaves += 1;
            if !self.doors || reachable(self.w, self.h, self.walk, &self.placed, self.zones) {
                self.found = Some(self.placed.clone());
                return true;
            }
            if self.leaves >= MAX_LEAF_CHECKS {
                self.limit_hit = true;
            }
            return false;
        }
        let it = self.items[i].clone();
        let mut orients = vec![(it.w, it.d, false)];
        if it.rotatable && it.w != it.d {
            orients.push((it.d, it.w, true));
        }
        let prev = if it.same_as_prev { self.placed.last().map(|p| (p.y, p.x)) } else { None };
        for (w, d, rotated) in orients {
            let (xs, ys) = self.candidates(w, d);
            for &y in &ys {
                for &x in &xs {
                    if self.limit_hit {
                        return false;
                    }
                    self.checks += 1;
                    if self.checks > MAX_CANDIDATE_CHECKS {
                        self.limit_hit = true;
                        return false;
                    }
                    if prev.is_some_and(|pv| (y, x) <= pv) || !self.valid(i, x, y, w, d) {
                        continue;
                    }
                    self.placed.push(Placed { item: i, x, y, w, d, rotated });
                    if self.rec(i + 1) {
                        return true;
                    }
                    self.placed.pop();
                    if self.limit_hit {
                        return false;
                    }
                }
            }
        }
        false
    }
}

/// Can a walkway-sized square travel from every door landing to a side of every piece?
fn reachable(w: i64, h: i64, walk: i64, placed: &[Placed], zones: &[Zone]) -> bool {
    let (nx, ny) = (((w + GRID_MM - 1) / GRID_MM) as usize, ((h + GRID_MM - 1) / GRID_MM) as usize);
    let half = walk as f64 / 2.0;
    let centre = |i: usize, j: usize| ((i as f64 + 0.5) * GRID_MM as f64, (j as f64 + 0.5) * GRID_MM as f64);
    let mut open = vec![false; nx * ny];
    for j in 0..ny {
        for i in 0..nx {
            let (cx, cy) = centre(i, j);
            let inside = cx - half >= -1e-9 && cy - half >= -1e-9 && cx + half <= w as f64 + 1e-9 && cy + half <= h as f64 + 1e-9;
            open[j * nx + i] = inside
                && !placed.iter().any(|p| cx + half > p.x as f64 + 1e-9 && cx - half < (p.x + p.w) as f64 - 1e-9 && cy + half > p.y as f64 + 1e-9 && cy - half < (p.y + p.d) as f64 - 1e-9);
        }
    }
    let mut seen = vec![false; nx * ny];
    let mut stack = Vec::new();
    for z in zones {
        let mut any = false;
        for j in 0..ny {
            for i in 0..nx {
                let (cx, cy) = centre(i, j);
                if open[j * nx + i] && cx >= z.x0 as f64 - half && cx <= z.x1 as f64 + half && cy >= z.y0 as f64 - half && cy <= z.y1 as f64 + half {
                    any = true;
                    if !seen[j * nx + i] {
                        seen[j * nx + i] = true;
                        stack.push((i, j));
                    }
                }
            }
        }
        if !any {
            return false;
        }
    }
    while let Some((i, j)) = stack.pop() {
        for (di, dj) in [(1i64, 0i64), (-1, 0), (0, 1), (0, -1)] {
            let (ni, nj) = (i as i64 + di, j as i64 + dj);
            if ni < 0 || nj < 0 || ni >= nx as i64 || nj >= ny as i64 {
                continue;
            }
            let k = nj as usize * nx + ni as usize;
            if open[k] && !seen[k] {
                seen[k] = true;
                stack.push((ni as usize, nj as usize));
            }
        }
    }
    placed.iter().all(|p| {
        let reach = half + GRID_MM as f64;
        (0..ny).any(|j| {
            (0..nx).any(|i| {
                if !seen[j * nx + i] {
                    return false;
                }
                let (cx, cy) = centre(i, j);
                cx + reach > p.x as f64 && cx - reach < (p.x + p.w) as f64 && cy + reach > p.y as f64 && cy - reach < (p.y + p.d) as f64
            })
        })
    })
}

fn door_zone(door: &DoorSpec, room: &RoomDims, clear: i64) -> Result<Zone, String> {
    let (o, dw) = (door.offset_mm, door.width_mm);
    if dw <= 0 || o < 0 {
        return Err(format!("furniture fit: the door on the {} wall needs a positive width and an offset of zero or more", door.wall));
    }
    let (len, zone) = match door.wall.as_str() {
        "south" => (room.w, Zone { x0: o, y0: 0, x1: o + dw, y1: clear }),
        "north" => (room.w, Zone { x0: o, y0: room.h - clear, x1: o + dw, y1: room.h }),
        "west" => (room.h, Zone { x0: 0, y0: o, x1: clear, y1: o + dw }),
        "east" => (room.h, Zone { x0: room.w - clear, y0: o, x1: room.w, y1: o + dw }),
        other => return Err(format!("furniture fit: a door's wall must be south, north, west or east, not '{other}'")),
    };
    if o + dw > len {
        return Err(format!("furniture fit: the door on the {} wall runs past the end of the wall ({} mm)", door.wall, len));
    }
    if clear > room.w.min(room.h) {
        return Err("furniture fit: door_clear_mm is larger than the room".to_string());
    }
    Ok(zone)
}

/// Arranges the pieces in the room and reports whether they fit. Missing or impossible inputs are rejected by name.
pub fn arrange(spec: &FitSpec) -> Result<FitReport, String> {
    let room = spec.room.as_ref().ok_or("furniture fit: 'room' { w, h } in millimetres is required")?;
    if room.w <= 0 || room.h <= 0 {
        return Err("furniture fit: the room's w and h must be positive whole millimetres".to_string());
    }
    if room.w > MAX_SIDE_MM || room.h > MAX_SIDE_MM {
        return Err(format!("furniture fit: a room side over {MAX_SIDE_MM} mm is too large for this check"));
    }
    let walk = spec.min_walkway_mm.ok_or("furniture fit: 'min_walkway_mm' (smallest clear walkway) is required; no default is assumed")?;
    if walk <= 0 {
        return Err("furniture fit: 'min_walkway_mm' must be a positive number of millimetres".to_string());
    }
    if spec.pieces.is_empty() {
        return Err("furniture fit: 'pieces' needs at least one piece".to_string());
    }
    if let Some(s) = spec.max_occupancy_share
        && (!s.is_finite() || s <= 0.0 || s > 1.0)
    {
        return Err("furniture fit: 'max_occupancy_share' must be between 0 and 1".to_string());
    }
    let mut zones = Vec::new();
    if !spec.doors.is_empty() {
        let clear = spec.door_clear_mm.ok_or("furniture fit: 'door_clear_mm' (clear depth inside each door) is required when doors are given")?;
        if clear <= 0 {
            return Err("furniture fit: 'door_clear_mm' must be a positive number of millimetres".to_string());
        }
        for d in &spec.doors {
            zones.push(door_zone(d, room, clear)?);
        }
    }
    let mut items: Vec<(usize, Item)> = Vec::new();
    let mut keys = std::collections::HashSet::new();
    for (n, p) in spec.pieces.iter().enumerate() {
        if p.key.trim().is_empty() || !keys.insert(p.key.clone()) {
            return Err(format!("furniture fit: piece {} has an empty or repeated key '{}'", n + 1, p.key));
        }
        if p.w_mm <= 0 || p.d_mm <= 0 {
            return Err(format!("furniture fit: piece '{}' needs a positive w_mm and d_mm", p.key));
        }
        let count = p.count.unwrap_or(1);
        if count == 0 {
            return Err(format!("furniture fit: piece '{}' has a count of zero", p.key));
        }
        for c in 0..count {
            let key = if count > 1 { format!("{}#{}", p.key, c + 1) } else { p.key.clone() };
            items.push((n, Item { key, name: p.name.clone().unwrap_or_else(|| p.key.clone()), w: p.w_mm, d: p.d_mm, against: p.against_wall, rotatable: p.rotatable.unwrap_or(true), same_as_prev: c > 0 }));
        }
    }
    if items.len() > MAX_INSTANCES {
        return Err(format!("furniture fit: {} pieces in total is more than the {MAX_INSTANCES} this check handles", items.len()));
    }
    // Largest footprint first; equal footprints keep the input order so identical instances stay adjacent.
    items.sort_by_key(|(n, it)| (std::cmp::Reverse(it.w * it.d), *n));
    let mut list: Vec<Item> = items.into_iter().map(|(_, it)| it).collect();
    for i in 0..list.len() {
        list[i].same_as_prev = i > 0 && list[i].same_as_prev && list[i - 1].name == list[i].name && list[i - 1].w == list[i].w && list[i - 1].d == list[i].d;
    }

    let room_area = (room.w * room.h) as f64 / 1e6;
    let pieces_area: f64 = list.iter().map(|i| (i.w * i.d) as f64).sum::<f64>() / 1e6;
    let occupancy = pieces_area / room_area;
    let mut findings = Vec::new();
    let mut unfit = Vec::new();
    for it in &list {
        let direct = it.w <= room.w && it.d <= room.h;
        let turned = it.rotatable && it.d <= room.w && it.w <= room.h;
        if !direct && !turned && !unfit.contains(&it.name) {
            unfit.push(it.name.clone());
        }
    }
    for n in &unfit {
        findings.push(Finding { ar: format!("القطعة «{n}» أكبر من الغرفة في كل اتجاه ممكن"), en: format!("'{n}' is larger than the room in every orientation it may take") });
    }

    let mut s = Search { w: room.w, h: room.h, walk, items: &list, zones: &zones, doors: !zones.is_empty(), placed: Vec::new(), best: Vec::new(), checks: 0, leaves: 0, limit_hit: false, found: None };
    if unfit.is_empty() && pieces_area <= room_area {
        s.rec(0);
    }
    let fits = s.found.is_some();
    let status = if fits {
        "fits"
    } else if s.limit_hit {
        "search_limit"
    } else {
        "no_arrangement_found"
    };
    let chosen: Vec<Placed> = s.found.clone().unwrap_or_else(|| s.best.clone());
    let placements: Vec<Placement> = chosen.iter().map(|p| Placement { key: list[p.item].key.clone(), name: list[p.item].name.clone(), x_mm: p.x, y_mm: p.y, w_mm: p.w, d_mm: p.d, rotated: p.rotated }).collect();
    let used: std::collections::HashSet<usize> = chosen.iter().map(|p| p.item).collect();
    let unplaced: Vec<String> = list.iter().enumerate().filter(|(i, _)| !used.contains(i)).map(|(_, it)| it.key.clone()).collect();

    match status {
        "fits" => findings.push(Finding { ar: format!("وُجد ترتيب يتسع لكل القطع ({}) مع ممر لا يقل عن {} مم", list.len(), walk), en: format!("An arrangement holding all {} pieces with walkways of at least {} mm was found", list.len(), walk) }),
        "search_limit" => findings.push(Finding { ar: "بلغ البحث حدّه قبل أن يُحسم الأمر؛ لا يُعلَن نجاح ولا فشل".to_string(), en: "The search reached its limit before it could decide; neither a fit nor a misfit is claimed".to_string() }),
        _ => {
            if pieces_area > room_area {
                findings.push(Finding { ar: "مجموع مساحات القطع أكبر من مساحة الغرفة".to_string(), en: "The pieces' footprints add up to more than the room's area".to_string() });
            }
            findings.push(Finding { ar: format!("لم يجد البحث ترتيباً يتسع لكل القطع بممر {} مم؛ لا يعني ذلك استحالته", walk), en: format!("The search found no arrangement of all pieces with {} mm walkways; that does not prove none exists", walk) });
        }
    }
    let access_check = if !fits || zones.is_empty() { "unchecked" } else { "pass" };
    if fits && zones.is_empty() {
        findings.push(Finding { ar: "لم تُدخل أبواب، فلم يُفحص الوصول إلى القطع".to_string(), en: "No doors were given, so access to the pieces was not checked".to_string() });
    }
    if status == "no_arrangement_found" && !zones.is_empty() && s.leaves > 0 {
        findings.push(Finding { ar: "وُجدت ترتيبات هندسية لكن لا يصل الممر من الباب إلى كل القطع".to_string(), en: "Geometric arrangements exist but none lets a walkway reach every piece from the door".to_string() });
    }
    let occupancy_check = match spec.max_occupancy_share {
        None => "unchecked",
        Some(m) if occupancy <= m + 1e-12 => "pass",
        Some(_) => "fail",
    };
    if occupancy_check == "fail" {
        findings.push(Finding { ar: "نسبة ما تشغله القطع تتجاوز الحد المُدخل".to_string(), en: "The share of the room the pieces cover is over the limit entered".to_string() });
    }
    let r2 = |v: f64| (v * 100.0).round() / 100.0;
    Ok(FitReport {
        status: status.to_string(),
        room_area_m2: r2(room_area),
        pieces_area_m2: r2(pieces_area),
        occupancy_share: (occupancy * 10000.0).round() / 10000.0,
        free_area_m2: r2(room_area - pieces_area),
        min_walkway_mm: walk,
        placements,
        unplaced,
        access_check: access_check.to_string(),
        occupancy_check: occupancy_check.to_string(),
        findings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn piece(key: &str, w: i64, d: i64) -> PieceSpec {
        PieceSpec { key: key.into(), name: None, w_mm: w, d_mm: d, count: None, against_wall: false, rotatable: None }
    }

    fn spec(w: i64, h: i64, pieces: Vec<PieceSpec>) -> FitSpec {
        FitSpec { room: Some(RoomDims { w, h }), pieces, min_walkway_mm: Some(800), doors: vec![], door_clear_mm: None, max_occupancy_share: None }
    }

    #[test]
    fn a_bed_and_wardrobe_fit_a_bedroom_with_walkways() {
        let mut s = spec(4000, 3500, vec![piece("bed", 1600, 2000), piece("wardrobe", 2000, 600)]);
        s.pieces[1].against_wall = true;
        let r = arrange(&s).unwrap();
        assert_eq!(r.status, "fits");
        assert_eq!(r.placements.len(), 2);
        assert!(r.unplaced.is_empty());
        for p in &r.placements {
            assert!(p.x_mm >= 0 && p.y_mm >= 0 && p.x_mm + p.w_mm <= 4000 && p.y_mm + p.d_mm <= 3500);
        }
        let wd = r.placements.iter().find(|p| p.key == "wardrobe").unwrap();
        assert!(wd.x_mm == 0 || wd.y_mm == 0 || wd.x_mm + wd.w_mm == 4000 || wd.y_mm + wd.d_mm == 3500);
        assert_eq!(r.access_check, "unchecked");
        assert_eq!(r.occupancy_check, "unchecked");
    }

    #[test]
    fn open_gaps_under_the_walkway_are_never_produced() {
        let s = spec(5000, 4000, vec![piece("a", 1500, 1000), piece("b", 1500, 1000), piece("c", 1200, 900)]);
        let r = arrange(&s).unwrap();
        assert_eq!(r.status, "fits");
        let ps = &r.placements;
        for (i, a) in ps.iter().enumerate() {
            for b in ps.iter().skip(i + 1) {
                let ox = a.x_mm < b.x_mm + b.w_mm && b.x_mm < a.x_mm + a.w_mm;
                let oy = a.y_mm < b.y_mm + b.d_mm && b.y_mm < a.y_mm + a.d_mm;
                assert!(!(ox && oy), "overlap");
                if ox {
                    let g = (a.y_mm - (b.y_mm + b.d_mm)).max(b.y_mm - (a.y_mm + a.d_mm));
                    assert!(g == 0 || g >= 800, "gap {g}");
                }
                if oy {
                    let g = (a.x_mm - (b.x_mm + b.w_mm)).max(b.x_mm - (a.x_mm + a.w_mm));
                    assert!(g == 0 || g >= 800, "gap {g}");
                }
            }
        }
    }

    #[test]
    fn a_piece_larger_than_the_room_is_named_and_nothing_fits() {
        let r = arrange(&spec(2000, 2000, vec![piece("bed", 1600, 2400), piece("chair", 500, 500)])).unwrap();
        assert_eq!(r.status, "no_arrangement_found");
        assert!(r.findings.iter().any(|f| f.en.contains("'bed' is larger than the room")));
        assert!(r.placements.is_empty());
        assert_eq!(r.unplaced.len(), 2);
    }

    #[test]
    fn a_rotatable_piece_turns_to_fit_and_a_fixed_one_does_not() {
        let mut s = spec(2000, 3500, vec![piece("desk", 2600, 700)]);
        let r = arrange(&s).unwrap();
        assert_eq!(r.status, "fits");
        assert!(r.placements[0].rotated);
        s.pieces[0].rotatable = Some(false);
        assert_eq!(arrange(&s).unwrap().status, "no_arrangement_found");
    }

    #[test]
    fn too_much_furniture_is_reported_not_forced() {
        let r = arrange(&spec(3000, 3000, vec![piece("sofa", 2500, 1000), piece("sofa2", 2500, 1000), piece("sofa3", 2500, 1000)])).unwrap();
        assert_eq!(r.status, "no_arrangement_found");
        assert!(r.placements.len() < 3);
        assert!(!r.unplaced.is_empty());
    }

    #[test]
    fn counts_expand_into_numbered_instances() {
        let mut p = piece("chair", 500, 500);
        p.count = Some(4);
        let r = arrange(&spec(4000, 4000, vec![p])).unwrap();
        assert_eq!(r.status, "fits");
        let mut keys: Vec<_> = r.placements.iter().map(|p| p.key.clone()).collect();
        keys.sort();
        assert_eq!(keys, ["chair#1", "chair#2", "chair#3", "chair#4"]);
    }

    #[test]
    fn a_door_zone_stays_clear_and_a_blocked_door_fails_access() {
        let mut s = spec(4000, 3000, vec![piece("bed", 1600, 2000)]);
        s.doors = vec![DoorSpec { wall: "south".into(), offset_mm: 500, width_mm: 900 }];
        s.door_clear_mm = Some(900);
        let r = arrange(&s).unwrap();
        assert_eq!(r.status, "fits");
        assert_eq!(r.access_check, "pass");
        let b = &r.placements[0];
        let in_zone = b.x_mm < 1400 && b.x_mm + b.w_mm > 500 && b.y_mm < 900;
        assert!(!in_zone, "the bed sits in the door's clear zone");
    }

    #[test]
    fn a_barrier_of_pieces_cuts_the_door_off_from_a_piece() {
        let zone = [Zone { x0: 0, y0: 0, x1: 900, y1: 900 }];
        let wall_of_pieces = [Placed { item: 0, x: 0, y: 1500, w: 4000, d: 500, rotated: false }, Placed { item: 1, x: 1000, y: 2500, w: 600, d: 600, rotated: false }];
        assert!(!reachable(4000, 4000, 800, &wall_of_pieces, &zone));
        let open = [Placed { item: 1, x: 1000, y: 2500, w: 600, d: 600, rotated: false }];
        assert!(reachable(4000, 4000, 800, &open, &zone));
    }

    #[test]
    fn occupancy_limit_is_compared_only_when_given() {
        let mut s = spec(4000, 4000, vec![piece("table", 2000, 1000)]);
        s.max_occupancy_share = Some(0.1);
        let r = arrange(&s).unwrap();
        assert_eq!(r.occupancy_check, "fail");
        assert!((r.occupancy_share - 0.125).abs() < 1e-9);
        s.max_occupancy_share = Some(0.2);
        assert_eq!(arrange(&s).unwrap().occupancy_check, "pass");
    }

    #[test]
    fn missing_or_bad_inputs_are_named() {
        let ok = spec(3000, 3000, vec![piece("a", 500, 500)]);
        let mut no_walk = ok.clone();
        no_walk.min_walkway_mm = None;
        assert!(arrange(&no_walk).unwrap_err().contains("min_walkway_mm"));
        let mut no_room = ok.clone();
        no_room.room = None;
        assert!(arrange(&no_room).unwrap_err().contains("room"));
        let mut empty = ok.clone();
        empty.pieces.clear();
        assert!(arrange(&empty).unwrap_err().contains("pieces"));
        let mut dup = ok.clone();
        dup.pieces.push(piece("a", 400, 400));
        assert!(arrange(&dup).unwrap_err().contains("repeated key"));
        let mut zero = ok.clone();
        zero.pieces[0].w_mm = 0;
        assert!(arrange(&zero).unwrap_err().contains("positive"));
        let mut door = ok.clone();
        door.doors = vec![DoorSpec { wall: "south".into(), offset_mm: 0, width_mm: 900 }];
        assert!(arrange(&door).unwrap_err().contains("door_clear_mm"));
        door.door_clear_mm = Some(900);
        door.doors[0].wall = "up".into();
        assert!(arrange(&door).unwrap_err().contains("south, north, west or east"));
        door.doors[0].wall = "south".into();
        door.doors[0].offset_mm = 2500;
        assert!(arrange(&door).unwrap_err().contains("runs past"));
        let mut share = ok.clone();
        share.max_occupancy_share = Some(1.5);
        assert!(arrange(&share).unwrap_err().contains("max_occupancy_share"));
        let mut big = ok;
        big.room = Some(RoomDims { w: 25_000, h: 3000 });
        assert!(arrange(&big).unwrap_err().contains("too large"));
    }

    #[test]
    fn the_result_is_deterministic() {
        let s = spec(4500, 3800, vec![piece("bed", 1600, 2000), piece("desk", 1400, 700), piece("chair", 500, 500)]);
        assert_eq!(arrange(&s).unwrap(), arrange(&s).unwrap());
    }
}
