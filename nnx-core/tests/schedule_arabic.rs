//! The schedule on an Arabic screen: every activity and phase has its Arabic name, and the zero-quantity notice names the activity in it.

use nnx_core::msgs::localize;
use nnx_core::schedule::{name_ar, phase_ar, template_names, PHASES};

fn arabic(s: &str) -> bool {
    s.chars().any(|c| ('\u{0600}'..='\u{06FF}').contains(&c))
}

#[test]
fn every_activity_and_phase_has_an_arabic_name() {
    for (key, name, _) in template_names() {
        assert!(name_ar(key).is_some_and(arabic), "{key} ({name})");
    }
    for p in PHASES {
        assert!(phase_ar(p).is_some_and(arabic), "{p}");
    }
    assert_eq!((name_ar("nothing"), phase_ar("nothing")), (None, None));
}

#[test]
fn the_schedule_notices_name_the_activity_in_arabic() {
    let (text, known) = localize("'Footing formwork' has no quantity in the take-off: scheduled with zero duration");
    assert!(known && text.contains("شدات القواعد") && !text.contains("Footing"), "{text}");
    let (text, known) = localize("zone 2 has no elements for 'Column formwork'");
    assert!(known && text.contains("شدات الأعمدة") && text.contains('2'), "{text}");
}
