//! Excel (`.xlsx`) export of the bill of quantities, with live formulas.
//!
//! The file is written by hand: a workbook is a ZIP of XML parts, so this module has a minimal "stored" (uncompressed)
//! ZIP writer and the few SpreadsheetML parts needed. Nothing here is a model of Excel; it writes only what the BOQ
//! needs: text and number cells, formulas with their computed value cached (so viewers that do not recalculate still
//! show the right numbers), a header style, number formats, column widths, a frozen header row, an auto-filter and a
//! right-to-left sheet for Arabic.
//!
//! The formulas are the BOQ rules themselves, so editing a rate or a mark-up in Excel recalculates the amounts,
//! the sub-total, the mark-ups and the total:
//! - amount = ROUND(quantity x rate, 2); the status is "no rate" while the rate cell is empty;
//! - quantity to order = ROUND(quantity x (1 + waste / 100), 3);
//! - each mark-up is its percentage of the running total (sub-total plus the mark-ups above it);
//! - total = sub-total + all mark-ups; the total says PARTIAL while any item has no rate.

use crate::boq::BoqReport;

// ------------------------------------------------------------------ base64 (for moving the file through the page)

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn base64_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (u32::from(c[0]) << 16) | (u32::from(*c.get(1).unwrap_or(&0)) << 8) | u32::from(*c.get(2).unwrap_or(&0));
        out.push(B64[(n >> 18) as usize & 63] as char);
        out.push(B64[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 { B64[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if c.len() > 2 { B64[n as usize & 63] as char } else { '=' });
    }
    out
}

pub fn base64_decode(s: &str) -> Result<Vec<u8>, String> {
    let val = |c: u8| -> Result<u32, String> { B64.iter().position(|&b| b == c).map(|p| p as u32).ok_or_else(|| format!("invalid base64 character '{}'", c as char)) };
    let bytes: Vec<u8> = s.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    if !bytes.len().is_multiple_of(4) {
        return Err("base64 length is not a multiple of 4".into());
    }
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    for c in bytes.chunks(4) {
        let pad = c.iter().rev().take_while(|&&b| b == b'=').count();
        let mut n = 0u32;
        for &b in &c[..4 - pad] {
            n = (n << 6) | val(b)?;
        }
        n <<= 6 * pad as u32;
        out.push((n >> 16) as u8);
        if pad < 2 {
            out.push((n >> 8) as u8);
        }
        if pad < 1 {
            out.push(n as u8);
        }
    }
    Ok(out)
}

// ------------------------------------------------------------------ ZIP (stored)

/// CRC-32 (IEEE) as ZIP uses it.
pub fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= u32::from(b);
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

/// A ZIP archive with every file stored uncompressed.
pub fn zip_stored(files: &[(String, Vec<u8>)]) -> Vec<u8> {
    let (mut out, mut central) = (Vec::new(), Vec::new());
    let put16 = |v: &mut Vec<u8>, x: u16| v.extend_from_slice(&x.to_le_bytes());
    let put32 = |v: &mut Vec<u8>, x: u32| v.extend_from_slice(&x.to_le_bytes());
    for (name, data) in files {
        let (offset, crc, size) = (out.len() as u32, crc32(data), data.len() as u32);
        out.extend_from_slice(b"PK\x03\x04");
        for x in [20u16, 0x0800, 0, 0, 0x0021] {
            put16(&mut out, x);
        }
        put32(&mut out, crc);
        put32(&mut out, size);
        put32(&mut out, size);
        put16(&mut out, name.len() as u16);
        put16(&mut out, 0);
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(data);

        central.extend_from_slice(b"PK\x01\x02");
        for x in [20u16, 20, 0x0800, 0, 0, 0x0021] {
            put16(&mut central, x);
        }
        put32(&mut central, crc);
        put32(&mut central, size);
        put32(&mut central, size);
        for x in [name.len() as u16, 0, 0, 0, 0] {
            put16(&mut central, x);
        }
        put32(&mut central, 0);
        put32(&mut central, offset);
        central.extend_from_slice(name.as_bytes());
    }
    let cd_offset = out.len() as u32;
    out.extend_from_slice(&central);
    out.extend_from_slice(b"PK\x05\x06");
    for x in [0u16, 0, files.len() as u16, files.len() as u16] {
        put16(&mut out, x);
    }
    put32(&mut out, central.len() as u32);
    put32(&mut out, cd_offset);
    put16(&mut out, 0);
    out
}

// ------------------------------------------------------------------ workbook

#[derive(Debug, Clone)]
pub enum Cell {
    Empty,
    Text(String),
    Num(f64),
    /// A formula and the value it evaluates to (cached in the file).
    FormulaNum(String, f64),
    FormulaText(String, String),
}

/// Cell styles (indices into the style sheet).
pub mod style {
    pub const DEFAULT: u8 = 0;
    pub const HEADER: u8 = 1;
    pub const TEXT: u8 = 2;
    pub const MONEY: u8 = 3;
    pub const QTY: u8 = 4;
    pub const TOTAL_MONEY: u8 = 5;
    pub const TOTAL_TEXT: u8 = 6;
    pub const PCT: u8 = 7;
    pub const BOLD: u8 = 8;
}

pub struct Sheet {
    pub name: String,
    pub rows: Vec<Vec<(Cell, u8)>>,
    pub widths: Vec<f64>,
    pub rtl: bool,
    /// Freeze the first row and put an auto-filter on it, down to the row `filter_to` (1-based; 0 means no filter).
    pub header: bool,
    pub filter_to: usize,
}

fn xml_esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            c if (c as u32) < 0x20 && c != '\n' && c != '\t' => {}
            c => out.push(c),
        }
    }
    out
}

/// XML element text: quotes need no escape there (only in attributes, see `xml_esc`).
fn xml_text(s: &str) -> String {
    xml_esc(s).replace("&quot;", "\"")
}

/// Column letters of a 0-based index: 0 -> A, 25 -> Z, 26 -> AA.
pub fn col(i: usize) -> String {
    let (mut n, mut s) = (i + 1, String::new());
    while n > 0 {
        n -= 1;
        s.insert(0, (b'A' + (n % 26) as u8) as char);
        n /= 26;
    }
    s
}

fn num(v: f64) -> String {
    format!("{v}")
}

impl Sheet {
    fn xml(&self) -> String {
        let mut x = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<worksheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\">");
        let width = self.rows.iter().map(Vec::len).max().unwrap_or(1).max(1);
        x.push_str(&format!("<dimension ref=\"A1:{}{}\"/>", col(width - 1), self.rows.len().max(1)));
        x.push_str(&format!("<sheetViews><sheetView workbookViewId=\"0\"{}>", if self.rtl { " rightToLeft=\"1\"" } else { "" }));
        if self.header {
            x.push_str("<pane ySplit=\"1\" topLeftCell=\"A2\" activePane=\"bottomLeft\" state=\"frozen\"/><selection pane=\"bottomLeft\"/>");
        }
        x.push_str("</sheetView></sheetViews><sheetFormatPr defaultRowHeight=\"15\"/>");
        if !self.widths.is_empty() {
            x.push_str("<cols>");
            for (i, w) in self.widths.iter().enumerate() {
                x.push_str(&format!("<col min=\"{0}\" max=\"{0}\" width=\"{1}\" customWidth=\"1\"/>", i + 1, num(*w)));
            }
            x.push_str("</cols>");
        }
        x.push_str("<sheetData>");
        for (r, row) in self.rows.iter().enumerate() {
            x.push_str(&format!("<row r=\"{}\">", r + 1));
            for (c, (cell, st)) in row.iter().enumerate() {
                let at = format!("{}{}", col(c), r + 1);
                match cell {
                    Cell::Empty => {
                        if *st != 0 {
                            x.push_str(&format!("<c r=\"{at}\" s=\"{st}\"/>"));
                        }
                    }
                    Cell::Text(t) => x.push_str(&format!("<c r=\"{at}\" s=\"{st}\" t=\"inlineStr\"><is><t xml:space=\"preserve\">{}</t></is></c>", xml_text(t))),
                    Cell::Num(v) => x.push_str(&format!("<c r=\"{at}\" s=\"{st}\"><v>{}</v></c>", num(*v))),
                    Cell::FormulaNum(f, v) => x.push_str(&format!("<c r=\"{at}\" s=\"{st}\"><f>{}</f><v>{}</v></c>", xml_text(f), num(*v))),
                    Cell::FormulaText(f, v) => x.push_str(&format!("<c r=\"{at}\" s=\"{st}\" t=\"str\"><f>{}</f><v>{}</v></c>", xml_text(f), xml_text(v))),
                }
            }
            x.push_str("</row>");
        }
        x.push_str("</sheetData>");
        if self.header && self.filter_to > 1 {
            x.push_str(&format!("<autoFilter ref=\"A1:{}{}\"/>", col(width - 1), self.filter_to));
        }
        x.push_str("</worksheet>");
        x
    }
}

const STYLES: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<styleSheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\">\
<numFmts count=\"1\"><numFmt numFmtId=\"164\" formatCode=\"0.000\"/></numFmts>\
<fonts count=\"2\"><font><sz val=\"11\"/><name val=\"Calibri\"/></font><font><b/><sz val=\"11\"/><name val=\"Calibri\"/></font></fonts>\
<fills count=\"3\"><fill><patternFill patternType=\"none\"/></fill><fill><patternFill patternType=\"gray125\"/></fill><fill><patternFill patternType=\"solid\"><fgColor rgb=\"FFEDEDED\"/><bgColor indexed=\"64\"/></patternFill></fill></fills>\
<borders count=\"2\"><border><left/><right/><top/><bottom/><diagonal/></border><border><left style=\"thin\"><color rgb=\"FF999999\"/></left><right style=\"thin\"><color rgb=\"FF999999\"/></right><top style=\"thin\"><color rgb=\"FF999999\"/></top><bottom style=\"thin\"><color rgb=\"FF999999\"/></bottom><diagonal/></border></borders>\
<cellStyleXfs count=\"1\"><xf numFmtId=\"0\" fontId=\"0\" fillId=\"0\" borderId=\"0\"/></cellStyleXfs>\
<cellXfs count=\"9\">\
<xf numFmtId=\"0\" fontId=\"0\" fillId=\"0\" borderId=\"0\" xfId=\"0\"/>\
<xf numFmtId=\"0\" fontId=\"1\" fillId=\"2\" borderId=\"1\" xfId=\"0\" applyFont=\"1\" applyFill=\"1\" applyBorder=\"1\" applyAlignment=\"1\"><alignment horizontal=\"center\" vertical=\"center\" wrapText=\"1\"/></xf>\
<xf numFmtId=\"0\" fontId=\"0\" fillId=\"0\" borderId=\"1\" xfId=\"0\" applyBorder=\"1\" applyAlignment=\"1\"><alignment vertical=\"top\" wrapText=\"1\"/></xf>\
<xf numFmtId=\"4\" fontId=\"0\" fillId=\"0\" borderId=\"1\" xfId=\"0\" applyNumberFormat=\"1\" applyBorder=\"1\"/>\
<xf numFmtId=\"164\" fontId=\"0\" fillId=\"0\" borderId=\"1\" xfId=\"0\" applyNumberFormat=\"1\" applyBorder=\"1\"/>\
<xf numFmtId=\"4\" fontId=\"1\" fillId=\"2\" borderId=\"1\" xfId=\"0\" applyNumberFormat=\"1\" applyFont=\"1\" applyFill=\"1\" applyBorder=\"1\"/>\
<xf numFmtId=\"0\" fontId=\"1\" fillId=\"2\" borderId=\"1\" xfId=\"0\" applyFont=\"1\" applyFill=\"1\" applyBorder=\"1\"/>\
<xf numFmtId=\"2\" fontId=\"0\" fillId=\"0\" borderId=\"1\" xfId=\"0\" applyNumberFormat=\"1\" applyBorder=\"1\"/>\
<xf numFmtId=\"0\" fontId=\"1\" fillId=\"0\" borderId=\"0\" xfId=\"0\" applyFont=\"1\"/>\
</cellXfs><cellStyles count=\"1\"><cellStyle name=\"Normal\" xfId=\"0\" builtinId=\"0\"/></cellStyles></styleSheet>";

pub struct Workbook {
    pub sheets: Vec<Sheet>,
}

impl Workbook {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut files: Vec<(String, Vec<u8>)> = Vec::new();
        let n = self.sheets.len();
        let mut ct = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\"><Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/><Default Extension=\"xml\" ContentType=\"application/xml\"/><Override PartName=\"/xl/workbook.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml\"/><Override PartName=\"/xl/styles.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml\"/>");
        for i in 1..=n {
            ct.push_str(&format!("<Override PartName=\"/xl/worksheets/sheet{i}.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml\"/>"));
        }
        ct.push_str("</Types>");
        files.push(("[Content_Types].xml".into(), ct.into_bytes()));
        files.push(("_rels/.rels".into(), b"<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" Target=\"xl/workbook.xml\"/></Relationships>".to_vec()));
        let mut wb = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<workbook xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\"><bookViews><workbookView/></bookViews><sheets>");
        let mut rels = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">");
        for (i, s) in self.sheets.iter().enumerate() {
            wb.push_str(&format!("<sheet name=\"{}\" sheetId=\"{}\" r:id=\"rId{}\"/>", xml_esc(&s.name), i + 1, i + 1));
            rels.push_str(&format!("<Relationship Id=\"rId{0}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet\" Target=\"worksheets/sheet{0}.xml\"/>", i + 1));
            files.push((format!("xl/worksheets/sheet{}.xml", i + 1), s.xml().into_bytes()));
        }
        wb.push_str("</sheets><calcPr calcId=\"191029\" fullCalcOnLoad=\"1\"/></workbook>");
        rels.push_str(&format!("<Relationship Id=\"rId{}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles\" Target=\"styles.xml\"/></Relationships>", n + 1));
        files.push(("xl/workbook.xml".into(), wb.into_bytes()));
        files.push(("xl/_rels/workbook.xml.rels".into(), rels.into_bytes()));
        files.push(("xl/styles.xml".into(), STYLES.as_bytes().to_vec()));
        zip_stored(&files)
    }
}

// ------------------------------------------------------------------ the BOQ workbook

fn text(s: &str, st: u8) -> (Cell, u8) {
    (Cell::Text(s.to_string()), st)
}

fn opt_num(v: Option<f64>, st: u8) -> (Cell, u8) {
    (v.map_or(Cell::Empty, Cell::Num), st)
}

/// The BOQ, the materials, the specifications and the project information as a workbook with live formulas.
pub fn boq_workbook(r: &BoqReport, arabic: bool) -> Workbook {
    let t = |ar: &'static str, en: &'static str| if arabic { ar } else { en };
    let hdr = |labels: &[&str]| -> Vec<(Cell, u8)> { labels.iter().map(|l| text(l, style::HEADER)).collect() };

    // ---- sheet 1: the BOQ
    let mut rows: Vec<Vec<(Cell, u8)>> = vec![hdr(&[
        t("رقم", "No."),
        t("القسم", "Section"),
        t("الرمز", "Code"),
        t("الوصف", "Description"),
        t("المواصفة", "Specification"),
        t("الوحدة", "Unit"),
        t("الكمية", "Quantity"),
        t("هدر %", "Waste %"),
        t("كمية الطلب", "To order"),
        t("السعر", "Rate"),
        t("المبلغ", "Amount"),
        t("الحالة", "Status"),
    ])];
    for item in &r.rows {
        let n = rows.len() + 1;
        let (amount, status) = if item.reference_only {
            ((Cell::Text(t("مرجعي", "ref.").into()), style::TEXT), (Cell::Text("reference".into()), style::TEXT))
        } else {
            let f = format!("IF(J{n}=\"\",\"\",ROUND(G{n}*J{n},2))");
            (
                match item.amount {
                    Some(a) => (Cell::FormulaNum(f, a), style::MONEY),
                    None => (Cell::FormulaText(f, String::new()), style::MONEY),
                },
                (Cell::FormulaText(format!("IF(J{n}=\"\",\"no rate\",\"priced\")"), if item.amount.is_some() { "priced".into() } else { "no rate".into() }), style::TEXT),
            )
        };
        rows.push(vec![
            text(&item.no, style::TEXT),
            text(&item.section, style::TEXT),
            text(&item.code, style::TEXT),
            text(&item.description, style::TEXT),
            text(item.spec.as_deref().unwrap_or(""), style::TEXT),
            text(&item.unit, style::TEXT),
            (Cell::Num(item.quantity), style::QTY),
            opt_num(item.waste_pct, style::PCT),
            (Cell::FormulaNum(format!("ROUND(G{n}*(1+N(H{n})/100),3)"), item.ordered), style::QTY),
            if item.reference_only { (Cell::Empty, style::MONEY) } else { opt_num(item.rate, style::MONEY) },
            amount,
            status,
        ]);
    }
    let last = rows.len();
    rows.push(vec![]);
    let sub = rows.len() + 1;
    let blank = |c: usize| -> Vec<(Cell, u8)> { (0..c).map(|_| (Cell::Empty, style::TOTAL_TEXT)).collect() };
    let mut sub_row = blank(3);
    sub_row.push(text(t("المجموع الفرعي", "Subtotal"), style::TOTAL_TEXT));
    sub_row.extend(blank(6));
    sub_row.push((Cell::FormulaNum(format!("SUM(K2:K{last})"), r.subtotal), style::TOTAL_MONEY));
    sub_row.push((Cell::Empty, style::TOTAL_TEXT));
    rows.push(sub_row);
    let first_markup = rows.len() + 1;
    for (i, m) in r.markups.iter().enumerate() {
        let n = rows.len() + 1;
        let f = if i == 0 { format!("ROUND(K{sub}*J{n}/100,2)") } else { format!("ROUND((K{sub}+SUM(K{first_markup}:K{}))*J{n}/100,2)", n - 1) };
        let mut row = vec![(Cell::Empty, style::TEXT); 3];
        row.push(text(&m.name, style::TEXT));
        row.extend(vec![(Cell::Empty, style::TEXT); 5]);
        row.push((Cell::Num(m.pct), style::PCT));
        row.push((Cell::FormulaNum(f, m.amount), style::MONEY));
        rows.push(row);
    }
    let total_formula = if r.markups.is_empty() { format!("K{sub}") } else { format!("K{sub}+SUM(K{first_markup}:K{})", rows.len()) };
    let mut total = blank(3);
    total.push(text(&format!("{} {}", t("الإجمالي", "Total"), r.project.currency.as_deref().unwrap_or("")), style::TOTAL_TEXT));
    total.extend(blank(6));
    total.push((Cell::FormulaNum(total_formula, r.total), style::TOTAL_MONEY));
    total.push((Cell::FormulaText(format!("IF(COUNTIF(L2:L{last},\"no rate\")>0,\"PARTIAL\",\"complete\")"), if r.complete { "complete".into() } else { "PARTIAL".into() }), style::TOTAL_TEXT));
    rows.push(total);
    let boq = Sheet { name: "BOQ".into(), rows, widths: vec![8.0, 26.0, 26.0, 46.0, 30.0, 8.0, 12.0, 9.0, 12.0, 12.0, 14.0, 12.0], rtl: arabic, header: true, filter_to: last };

    // ---- sheet 2: materials
    let mut rows = vec![hdr(&[t("المادة", "Material"), t("الوحدة", "Unit"), t("الكمية", "Quantity"), t("المواصفة", "Specification"), t("السعر", "Rate"), t("التكلفة", "Cost"), t("تُستعمل في", "Used in")])];
    for m in &r.materials {
        let n = rows.len() + 1;
        let f = format!("IF(E{n}=\"\",\"\",ROUND(C{n}*E{n},2))");
        rows.push(vec![
            text(&m.shown_name(arabic), style::TEXT),
            text(&m.unit, style::TEXT),
            (Cell::Num(m.quantity), style::QTY),
            text(m.spec.as_deref().unwrap_or(""), style::TEXT),
            opt_num(m.rate, style::MONEY),
            match m.cost {
                Some(c) => (Cell::FormulaNum(f, c), style::MONEY),
                None => (Cell::FormulaText(f, String::new()), style::MONEY),
            },
            text(&m.used_in.join(", "), style::TEXT),
        ]);
    }
    let mlast = rows.len();
    let mut tot = vec![text(t("مجموع التكلفة", "Total cost"), style::TOTAL_TEXT)];
    tot.extend(vec![(Cell::Empty, style::TOTAL_TEXT); 4]);
    tot.push((Cell::FormulaNum(format!("SUM(F2:F{mlast})"), r.materials.iter().filter_map(|m| m.cost).sum()), style::TOTAL_MONEY));
    tot.push((Cell::Empty, style::TOTAL_TEXT));
    rows.push(tot);
    let materials = Sheet { name: t("المواد", "Materials").into(), rows, widths: vec![24.0, 9.0, 14.0, 30.0, 12.0, 14.0, 40.0], rtl: arabic, header: true, filter_to: 0 };

    // ---- sheet 3: specifications
    let mut rows = vec![hdr(&[t("الرمز", "Code"), t("الوصف", "Description"), t("المواصفة", "Specification"), t("المصدر", "Source"), t("الوحدة", "Unit"), t("الكمية", "Quantity")])];
    for s in &r.specs {
        rows.push(vec![text(&s.code, style::TEXT), text(&s.description, style::TEXT), text(s.spec.as_deref().unwrap_or(""), style::TEXT), text(s.source.as_deref().unwrap_or(""), style::TEXT), text(&s.unit, style::TEXT), (Cell::Num(s.quantity), style::QTY)]);
    }
    let specs = Sheet { name: t("المواصفات", "Specifications").into(), rows, widths: vec![26.0, 46.0, 30.0, 20.0, 8.0, 12.0], rtl: arabic, header: true, filter_to: 0 };

    // ---- sheet 4: project information and what is missing
    let p = &r.project;
    let mut rows: Vec<Vec<(Cell, u8)>> = vec![vec![text(t("جدول الكميات (BOQ)", "Bill of Quantities (BOQ)"), style::BOLD)]];
    for (label, v) in [(t("المشروع", "Project"), &p.name), (t("العميل", "Client"), &p.client), (t("الموقع", "Location"), &p.location), (t("التاريخ", "Date"), &p.date), (t("أعدّه", "Prepared by"), &p.prepared_by), (t("العملة", "Currency"), &p.currency)] {
        if let Some(v) = v.as_deref().filter(|v| !v.is_empty()) {
            rows.push(vec![text(label, style::TEXT), text(v, style::TEXT)]);
        }
    }
    rows.push(vec![text(t("الحالة", "Status"), style::TEXT), text(if r.complete { t("إجمالي كامل", "Complete total") } else { t("إجمالي جزئي: بنود بلا سعر", "PARTIAL total: items without a rate") }, style::TEXT)]);
    rows.push(vec![text(t("أولي", "Preliminary"), style::TEXT), text(t("الكميات مقيسة من النموذج؛ المواصفات والأسعار ومعاملات الاستهلاك من مدخلات المستخدم. يراجعه مهندس مرخّص.", "Quantities are measured from the model; specifications, rates and consumption factors are the user's inputs. To be checked by a licensed engineer."), style::TEXT)]);
    rows.push(vec![]);
    for (label, list) in [
        (t("بنود بلا سعر", "Items without a rate"), &r.missing.no_rate),
        (t("بنود بلا مواصفة", "Items without a specification"), &r.missing.no_spec),
        (t("بنود بلا معاملات مواد", "Items without material factors"), &r.missing.no_material_factors),
        (t("أرقام بلا مصدر", "Figures without a source"), &r.missing.no_source),
    ] {
        if !list.is_empty() {
            rows.push(vec![text(&format!("{label} ({})", list.len()), style::BOLD)]);
            for code in list {
                rows.push(vec![(Cell::Empty, style::DEFAULT), text(code, style::DEFAULT)]);
            }
        }
    }
    for w in &r.warnings {
        rows.push(vec![text(t("تنبيه", "Warning"), style::TEXT), text(w, style::TEXT)]);
    }
    let info = Sheet { name: t("معلومات", "Info").into(), rows, widths: vec![34.0, 70.0], rtl: arabic, header: false, filter_to: 0 };

    Workbook { sheets: vec![boq, materials, specs, info] }
}
