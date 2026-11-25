//! Arabic texts of the engine's user-facing messages on the main path (start project, generate, notes, finishes).
//!
//! The engine writes each message once, in English, with its numbers and names already in it. This table holds the Arabic wording of the same
//! message; a `{}` stands for a value of the English text and is carried into the Arabic. The shell sends both texts to the page, which shows the
//! one for the chosen language: the page never translates anything itself. A message that is not in the table is returned unchanged, and
//! `localize` says so and `untranslated` hands the same fact to the page, which marks such a message "EN" instead of showing it as if it were Arabic.

/// (English pattern, Arabic pattern). `{}` matches the value written there in the English message.
pub const TABLE: &[(&str, &str)] = &[
    ("max span not set: no extra columns were inserted for long spans", "لم يُحدَّد أقصى بحر: لم تُضف أعمدة إضافية للبحور الطويلة"),
    ("a {} mm span at {} cannot be reduced to {} mm because openings block extra columns", "بحر {} مم عند {} لا يمكن تقليله إلى {} مم لأن الفتحات تمنع إضافة أعمدة"),
    ("{} column(s) continue down from the level above where this level has no junction", "{} عمود/أعمدة تنزل من الدور الأعلى حيث لا تقاطع جدران في هذا الدور"),
    ("{} of them stand inside a room with no wall and no beam: the slab panel they support needs a structural engineer's check", "{} منها داخل فراغ بلا جدار ولا كمرة: لوح البلاطة الذي تحمله يحتاج فحص مهندس إنشائي"),
    ("{} of them stand in a door or window opening: move the opening or the room, or have an engineer provide a transfer", "{} منها في فتحة باب أو نافذة: انقل الفتحة أو الفراغ، أو يضع مهندس عنصر نقل"),
    ("floor loads not set: column loads and footings were not computed (enter dead and live loads from the SBC tables)", "لم تُدخل أحمال الأدوار: لم تُحسب أحمال الأعمدة ولا القواعد (أدخل الحمل الميت والحي من جداول SBC)"),
    ("wall, beam and column self-weight are not included in the loads; add them to the dead load", "الوزن الذاتي للجدران والكمرات والأعمدة غير داخل في الأحمال؛ أضفه إلى الحمل الميت"),
    ("slab of room '{}' has no supporting column", "بلاطة الفراغ '{}' بلا عمود يحملها"),
    ("a column above at ({}, {}) has no column below it: its {} kN is not carried to the foundations", "عمود علوي عند ({}, {}) بلا عمود تحته: حمله {} kN لا يصل إلى الأساسات"),
    ("soil bearing capacity not set: footings were not sized (enter it from the soil investigation report)", "لم تُدخل قدرة تحمل التربة: لم تُحدَّد مقاسات القواعد (أدخلها من تقرير فحص التربة)"),
    ("footing depth and reinforcement are not designed: a licensed structural engineer must complete them", "عمق القواعد وتسليحها غير مصمَّمين: يجب أن يكملهما مهندس إنشائي مرخّص"),
    ("{} door/window position(s) were moved along their wall to clear columns", "نُقل موضع {} باب/نافذة على امتداد جدارها لتفادي الأعمدة"),
    ("{} column(s) on upper levels have no column below them: their load is not carried to the foundations (align the rooms or add transfer beams; an engineer must decide)", "{} عمود/أعمدة في الأدوار العليا بلا عمود تحتها: حملها لا يصل إلى الأساسات (حاذِ الفراغات أو أضف كمرات نقل؛ القرار للمهندس)"),
    ("door and window positions did not settle after 4 passes: check the openings next to columns", "لم تستقر مواضع الأبواب والنوافذ بعد 4 مراحل: راجع الفتحات المجاورة للأعمدة"),
    ("floor {} has no rooms of its own: it repeats floor {}", "الدور {} بلا فراغات خاصة به: يكرر الدور {}"),
    ("the floors input ({}) differs from the {} levels drawn: generate again", "عدد الأدوار المدخل ({}) يخالف الأدوار المرسومة ({}): ولّد من جديد"),
    ("the structure does not match the walls: press Sync", "الإنشائي لا يطابق الجدران: اضغط مزامنة"),
    ("plot {} x {} m, {} floor(s), street {} m at the front: assumptions, edit them", "أرض {} × {} م، {} دور، شارع {} م في الواجهة: افتراضات، عدّلها"),
    ("room areas are starting targets of this application, each above the SBC minimum for its type: review them", "مساحات الفراغات أهداف ابتدائية من التطبيق، كل منها فوق حد SBC الأدنى لنوعه: راجعها"),
    ("the Ministry villa guideline (clause 4-1) is applied because the guidelines give no separate rule for a palace: confirm with the municipality", "طُبّق دليل الوزارة للفلل (البند 4-1) لأن الدليل لا يضع قاعدة مستقلة للقصور: تأكد من البلدية"),
    ("one apartment per floor", "شقة واحدة في كل دور"),
    ("one dwelling per floor; the Ministry villa guideline (clause 4-1) is applied because the guidelines give no separate rule for a duplex: confirm with the municipality", "وحدة سكنية واحدة في كل دور؛ طُبّق دليل الوزارة للفلل (البند 4-1) لأن الدليل لا يضع قاعدة مستقلة للدوبلكس: تأكد من البلدية"),
    ("a stand-alone annex on its own plot; for an annex inside the main plot the setbacks of the whole plot govern: the Ministry villa guideline (clause 4-1) is applied, confirm with the municipality", "ملحق مستقل على أرضه؛ أما الملحق داخل الأرض الرئيسية فتحكمه ارتدادات الأرض كلها: طُبّق دليل الوزارة للفلل (البند 4-1)، تأكد من البلدية"),
    ("no finish chosen yet", "لم يُختر تشطيب بعد"),
    ("no rate for {}: enter one for the code {} in the BOQ library", "لا سعر لـ {}: أدخل سعراً للرمز {} في مكتبة BOQ"),
    ("a door {} mm wide stands in a column on a {} mm wall and cannot be moved clear of it", "باب بعرض {} مم يقع في عمود على جدار طوله {} مم ولا يمكن إبعاده عنه"),
    ("a window {} mm wide stands in a column on a {} mm wall and cannot be moved clear of it", "نافذة بعرض {} مم تقع في عمود على جدار طوله {} مم ولا يمكن إبعادها عنه"),
    // Where each setback came from: the citation (clause, page) is kept as written; only the words around it change.
    ("Ground floor: room '{}' needs daylight but has no window (no exterior wall space)", "الدور الأرضي: الفراغ '{}' يحتاج إضاءة طبيعية ولا نافذة له (لا جدار خارجي متاح)"),
    ("Floor {}: room '{}' needs daylight but has no window (no exterior wall space)", "الدور {}: الفراغ '{}' يحتاج إضاءة طبيعية ولا نافذة له (لا جدار خارجي متاح)"),
    ("{}: front setback, street {} m", "{}: الارتداد الأمامي، شارع {} م"),
    ("{}: side/rear street setback, street {} m", "{}: ارتداد الشارع الجانبي/الخلفي، شارع {} م"),
    ("{}: towards neighbours", "{}: تجاه الجيران"),
    ("{}: towards neighbours, {} floors", "{}: تجاه الجيران، {} أدوار"),
    ("{}: towards squares and plazas", "{}: تجاه الساحات والميادين"),
];

/// The names the engine gives the rooms it makes itself (the stored names stay as they are), with their Arabic for the screen.
pub const ROOM_NAMES: &[(&str, &str)] = &[("Hall", "صالة"), ("Stair hall", "بيت الدرج"), ("Stair", "درج")];

/// The Arabic display name of a generated room name, or None for a name the user wrote.
pub fn room_name_ar(name: &str) -> Option<&'static str> {
    ROOM_NAMES.iter().find(|(en, _)| *en == name).map(|(_, ar)| *ar)
}

/// For each message, true when the table has no Arabic for it (the page then shows it in English with a mark, never silently).
pub fn untranslated<S: AsRef<str>>(list: &[S]) -> Vec<bool> {
    list.iter().map(|m| !localize(m.as_ref()).1).collect()
}

fn split(pat: &str) -> Vec<&str> {
    pat.split("{}").collect()
}

/// The values the English pattern leaves for its `{}` in `msg`, or None when `msg` is not that message.
fn captures<'a>(pat: &str, msg: &'a str) -> Option<Vec<&'a str>> {
    let parts = split(pat);
    let mut rest = msg.strip_prefix(parts[0])?;
    let mut out = Vec::new();
    for (i, lit) in parts.iter().enumerate().skip(1) {
        let last = i == parts.len() - 1;
        let cut = if last {
            if !rest.ends_with(lit) {
                return None;
            }
            rest.len() - lit.len()
        } else {
            rest.find(lit)?
        };
        out.push(&rest[..cut]);
        rest = &rest[cut + lit.len()..];
    }
    rest.is_empty().then_some(out)
}

/// The Arabic text of an English engine message, and whether the table knew it.
pub fn localize(msg: &str) -> (String, bool) {
    for (en, ar) in TABLE {
        if let Some(vals) = captures(en, msg) {
            let mut out = String::new();
            for (i, lit) in split(ar).iter().enumerate() {
                out.push_str(lit);
                if let Some(v) = vals.get(i) {
                    out.push_str(v);
                }
            }
            return (out, true);
        }
    }
    (msg.to_string(), false)
}

/// The Arabic texts of a list, one per message (an unknown message stays as written; `untranslated` tells which).
pub fn localize_all<S: AsRef<str>>(list: &[S]) -> Vec<String> {
    list.iter().map(|m| localize(m.as_ref()).0).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_entry_keeps_the_same_values_in_both_languages() {
        for (en, ar) in TABLE {
            assert_eq!(en.matches("{}").count(), ar.matches("{}").count(), "{en}");
        }
    }

    #[test]
    fn values_are_carried_into_the_arabic() {
        let (ar, known) = localize("3 column(s) continue down from the level above where this level has no junction");
        assert!(known && ar.starts_with("3 عمود"));
        let (ar, known) = localize("a column above at (1200, 3400) has no column below it: its 12.5 kN is not carried to the foundations");
        assert!(known && ar.contains("(1200, 3400)") && ar.contains("12.5 kN"));
        let (ar, known) = localize("something the table does not hold");
        assert!(!known && ar == "something the table does not hold");
    }

    #[test]
    fn the_start_projects_notes_are_all_known() {
        for t in crate::templates::all() {
            for a in &t.assumptions {
                assert!(localize(a).1, "no Arabic for template note: {a}");
            }
        }
    }

    #[test]
    fn the_structural_messages_are_known() {
        for m in [
            "max span not set: no extra columns were inserted for long spans",
            "floor loads not set: column loads and footings were not computed (enter dead and live loads from the SBC tables)",
            "wall, beam and column self-weight are not included in the loads; add them to the dead load",
            "soil bearing capacity not set: footings were not sized (enter it from the soil investigation report)",
            "footing depth and reinforcement are not designed: a licensed structural engineer must complete them",
            "no finish chosen yet",
        ] {
            assert!(localize(m).1, "{m}");
        }
    }
}
