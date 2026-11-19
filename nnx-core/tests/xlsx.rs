//! Excel export. SYNTHETIC test values, checked by hand against the written workbook.

use nnx_core::boq::{self, BoqLib};
use nnx_core::qto::{QtoItem, Unit};
use nnx_core::xlsx::{base64_decode, base64_encode, boq_workbook, col, crc32, zip_stored};

fn item(code: &str, unit: Unit, q: f64) -> QtoItem {
    QtoItem { code: code.into(), description: format!("desc <{code}> & co"), unit, quantity: q, waste_pct: None, sources: Vec::new(), parts: Vec::new() }
}

fn report() -> boq::BoqReport {
    let items = vec![item("SLAB.VOL", Unit::M3, 3.0), item("COLUMN.VOL", Unit::M3, 0.5), item("FLOOR.AREA.bedroom", Unit::M2, 20.0), item("DOOR.900x2100", Unit::Nr, 1.0), item("SPACE.GROSS.AREA", Unit::M2, 20.0)];
    let lib: BoqLib = serde_json::from_value(serde_json::json!({
        "project": {"name": "غرفة <A>", "currency": "XXX"},
        "items": {"SLAB.VOL": {"spec": "spec S", "rate": 100, "waste_pct": 5}, "COLUMN.VOL": {"rate": 200}, "FLOOR.AREA.*": {"rate": 10}},
        "materials": {"SLAB.*": [{"material": "cement", "unit": "bag", "per_unit": 7}]},
        "material_data": {"cement": {"rate": 2}},
        "markups": [{"name": "Overheads", "pct": 10}, {"name": "VAT", "pct": 15}]
    }))
    .unwrap();
    boq::compute(&items, &lib)
}

/// Reads the stored entries of a ZIP back: (name, bytes, crc as written).
fn unzip(z: &[u8]) -> Vec<(String, Vec<u8>, u32)> {
    let u16le = |o: usize| u16::from_le_bytes([z[o], z[o + 1]]) as usize;
    let u32le = |o: usize| u32::from_le_bytes([z[o], z[o + 1], z[o + 2], z[o + 3]]);
    let eocd = z.len() - 22;
    assert_eq!(&z[eocd..eocd + 4], b"PK\x05\x06");
    let (count, cd_off) = (u16le(eocd + 10), u32le(eocd + 16) as usize);
    let mut out = Vec::new();
    let mut p = cd_off;
    for _ in 0..count {
        assert_eq!(&z[p..p + 4], b"PK\x01\x02");
        let (crc, size, name_len, extra, comment, off) = (u32le(p + 16), u32le(p + 24) as usize, u16le(p + 28), u16le(p + 30), u16le(p + 32), u32le(p + 42) as usize);
        let name = String::from_utf8(z[p + 46..p + 46 + name_len].to_vec()).unwrap();
        assert_eq!(&z[off..off + 4], b"PK\x03\x04");
        let (lname, lextra) = (u16le(off + 26), u16le(off + 28));
        let start = off + 30 + lname + lextra;
        out.push((name, z[start..start + size].to_vec(), crc));
        p += 46 + name_len + extra + comment;
    }
    out
}

fn part(z: &[u8], name: &str) -> String {
    String::from_utf8(unzip(z).into_iter().find(|e| e.0 == name).unwrap_or_else(|| panic!("no part {name}")).1).unwrap()
}

#[test]
fn base64_and_crc_match_the_standard_vectors() {
    for (raw, enc) in [("", ""), ("f", "Zg=="), ("fo", "Zm8="), ("foo", "Zm9v"), ("foob", "Zm9vYg=="), ("fooba", "Zm9vYmE="), ("foobar", "Zm9vYmFy")] {
        assert_eq!(base64_encode(raw.as_bytes()), enc);
        assert_eq!(base64_decode(enc).unwrap(), raw.as_bytes());
    }
    let all: Vec<u8> = (0..=255u8).collect();
    assert_eq!(base64_decode(&base64_encode(&all)).unwrap(), all);
    assert!(base64_decode("abc").is_err() && base64_decode("ab!d").is_err());
    assert_eq!(crc32(b"123456789"), 0xCBF4_3926, "the standard CRC-32 check value");
    assert_eq!(col(0), "A");
    assert_eq!(col(25), "Z");
    assert_eq!(col(26), "AA");
    assert_eq!(col(701), "ZZ");
    assert_eq!(col(702), "AAA");
}

#[test]
fn the_zip_reads_back_exactly_with_valid_checksums() {
    let files = vec![("a.txt".to_string(), b"hello".to_vec()), ("dir/b.bin".to_string(), (0..=255u8).cycle().take(10_000).collect())];
    let z = zip_stored(&files);
    let back = unzip(&z);
    assert_eq!(back.len(), 2);
    for ((n, d), (bn, bd, crc)) in files.iter().zip(&back) {
        assert_eq!((n, d), (bn, bd));
        assert_eq!(*crc, crc32(d), "the stored CRC is the data's");
    }
}

#[test]
fn the_workbook_has_the_ooxml_parts_and_four_sheets() {
    let z = boq_workbook(&report(), false).to_bytes();
    let names: Vec<String> = unzip(&z).into_iter().map(|e| e.0).collect();
    for want in ["[Content_Types].xml", "_rels/.rels", "xl/workbook.xml", "xl/_rels/workbook.xml.rels", "xl/styles.xml", "xl/worksheets/sheet1.xml", "xl/worksheets/sheet4.xml"] {
        assert!(names.contains(&want.to_string()), "{want} in {names:?}");
    }
    let wb = part(&z, "xl/workbook.xml");
    for s in ["BOQ", "Materials", "Specifications", "Info"] {
        assert!(wb.contains(&format!("name=\"{s}\"")), "{s}");
    }
    assert!(wb.contains("fullCalcOnLoad=\"1\""), "Excel recalculates on opening");
}

#[test]
fn the_boq_sheet_carries_the_rules_as_formulas_with_hand_checked_values() {
    let z = boq_workbook(&report(), false).to_bytes();
    let s1 = part(&z, "xl/worksheets/sheet1.xml");
    // rows (sorted by section): 2 = SPACE (reference), 3 = COLUMN.VOL, 4 = SLAB.VOL, 5 = DOOR, 6 = FLOOR
    // SLAB.VOL: 3 x 100 = 300; ordered 3 x 1.05 = 3.15
    assert!(s1.contains("<f>IF(J4=\"\",\"\",ROUND(G4*J4,2))</f><v>300</v>"), "{s1}");
    assert!(s1.contains("<f>ROUND(G4*(1+N(H4)/100),3)</f><v>3.15</v>"));
    // COLUMN.VOL: 0.5 x 200 = 100; FLOOR (prefix rule): 20 x 10 = 200
    assert!(s1.contains("<f>IF(J3=\"\",\"\",ROUND(G3*J3,2))</f><v>100</v>"));
    assert!(s1.contains("<f>IF(J6=\"\",\"\",ROUND(G6*J6,2))</f><v>200</v>"));
    // the door has no rate: an empty cached text and the status formula says so
    assert!(s1.contains("<f>IF(J5=\"\",\"\",ROUND(G5*J5,2))</f><v></v>"));
    assert!(s1.contains("<f>IF(J5=\"\",\"no rate\",\"priced\")</f><v>no rate</v>"));
    // the reference row is not priced
    assert!(s1.contains(">reference<"));
    // sub-total 300 + 100 + 200 = 600; overheads 10 % = 60; VAT 15 % of 660 = 99; total 759
    assert!(s1.contains("<f>SUM(K2:K6)</f><v>600</v>"));
    assert!(s1.contains("<f>ROUND(K8*J9/100,2)</f><v>60</v>"), "first mark-up on the sub-total");
    assert!(s1.contains("<f>ROUND((K8+SUM(K9:K9))*J10/100,2)</f><v>99</v>"), "the next on the running total");
    assert!(s1.contains("<f>K8+SUM(K9:K10)</f><v>759</v>"));
    assert!(s1.contains("<f>IF(COUNTIF(L2:L6,\"no rate\")&gt;0,\"PARTIAL\",\"complete\")</f><v>PARTIAL</v>"));
    // the mark-up percentages are inputs in column J
    assert!(s1.contains("<c r=\"J9\" s=\"7\"><v>10</v></c>") && s1.contains("<c r=\"J10\" s=\"7\"><v>15</v></c>"));
    // the header row is frozen and filtered
    assert!(s1.contains("state=\"frozen\"") && s1.contains("<autoFilter ref=\"A1:L6\"/>"));
    assert!(!s1.contains("rightToLeft"));
}

#[test]
fn materials_total_and_escaping_and_right_to_left() {
    let z = boq_workbook(&report(), true).to_bytes();
    let s2 = part(&z, "xl/worksheets/sheet2.xml");
    // cement: ordered 3.15 x 7 = 22.05 bags at a rate of 2: cost 44.1
    assert!(s2.contains("<v>22.05</v>") && s2.contains("<f>IF(E2=\"\",\"\",ROUND(C2*E2,2))</f><v>44.1</v>"), "{s2}");
    assert!(s2.contains("<f>SUM(F2:F2)</f><v>44.1</v>"));
    let s1 = part(&z, "xl/worksheets/sheet1.xml");
    assert!(s1.contains("rightToLeft=\"1\""), "an Arabic workbook opens right to left");
    assert!(s1.contains("desc &lt;SLAB.VOL&gt; &amp; co"), "text is XML-escaped");
    assert!(part(&z, "xl/workbook.xml").contains("name=\"المواد\""));
    let info = part(&z, "xl/worksheets/sheet4.xml");
    assert!(info.contains("غرفة &lt;A&gt;") && info.contains("DOOR.900x2100"), "the missing rate is listed by name");
    assert!(info.contains("إجمالي جزئي"));
    for (name, data, _) in unzip(&z) {
        if name.ends_with(".xml") || name.ends_with(".rels") {
            let t = String::from_utf8(data).unwrap();
            assert_eq!(t.matches("<?xml").count(), 1, "{name}");
            assert_eq!(t.matches('<').count(), t.matches('>').count(), "{name} brackets balance");
        }
    }
}
