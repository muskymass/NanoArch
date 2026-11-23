//! The clarifying conversation: only what is missing is asked, and it stops when the brief is complete.

use nnx_core::briefai;
use nnx_core::clarify::{self, FIELDS};
use nnx_core::project::BuildingType;

fn fields(d: &briefai::BriefDraft) -> Vec<&'static str> {
    clarify::missing(d).iter().map(|q| q.field).collect()
}

#[test]
fn only_the_missing_fields_are_asked_in_order() {
    let d = briefai::from_keywords("فيلا دورين وملحق في الجبيل على أرض 600م طراز نجدي حديث");
    assert_eq!(fields(&d), ["plot_size", "rooms"]);
    let q = clarify::next_question(&d).unwrap();
    assert_eq!(q.field, "plot_size");
    assert!(q.ar.contains("600") && q.en.contains("600"), "{q:?}");
    assert_eq!(fields(&briefai::BriefDraft::default()), FIELDS);
}

#[test]
fn the_conversation_stops_when_the_brief_is_complete() {
    let mut d = briefai::from_keywords("فيلا دورين وملحق في الجبيل على أرض 600م طراز نجدي حديث");
    let mut asked = Vec::new();
    for answer in ["20×30", "ثلاث غرف نوم ومجلس رجال ومطبخ وحمامين"] {
        let q = clarify::next_question(&d).unwrap();
        asked.push(q.field);
        clarify::apply_answer(&mut d, q.field, answer).unwrap();
    }
    assert_eq!(asked, ["plot_size", "rooms"]);
    assert!(clarify::is_complete(&d) && clarify::next_question(&d).is_none() && clarify::missing(&d).is_empty());
    assert_eq!((d.plot_width_m, d.plot_length_m), (Some(20.0), Some(30.0)));
    let beds: Vec<_> = d.rooms.iter().filter(|r| r.func == "bedroom").collect();
    assert_eq!(beds.len(), 3);
    assert!(beds.iter().all(|r| r.floor == 2 && r.assumed), "two floors: bedrooms go upstairs, marked assumed");
    assert!(d.rooms.iter().all(|r| r.area_m2.is_some()));
}

#[test]
fn each_field_reads_its_own_short_answer() {
    let mut d = briefai::BriefDraft::default();
    clarify::apply_answer(&mut d, "building_type", "قصر").unwrap();
    clarify::apply_answer(&mut d, "floors", "٣").unwrap();
    clarify::apply_answer(&mut d, "plot_size", "25 x 40").unwrap();
    clarify::apply_answer(&mut d, "city", "Riyadh").unwrap();
    clarify::apply_answer(&mut d, "style", "كلاسيكي حديث").unwrap();
    clarify::apply_answer(&mut d, "rooms", "4").unwrap();
    assert_eq!((d.building_type, d.floors), (Some(BuildingType::Palace), Some(3)));
    assert_eq!((d.city.as_deref(), d.climate_zone), (Some("Riyadh"), Some(1)));
    assert_eq!((d.plot_area_m2, d.style.as_deref()), (Some(1000.0), Some("neo_classical")));
    assert_eq!(d.rooms.len(), 4);
    assert!(clarify::is_complete(&d));
}

#[test]
fn an_unreadable_answer_changes_nothing_and_names_the_expectation() {
    let mut d = briefai::from_keywords("فيلا في الرياض");
    let before = d.clone();
    for (f, a) in [("floors", "كثير"), ("floors", "40"), ("plot_size", "كبيرة"), ("city", "Atlantis"), ("style", "xyz"), ("rooms", "لا شيء"), ("building_type", "خيمة"), ("floors", "  "), ("nope", "1")] {
        let e = clarify::apply_answer(&mut d, f, a).unwrap_err();
        assert!(e.contains(" | "), "both languages: {e}");
        assert_eq!(d, before, "{f}={a}");
    }
}

#[test]
fn a_size_that_contradicts_the_stated_area_is_refused() {
    let mut d = briefai::from_keywords("فيلا على أرض 600م");
    let e = clarify::apply_answer(&mut d, "plot_size", "10x10").unwrap_err();
    assert!(e.contains("600") && e.contains("100"), "{e}");
    assert_eq!(d.plot_width_m, None);
    clarify::apply_answer(&mut d, "plot_size", "20x30").unwrap();
}
