//! Voice editing of the plan: the text of a spoken instruction (from any speech-to-text, see `transcribe_command` in the
//! application) becomes a PROPOSED edit that the user confirms. Nothing here changes the model.
//!
//! Two instructions are understood, in Arabic or English:
//! - move a room towards a side of the plan ("حرّك المطبخ إلى الشمال", "move the kitchen north"): the room trades places with
//!   the room beside it on that side. The two must share a full wall of the same length (the rule of `swap_rooms`), so the plan
//!   keeps tiling; if no room does, the instruction is refused with that reason instead of being approximated.
//! - swap two named rooms ("بدّل المطبخ مع المجلس", "swap the kitchen with the majlis").
//!
//! Sides are the plan's own frame, as everywhere else in the engine: south is the smaller `y`, north the larger, west the
//! smaller `x`, east the larger. A distance in the instruction ("خمسة أمتار", "2 m") is refused: a swap has no distance, and
//! a distance is never guessed. A room named by function that matches several rooms is reported as ambiguous with the
//! candidates; none is picked. Speech-to-text can mishear, so the result is always `unconfirmed` and carries the transcript.
//!
//! Every message carries both languages as `arabic | english`.

use crate::layout::{Rect, Side};
use serde::Serialize;

/// A room of the current level as the instruction can refer to it.
#[derive(Debug, Clone)]
pub struct VoiceRoom {
    /// The element id, returned untouched in the proposal.
    pub id: String,
    pub key: String,
    pub func: String,
    /// The display name the user sees (may be empty).
    pub name: String,
    pub rect: Rect,
}

/// The edit the user is asked to confirm.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Proposal {
    /// Always `swap_rooms`: the application command that carries it out.
    pub command: &'static str,
    pub a_id: String,
    pub b_id: String,
    pub a_key: String,
    pub b_key: String,
    /// `north`, `south`, `east` or `west` when the instruction named a direction.
    pub direction: Option<&'static str>,
    pub transcript: String,
    pub summary_ar: String,
    pub summary_en: String,
    /// Always true: the proposal changes nothing until the user confirms it.
    pub unconfirmed: bool,
}

fn bad(ar: &str, en: &str) -> String {
    format!("{ar} | {en}")
}

/// Lower-cases, folds Arabic letter variants, drops vowel marks and tatweel, and turns Arabic-Indic digits into ASCII.
fn normalise(text: &str) -> String {
    text.chars()
        .filter(|c| !matches!(*c, '\u{064B}'..='\u{0652}' | '\u{0670}' | '\u{0640}'))
        .map(|c| match c {
            'أ' | 'إ' | 'آ' | 'ٱ' => 'ا',
            'ة' => 'ه',
            'ى' => 'ي',
            '٠'..='٩' => char::from(b'0' + (c as u32 - '٠' as u32) as u8),
            c if c.is_alphanumeric() => c.to_lowercase().next().unwrap_or(c),
            _ => ' ',
        })
        .collect()
}

/// Function words, folded the same way as the transcript. A word matches a token when the token is the word or the word
/// with the Arabic article / one-letter prefix (ال, لل, و, ب, ل, ف) in front.
const FUNC_WORDS: [(&str, &[&str]); 6] = [
    ("kitchen", &["مطبخ", "kitchen"]),
    ("majlis", &["مجلس", "majlis"]),
    ("living", &["صاله", "living"]),
    ("dining", &["طعام", "dining"]),
    ("bedroom", &["نوم", "bedroom"]),
    ("bathroom", &["حمام", "bathroom", "toilet"]),
];

const MOVE_WORDS: [&str; 12] = ["حرك", "انقل", "ازح", "زح", "اجعل", "ضع", "move", "shift", "put", "push", "relocate", "place"];
const SWAP_WORDS: [&str; 6] = ["بدل", "بادل", "غير", "swap", "switch", "exchange"];
const SIDES: [(Side, &str, &[&str]); 4] = [
    (Side::North, "north", &["شمال", "north"]),
    (Side::South, "south", &["جنوب", "south"]),
    (Side::East, "east", &["شرق", "east"]),
    (Side::West, "west", &["غرب", "west"]),
];
const UNITS: [&str; 12] = ["م", "متر", "امتار", "سم", "سنتي", "مم", "m", "cm", "mm", "meter", "meters", "metres"];

/// The token with a leading article or one-letter preposition removed, or the token itself.
fn stems(token: &str) -> Vec<&str> {
    let mut v = vec![token];
    for p in ["لل", "ال", "وال", "بال", "فال", "و", "ب", "ل", "ف"] {
        if let Some(rest) = token.strip_prefix(p).filter(|r| !r.is_empty()) {
            v.push(rest);
        }
    }
    v
}

fn token_is(token: &str, word: &str) -> bool {
    stems(token).iter().any(|s| *s == word || (word.chars().count() >= 4 && s.chars().count() > word.chars().count() && s.starts_with(word)))
}

fn has_any(tokens: &[&str], words: &[&str]) -> bool {
    tokens.iter().any(|t| words.iter().any(|w| token_is(t, w)))
}

fn side_of(tokens: &[&str]) -> Result<Option<(Side, &'static str)>, String> {
    let found: Vec<(Side, &'static str)> = SIDES.iter().filter(|(_, _, ws)| has_any(tokens, ws)).map(|(s, n, _)| (*s, *n)).collect();
    match found.as_slice() {
        [] => Ok(None),
        [one] => Ok(Some(*one)),
        _ => Err(bad("ذُكرت أكثر من جهة في التعليمة؛ اذكر جهة واحدة.", "More than one side is named; name a single side.")),
    }
}

/// A number followed by a length unit (`5 m`, `50 سم`, `2m`).
fn names_a_distance(tokens: &[&str]) -> bool {
    tokens.iter().enumerate().any(|(i, t)| {
        let digits: String = t.chars().take_while(char::is_ascii_digit).collect();
        if digits.is_empty() {
            return false;
        }
        let rest = &t[digits.len()..];
        (!rest.is_empty() && UNITS.contains(&rest)) || (rest.is_empty() && tokens.get(i + 1).is_some_and(|n| UNITS.contains(n)))
    })
}

fn label(r: &VoiceRoom) -> String {
    if r.name.is_empty() { r.key.clone() } else { r.name.clone() }
}

/// Where `own` (a normalised name) is spoken in `masked`, with or without an article in front: byte position and length.
fn spoken_at(masked: &str, own: &str) -> Option<(usize, usize)> {
    ["", "ال", "لل", "وال", "بال", "و"].iter().find_map(|p| {
        let needle = format!(" {p}{own} ");
        masked.find(&needle).map(|pos| (pos, needle.len() - 1))
    })
}

/// Which rooms the instruction names, in the order they are spoken. A room whose own name or key is spoken wins over a
/// function word; a function word that matches several rooms is an error listing them.
fn named_rooms(text: &str, rooms: &[VoiceRoom]) -> Result<Vec<usize>, String> {
    let mut hits: Vec<(usize, usize)> = Vec::new(); // (position in the text, room index)
    // 1. an exact room name or key, longest first so "bedroom 2" is not read as "bedroom".
    let mut order: Vec<usize> = (0..rooms.len()).collect();
    order.sort_by_key(|&i| std::cmp::Reverse(label(&rooms[i]).len().max(rooms[i].key.len())));
    let mut masked = format!(" {text} ");
    for i in order {
        let owns = [normalise(&rooms[i].name), normalise(&rooms[i].key)].map(|o| o.split_whitespace().collect::<Vec<_>>().join(" "));
        if let Some((pos, len)) = owns.iter().filter(|o| !o.is_empty()).find_map(|o| spoken_at(&masked, o)) {
            hits.push((pos, i));
            // Blank the name out (same byte length) so its words are not read again as a function word.
            masked.replace_range(pos + 1..pos + len, &" ".repeat(len - 1));
        }
    }
    // 2. a function word, in the words left.
    let mut offset = 0usize;
    for tok in masked.clone().split_whitespace() {
        let pos = masked[offset..].find(tok).map_or(offset, |p| p + offset);
        offset = pos + tok.len();
        for (func, words) in FUNC_WORDS {
            if !words.iter().any(|w| token_is(tok, w)) {
                continue;
            }
            let cands: Vec<usize> = (0..rooms.len()).filter(|&i| rooms[i].func == func && !hits.iter().any(|(_, h)| *h == i)).collect();
            match cands.as_slice() {
                [] => {
                    return Err(bad(&format!("لا توجد غرفة من نوع «{tok}» في هذا الدور."), &format!("There is no '{tok}' room on this level.")));
                }
                [one] => hits.push((pos, *one)),
                many => {
                    let names: Vec<String> = many.iter().map(|&i| label(&rooms[i])).collect();
                    return Err(bad(
                        &format!("«{tok}» تنطبق على أكثر من غرفة ({}); سمِّ الغرفة باسمها.", names.join("، ")),
                        &format!("'{tok}' matches more than one room ({}); name the room.", names.join(", ")),
                    ));
                }
            }
        }
    }
    hits.sort();
    Ok(hits.into_iter().map(|(_, i)| i).collect())
}

fn full_side_neighbour(rooms: &[VoiceRoom], i: usize, side: Side) -> Option<usize> {
    let a = rooms[i].rect;
    (0..rooms.len()).filter(|&j| j != i).find(|&j| {
        let b = rooms[j].rect;
        match side {
            Side::North => a.x == b.x && a.w == b.w && a.y + a.h == b.y,
            Side::South => a.x == b.x && a.w == b.w && b.y + b.h == a.y,
            Side::East => a.y == b.y && a.h == b.h && a.x + a.w == b.x,
            Side::West => a.y == b.y && a.h == b.h && b.x + b.w == a.x,
        }
    })
}

fn side_ar(n: &str) -> &'static str {
    match n {
        "north" => "الشمال",
        "south" => "الجنوب",
        "east" => "الشرق",
        _ => "الغرب",
    }
}

/// Reads one spoken instruction against the rooms of the level. See the module notes for what is accepted and refused.
pub fn interpret(transcript: &str, rooms: &[VoiceRoom]) -> Result<Proposal, String> {
    let text = normalise(transcript);
    let tokens: Vec<&str> = text.split_whitespace().collect();
    if tokens.is_empty() {
        return Err(bad("لم يصل نص من الصوت.", "No text came from the speech."));
    }
    if rooms.len() < 2 {
        return Err(bad("لا توجد غرفتان في هذا الدور لتعديلهما.", "This level has fewer than two rooms to edit."));
    }
    if names_a_distance(&tokens) {
        return Err(bad(
            "التعليمة تذكر مسافة، والتعديل الصوتي ينقل الغرفة بمبادلة مكانها مع جارتها دون مسافة. استعمل تحريك الجدار لمسافة محددة.",
            "The instruction names a distance; a voice edit moves a room by trading places with its neighbour, with no distance. Use the partition move for a set distance.",
        ));
    }
    let side = side_of(&tokens)?;
    let swap = has_any(&tokens, &SWAP_WORDS);
    let moving = has_any(&tokens, &MOVE_WORDS);
    if !swap && !moving {
        return Err(bad(
            "لم أفهم التعليمة. الصيغ المدعومة: «حرّك المطبخ إلى الشمال» و«بدّل المطبخ مع المجلس».",
            "The instruction was not understood. Supported forms: 'move the kitchen north' and 'swap the kitchen with the majlis'.",
        ));
    }
    let named = named_rooms(&text, rooms)?;

    let (a, b, direction) = if swap && side.is_none() && named.len() == 2 {
        (named[0], named[1], None)
    } else if let Some((s, n)) = side {
        let [a] = named.as_slice() else {
            return Err(bad(
                "سمِّ غرفة واحدة لتحريكها نحو الجهة.",
                "Name exactly one room to move towards that side.",
            ));
        };
        let Some(b) = full_side_neighbour(rooms, *a, s) else {
            return Err(bad(
                &format!("لا توجد غرفة ملاصقة لـ «{}» من جهة {} بجدار كامل بالطول نفسه، فلا يمكن تحريكها بالمبادلة.", label(&rooms[*a]), side_ar(n)),
                &format!("No room shares a full wall of the same length with '{}' on the {n} side, so it cannot be moved by trading places.", label(&rooms[*a])),
            ));
        };
        (*a, b, Some(n))
    } else {
        return Err(bad(
            "اذكر الجهة لتحريك الغرفة، أو غرفتين لتبديلهما.",
            "Name a side to move a room, or two rooms to swap them.",
        ));
    };
    if a == b {
        return Err(bad("اذكر غرفتين مختلفتين.", "Name two different rooms."));
    }
    let (la, lb) = (label(&rooms[a]), label(&rooms[b]));
    let (summary_ar, summary_en) = match direction {
        Some(n) => (
            format!("نقل «{la}» نحو {} بمبادلة مكانها مع «{lb}»", side_ar(n)),
            format!("Move '{la}' {n} by trading places with '{lb}'"),
        ),
        None => (format!("تبديل «{la}» مع «{lb}»"), format!("Swap '{la}' with '{lb}'")),
    };
    Ok(Proposal {
        command: "swap_rooms",
        a_id: rooms[a].id.clone(),
        b_id: rooms[b].id.clone(),
        a_key: rooms[a].key.clone(),
        b_key: rooms[b].key.clone(),
        direction,
        transcript: transcript.trim().to_string(),
        summary_ar,
        summary_en,
        unconfirmed: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn room(id: &str, key: &str, func: &str, name: &str, (x, y, w, h): (i64, i64, i64, i64)) -> VoiceRoom {
        VoiceRoom { id: id.into(), key: key.into(), func: func.into(), name: name.into(), rect: Rect { x, y, w, h } }
    }

    /// kitchen (south) under majlis (north); two bedrooms side by side to the east.
    fn plan() -> Vec<VoiceRoom> {
        vec![
            room("k", "kitchen", "kitchen", "المطبخ", (0, 0, 4000, 3000)),
            room("m", "majlis", "majlis", "المجلس", (0, 3000, 4000, 5000)),
            room("b1", "bed1", "bedroom", "غرفة النوم 1", (4000, 0, 3000, 4000)),
            room("b2", "bed2", "bedroom", "غرفة النوم 2", (4000, 4000, 3000, 4000)),
        ]
    }

    #[test]
    fn moving_a_room_north_trades_places_with_the_room_on_that_side() {
        let p = interpret("حرّك المطبخ إلى الشمال", &plan()).unwrap();
        assert_eq!((p.a_id.as_str(), p.b_id.as_str(), p.direction), ("k", "m", Some("north")));
        assert!(p.unconfirmed && p.command == "swap_rooms");
        let e = interpret("Move the kitchen north", &plan()).unwrap();
        assert_eq!((e.a_id.as_str(), e.b_id.as_str()), ("k", "m"));
        let s = interpret("انقل المجلس للجنوب", &plan()).unwrap();
        assert_eq!((s.a_id.as_str(), s.b_id.as_str(), s.direction), ("m", "k", Some("south")));
    }

    #[test]
    fn a_room_with_no_full_wall_neighbour_on_that_side_is_refused_with_the_reason() {
        let e = interpret("حرّك المطبخ إلى الغرب", &plan()).unwrap_err();
        assert!(e.contains("لا توجد غرفة ملاصقة") && e.contains(" | ") && e.contains("trading places"), "{e}");
        // the kitchen's east neighbour has a different height, so the full-side rule fails too.
        assert!(interpret("move the kitchen east", &plan()).unwrap_err().contains("No room shares"));
    }

    #[test]
    fn swapping_two_named_rooms_works_and_a_shared_function_word_is_ambiguous() {
        let p = interpret("بدّل المطبخ مع المجلس", &plan()).unwrap();
        assert_eq!((p.a_id.as_str(), p.b_id.as_str(), p.direction), ("k", "m", None));
        let p = interpret("swap the majlis with the kitchen", &plan()).unwrap();
        assert_eq!((p.a_id.as_str(), p.b_id.as_str()), ("m", "k"));
        let e = interpret("حرّك غرفة النوم إلى الشمال", &plan()).unwrap_err();
        assert!(e.contains("أكثر من غرفة") && e.contains("غرفة النوم 1") && e.contains("غرفة النوم 2"), "{e}");
        // naming the room by its own name resolves it: bed1 sits south of bed2.
        let p = interpret("حرّك غرفة النوم 1 إلى الشمال", &plan()).unwrap();
        assert_eq!((p.a_id.as_str(), p.b_id.as_str()), ("b1", "b2"));
    }

    #[test]
    fn a_distance_an_unknown_phrase_or_missing_pieces_are_refused() {
        assert!(interpret("حرّك المطبخ 5 متر إلى الشمال", &plan()).unwrap_err().contains("مسافة"));
        assert!(interpret("move the kitchen 50cm north", &plan()).unwrap_err().contains("distance"));
        assert!(interpret("حرّك المطبخ ٢ م شمالا", &plan()).unwrap_err().contains("distance"));
        assert!(interpret("ما هو الطقس اليوم", &plan()).unwrap_err().contains("لم أفهم"));
        assert!(interpret("   ", &plan()).unwrap_err().contains("No text"));
        assert!(interpret("حرّك المطبخ", &plan()).unwrap_err().contains("اذكر الجهة"));
        assert!(interpret("حرّك المطبخ إلى الشمال والجنوب", &plan()).unwrap_err().contains("أكثر من جهة"));
        assert!(interpret("حرّك المجلس والمطبخ إلى الشمال", &plan()).unwrap_err().contains("غرفة واحدة"));
        assert!(interpret("حرّك الحمام إلى الشمال", &plan()).unwrap_err().contains("لا توجد غرفة من نوع"));
        assert!(interpret("حرّك المطبخ إلى الشمال", &plan()[..1]).unwrap_err().contains("fewer than two"));
    }
}
