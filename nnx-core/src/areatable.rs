//! Room-area schedule read from an Excel workbook (`.xlsx`, `.xlsm`) into brief rooms.
//!
//! - **Columns** are found by their headers, in Arabic or English: the room name (`الغرفة`, `الاسم`, `Room`, `Name`), the area
//!   (`المساحة`, `Area`, `m²`), and optionally the function (`النوع`, `الوظيفة`, `Function`, `Type`) and the floor (`الدور`,
//!   `Floor`, `Level`). The header row is the first of the top 15 rows that has both a name and an area column.
//! - **Units.** Areas are square metres. A header that names another unit (ft², cm², mm²) refuses the sheet: no conversion is
//!   guessed.
//! - **Function.** Taken from the function column when it has one, else from the room's name. A name the table below does not
//!   recognise becomes `other` and is reported, never guessed.
//! - **Floor.** From the floor column (a whole number, 1 is the ground floor, or `ground` / `أرضي`). Without a floor column every
//!   room is on floor 1 and the result says so. A floor cell that cannot be read skips that row and says so.
//! - **Areas.** A row with a name and no usable area is kept with `area_m2 = None` and reported: nothing fills it in.
//! - **Totals.** Rows named `Total` / `المجموع` / `الإجمالي` are skipped and reported.
//!
//! The reading is of the first sheet unless a sheet name is given. Formula cells are read at their cached value.

use crate::briefai::{DraftRoom, FUNCS, norm};
use calamine::{Data, Reader, Xlsx};
use serde::Serialize;
use std::io::Cursor;

/// Header words (normalised) for each column.
const NAME_HEADS: [&str; 7] = ["الغرفه", "اسم الغرفه", "الاسم", "الفراغ", "room", "name", "space"];
const AREA_HEADS: [&str; 6] = ["المساحه", "مساحه", "area", "m2", "sqm", "م2"];
const FUNC_HEADS: [&str; 6] = ["النوع", "الوظيفه", "function", "type", "use", "الاستخدام"];
const FLOOR_HEADS: [&str; 5] = ["الدور", "الطابق", "floor", "level", "storey"];
const TOTAL_WORDS: [&str; 6] = ["المجموع", "الاجمالي", "اجمالي", "total", "sum", "grand total"];
/// Units in a header that are not square metres.
const FOREIGN_UNITS: [&str; 8] = ["ft2", "ft²", "sqft", "sq ft", "قدم", "cm2", "cm²", "mm2"];

/// Function keywords, tried in this order (a bathroom of a bedroom is a bathroom).
const FUNC_WORDS: [(&str, &[&str]); 6] = [
    ("bathroom", &["حمام", "دوره مياه", "دورات مياه", "bathroom", "toilet", "bath", "wc"]),
    ("kitchen", &["مطبخ", "kitchen"]),
    ("dining", &["طعام", "dining"]),
    ("majlis", &["مجلس", "majlis"]),
    ("bedroom", &["نوم", "bedroom", "master"]),
    ("living", &["صاله", "معيشه", "جلوس", "living", "lounge"]),
];

#[derive(Debug, Clone, Serialize)]
pub struct SkippedRow {
    /// 1-based row number in the sheet.
    pub row: usize,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct AreaTable {
    pub sheet: String,
    /// 1-based row of the header.
    pub header_row: usize,
    pub rooms: Vec<DraftRoom>,
    pub skipped: Vec<SkippedRow>,
    pub warnings: Vec<String>,
    /// Sum of the areas that were read, m².
    pub total_area_m2: f64,
}

fn cell_text(d: &Data) -> String {
    match d {
        Data::Empty => String::new(),
        Data::String(s) | Data::DateTimeIso(s) | Data::DurationIso(s) => s.trim().to_string(),
        Data::Float(f) => f.to_string(),
        Data::Int(i) => i.to_string(),
        Data::Bool(b) => b.to_string(),
        Data::DateTime(_) => String::new(),
        Data::Error(_) => String::new(),
    }
}

/// The text rows of a workbook's sheet (the first one, or the one named), with its name.
pub fn read_rows(bytes: &[u8], sheet: Option<&str>) -> Result<(String, Vec<Vec<String>>), String> {
    if bytes.len() < 4 || &bytes[..2] != b"PK" {
        return Err("this is not an .xlsx workbook (an Excel 97-2003 .xls or a CSV is not read: save it as .xlsx)".into());
    }
    let mut wb: Xlsx<_> = Xlsx::new(Cursor::new(bytes)).map_err(|e| format!("the workbook cannot be opened: {e}"))?;
    let names = wb.sheet_names();
    let name = match sheet.map(str::trim).filter(|s| !s.is_empty()) {
        Some(s) => names.iter().find(|n| n.as_str() == s).cloned().ok_or_else(|| format!("no sheet named '{s}' (the sheets are: {})", names.join(", ")))?,
        None => names.first().cloned().ok_or("the workbook has no sheets")?,
    };
    let range = wb.worksheet_range(&name).map_err(|e| format!("the sheet '{name}' cannot be read: {e}"))?;
    // `range` starts at its first used cell: put the rows back at their sheet position so row numbers match Excel.
    let (r0, c0) = range.start().map_or((0, 0), |(r, c)| (r as usize, c as usize));
    let mut rows: Vec<Vec<String>> = vec![Vec::new(); r0];
    for r in range.rows() {
        let mut v = vec![String::new(); c0];
        v.extend(r.iter().map(cell_text));
        rows.push(v);
    }
    Ok((name, rows))
}

fn find_col(heads: &[&str], cells: &[String]) -> Option<usize> {
    cells.iter().position(|c| {
        let n = norm(c);
        let n = n.trim();
        // a header like "المساحة (م²)" or "Area m2" still counts: the head word must start a word of it.
        !n.is_empty() && heads.iter().any(|h| n == *h || n.split(|ch: char| !ch.is_alphanumeric()).any(|w| w == *h) || n.starts_with(h))
    })
}

/// The function of a room from a text, or `None` when no keyword matches.
pub fn function_of(text: &str) -> Option<&'static str> {
    let n = norm(text);
    let tokens: Vec<&str> = n.split(|c: char| !c.is_alphanumeric()).filter(|t| !t.is_empty()).collect();
    FUNC_WORDS.iter().find(|(_, words)| words.iter().any(|w| if *w == "wc" || *w == "bath" { tokens.contains(w) } else { n.contains(w) })).map(|(f, _)| *f)
}

/// A number written in a cell: digits (ASCII or Arabic-Indic), an optional decimal point or comma, and an optional m² suffix.
fn parse_area(text: &str) -> Option<f64> {
    let n = norm(text);
    let n = n.trim();
    let n = ["m2", "m²", "sqm", "م2", "م²", "متر مربع", "م"].iter().find_map(|u| n.strip_suffix(u)).unwrap_or(n).trim();
    if n.is_empty() || !n.chars().all(|c| c.is_ascii_digit() || c == '.' || c == ',' || c == '٫') {
        return None;
    }
    n.replace(['٫', ','], ".").parse::<f64>().ok().filter(|v| v.is_finite())
}

fn parse_floor(text: &str) -> Option<u32> {
    let n = norm(text);
    let n = n.trim();
    if matches!(n, "ground" | "ground floor" | "gf" | "ارضي" | "الارضي" | "الدور الارضي") {
        return Some(1);
    }
    n.parse::<u32>().ok().filter(|f| (1..=20).contains(f))
}

/// Reads the room schedule out of a table of text rows.
pub fn parse_rows(sheet: &str, rows: &[Vec<String>]) -> Result<AreaTable, String> {
    let (hi, name_col, area_col) = rows
        .iter()
        .enumerate()
        .take(15)
        .find_map(|(i, r)| Some((i, find_col(&NAME_HEADS, r)?, find_col(&AREA_HEADS, r)?)))
        .ok_or("no header row found in the first 15 rows: it needs a room-name column (الغرفة / Room) and an area column (المساحة / Area)")?;
    let header = &rows[hi];
    let area_head = norm(&header[area_col]);
    if let Some(u) = FOREIGN_UNITS.iter().find(|u| area_head.contains(&norm(u))) {
        return Err(format!("the area column header '{}' names a unit ({u}) other than m²: convert the column to square metres, no conversion is guessed", header[area_col].trim()));
    }
    let func_col = find_col(&FUNC_HEADS, header).filter(|c| *c != name_col && *c != area_col);
    let floor_col = find_col(&FLOOR_HEADS, header).filter(|c| *c != name_col && *c != area_col && Some(*c) != func_col);

    let mut out = AreaTable { sheet: sheet.to_string(), header_row: hi + 1, rooms: Vec::new(), skipped: Vec::new(), warnings: Vec::new(), total_area_m2: 0.0 };
    if floor_col.is_none() {
        out.warnings.push("no floor column: every room is on floor 1, set the floors before generating".into());
    }
    let get = |r: &Vec<String>, c: usize| r.get(c).map(|s| s.trim().to_string()).unwrap_or_default();
    for (i, r) in rows.iter().enumerate().skip(hi + 1) {
        let row = i + 1;
        let name = get(r, name_col);
        let area_text = get(r, area_col);
        if name.is_empty() && area_text.is_empty() {
            continue; // an empty line
        }
        if name.is_empty() {
            out.skipped.push(SkippedRow { row, reason: format!("an area ({area_text}) with no room name") });
            continue;
        }
        let nn = norm(&name);
        if TOTAL_WORDS.iter().any(|w| nn.trim() == *w || nn.trim().starts_with(&format!("{w} "))) {
            out.skipped.push(SkippedRow { row, reason: "a totals row".into() });
            continue;
        }
        let floor = match floor_col {
            None => 1,
            Some(c) => match parse_floor(&get(r, c)) {
                Some(f) => f,
                None => {
                    out.skipped.push(SkippedRow { row, reason: format!("the floor '{}' of '{name}' is not a whole number from 1 to 20 (1 is the ground floor)", get(r, c)) });
                    continue;
                }
            },
        };
        let from_column = func_col.map(|c| get(r, c)).filter(|t| !t.is_empty());
        let func = match from_column.as_deref() {
            Some(t) if FUNCS.contains(&norm(t).trim()) => FUNCS.iter().find(|f| **f == norm(t).trim()).copied(),
            Some(t) => function_of(t).or_else(|| function_of(&name)),
            None => function_of(&name),
        };
        let func = func.unwrap_or_else(|| {
            out.warnings.push(format!("row {row}: the function of '{name}' is not recognised: it is 'other'"));
            "other"
        });
        let area = match parse_area(&area_text) {
            Some(a) if a > 0.0 => Some(a),
            Some(_) => {
                out.warnings.push(format!("row {row}: '{name}' has an area of zero or less: enter it"));
                None
            }
            None => {
                out.warnings.push(format!("row {row}: '{name}' has no readable area ('{area_text}'): enter it, none is assumed"));
                None
            }
        };
        out.total_area_m2 += area.unwrap_or(0.0);
        out.rooms.push(DraftRoom { func: func.into(), name: Some(name), area_m2: area, area_suggested: false, floor, assumed: false });
    }
    if out.rooms.is_empty() {
        return Err(format!("the sheet '{sheet}' has a header but no room rows under it"));
    }
    Ok(out)
}

/// Reads a workbook's room schedule: the first sheet, or the one named.
pub fn import(bytes: &[u8], sheet: Option<&str>) -> Result<AreaTable, String> {
    let (name, rows) = read_rows(bytes, sheet)?;
    parse_rows(&name, &rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::xlsx::{Cell, Sheet, Workbook};

    fn book(rows: Vec<Vec<Cell>>) -> Vec<u8> {
        let rows = rows.into_iter().map(|r| r.into_iter().map(|c| (c, 0u8)).collect()).collect();
        Workbook { sheets: vec![Sheet { name: "Areas".into(), rows, widths: vec![], rtl: false, header: false, filter_to: 0 }] }.to_bytes()
    }
    fn t(s: &str) -> Cell {
        Cell::Text(s.into())
    }

    #[test]
    fn reads_an_arabic_schedule_with_floors_and_a_totals_row() {
        let b = book(vec![
            vec![t("الغرفة"), t("المساحة (م²)"), t("الدور")],
            vec![t("مجلس رجال"), Cell::Num(32.5), Cell::Num(1.0)],
            vec![t("غرفة نوم رئيسية"), Cell::Num(20.0), Cell::Num(2.0)],
            vec![t("حمام غرفة النوم"), t("٦"), Cell::Num(2.0)],
            vec![t("مخزن"), Cell::Num(4.0), t("أرضي")],
            vec![Cell::Empty, Cell::Empty, Cell::Empty],
            vec![t("المجموع"), Cell::Num(62.5), Cell::Empty],
        ]);
        let r = import(&b, None).unwrap();
        assert_eq!(r.header_row, 1);
        let f: Vec<(&str, f64, u32)> = r.rooms.iter().map(|x| (x.func.as_str(), x.area_m2.unwrap(), x.floor)).collect();
        assert_eq!(f, vec![("majlis", 32.5, 1), ("bedroom", 20.0, 2), ("bathroom", 6.0, 2), ("other", 4.0, 1)]);
        assert!(r.warnings.iter().any(|w| w.contains("مخزن") && w.contains("not recognised")));
        assert_eq!(r.skipped.len(), 1);
        assert!(r.skipped[0].reason.contains("totals"));
        assert_eq!(r.skipped[0].row, 7);
        assert!((r.total_area_m2 - 62.5).abs() < 1e-9);
        assert!(r.rooms.iter().all(|x| !x.area_suggested && !x.assumed));
    }

    #[test]
    fn english_headers_a_function_column_and_a_title_above_the_header() {
        let b = book(vec![
            vec![t("Villa schedule")],
            vec![t("Room"), t("Type"), t("Area m2")],
            vec![t("Suite 1"), t("bedroom"), t("18,5")],
            vec![t("Prep"), t("Kitchen"), Cell::Num(12.0)],
        ]);
        let r = import(&b, None).unwrap();
        assert_eq!(r.header_row, 2);
        assert_eq!(r.rooms[0].func, "bedroom");
        assert_eq!(r.rooms[0].area_m2, Some(18.5));
        assert_eq!(r.rooms[1].func, "kitchen");
        assert!(r.warnings.iter().any(|w| w.contains("no floor column")));
        assert!(r.rooms.iter().all(|x| x.floor == 1));
    }

    #[test]
    fn a_missing_area_is_reported_and_never_filled_in() {
        let b = book(vec![vec![t("Room"), t("Area")], vec![t("Kitchen"), Cell::Empty], vec![t("Living"), t("n/a")], vec![t("Majlis"), Cell::Num(-3.0)], vec![t("Dining"), Cell::Num(15.0)]]);
        let r = import(&b, None).unwrap();
        assert_eq!(r.rooms.iter().map(|x| x.area_m2).collect::<Vec<_>>(), vec![None, None, None, Some(15.0)]);
        assert_eq!(r.warnings.iter().filter(|w| w.contains("enter it")).count(), 3);
        assert!((r.total_area_m2 - 15.0).abs() < 1e-9);
    }

    #[test]
    fn a_foreign_unit_an_unreadable_floor_and_a_missing_header_are_refused() {
        let b = book(vec![vec![t("Room"), t("Area (sq ft)")], vec![t("Kitchen"), Cell::Num(120.0)]]);
        assert!(import(&b, None).unwrap_err().contains("other than m²"));
        let b = book(vec![vec![t("Room"), t("Area"), t("Floor")], vec![t("Kitchen"), Cell::Num(12.0), t("roof")], vec![t("Majlis"), Cell::Num(30.0), Cell::Num(1.0)]]);
        let r = import(&b, None).unwrap();
        assert_eq!(r.rooms.len(), 1);
        assert!(r.skipped[0].reason.contains("roof"));
        let b = book(vec![vec![t("Item"), t("Qty")], vec![t("Kitchen"), Cell::Num(12.0)]]);
        assert!(import(&b, None).unwrap_err().contains("no header row"));
        let b = book(vec![vec![t("Room"), t("Area")]]);
        assert!(import(&b, None).unwrap_err().contains("no room rows"));
    }

    #[test]
    fn wrong_files_and_wrong_sheet_names_are_refused() {
        assert!(import(b"Room,Area\nKitchen,12", None).unwrap_err().contains("not an .xlsx"));
        assert!(import(b"PK\x03\x04garbage", None).unwrap_err().contains("cannot be opened"));
        let b = book(vec![vec![t("Room"), t("Area")], vec![t("Kitchen"), Cell::Num(12.0)]]);
        let e = import(&b, Some("Missing")).unwrap_err();
        assert!(e.contains("no sheet named 'Missing'") && e.contains("Areas"));
        assert_eq!(import(&b, Some("Areas")).unwrap().rooms.len(), 1);
    }

    #[test]
    fn reads_a_deflate_compressed_workbook_as_excel_writes_it() {
        let r = import(include_bytes!("../tests/data/areas_deflated.xlsx"), None).unwrap();
        assert_eq!(r.sheet, "S");
        assert_eq!((r.rooms[0].func.as_str(), r.rooms[0].area_m2), ("kitchen", Some(14.5)));
    }

    #[test]
    fn function_keywords_prefer_the_bathroom_and_the_dining_room() {
        assert_eq!(function_of("حمام غرفة النوم"), Some("bathroom"));
        assert_eq!(function_of("صالة طعام"), Some("dining"));
        assert_eq!(function_of("Master bedroom"), Some("bedroom"));
        assert_eq!(function_of("WC"), Some("bathroom"));
        assert_eq!(function_of("Store"), None);
    }
}
