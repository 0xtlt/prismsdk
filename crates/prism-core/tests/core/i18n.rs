//! Locales, plurals, right-to-left, number and date formatting.

use prism_core::i18n::{chrome_text, plural_category};
use prism_core::prelude::*;
use prism_core::{BasicFormatter, Direction, Formatter};

#[test]
fn locale_tags_regions_and_directions() {
    let l = Locale::new("pt_br");
    assert_eq!(
        (l.tag(), l.language(), l.region()),
        ("pt-BR", "pt", Some("BR"))
    );
    assert_eq!(Locale::new("sr-latn-RS").region(), Some("RS"));
    assert_eq!(Locale::new("zh-Hant").region(), None);
    assert_eq!(Locale::new("").tag(), "en-US");
    assert_eq!(Locale::new("de@euro").tag(), "de");
    for rtl in ["ar", "he", "iw", "fa-IR", "ur", "ckb"] {
        assert_eq!(Locale::new(rtl).direction(), Direction::Rtl, "{rtl}");
    }
    for ltr in ["en", "fr", "tr", "zh-Hant-TW"] {
        assert_eq!(Locale::new(ltr).direction(), Direction::Ltr, "{ltr}");
    }
}

#[test]
fn accept_language_picks_the_highest_quality() {
    let tag = |h: &str| Locale::from_accept_language(h).map(|l| l.tag().to_string());
    assert_eq!(tag("de-DE,de;q=0.9,en;q=0.8").as_deref(), Some("de-DE"));
    assert_eq!(tag("en;q=0.2, ja;q=0.7").as_deref(), Some("ja"));
    // Ties keep the first; `*` and empty entries are skipped.
    assert_eq!(tag("*, fr, es").as_deref(), Some("fr"));
    assert_eq!(tag("*").as_deref(), None);
    assert_eq!(tag("").as_deref(), None);
    // A malformed quality counts as 1.
    assert_eq!(tag("it;q=x, en;q=0.5").as_deref(), Some("it"));
}

#[test]
fn plural_categories_for_each_rule_family() {
    let cats = |lang: &str, ns: &[i64]| -> Vec<&'static str> {
        ns.iter().map(|n| plural_category(lang, *n)).collect()
    };
    assert_eq!(cats("en", &[0, 1, 2, -1]), ["other", "one", "other", "one"]);
    assert_eq!(cats("fr", &[0, 1, 2]), ["one", "one", "other"]);
    assert_eq!(
        cats("ru", &[1, 2, 5, 11, 12, 21, 22, 25, 111, 101]),
        [
            "one", "few", "many", "many", "many", "one", "few", "many", "many", "one"
        ]
    );
    assert_eq!(
        cats("pl", &[1, 2, 5, 12, 21, 22]),
        ["one", "few", "many", "many", "many", "few"]
    );
    assert_eq!(cats("cs", &[1, 3, 5]), ["one", "few", "other"]);
    assert_eq!(
        cats("ar", &[0, 1, 2, 3, 10, 11, 99, 100, 102, 103, 111]),
        [
            "zero", "one", "two", "few", "few", "many", "many", "other", "other", "few", "many"
        ]
    );
    assert_eq!(cats("he", &[1, 2, 3]), ["one", "two", "other"]);
    assert_eq!(cats("ja", &[0, 1, 2]), ["other", "other", "other"]);
}

#[test]
fn numbers_group_and_round_per_locale() {
    let f = BasicFormatter::default();
    let n = |tag: &str, v: f64, d: usize| f.number(&Locale::new(tag), v, d);
    assert_eq!(n("en", 0., 0), "0");
    assert_eq!(n("en", 999., 0), "999");
    assert_eq!(n("en", 1000., 0), "1,000");
    assert_eq!(n("en", 1234567., 0), "1,234,567");
    assert_eq!(n("en", 2.675, 1), "2.7");
    assert_eq!(n("en", -1234.5, 2), "-1,234.50");
    // Rounds to zero: no "-0".
    assert_eq!(n("en", -0.004, 2), "0.00");
    assert_eq!(n("de", 1234567.25, 2), "1.234.567,25");
    assert_eq!(n("ru", 12345., 0), "12\u{a0}345");
    assert_eq!(n("fr-CA", 0.5, 1), "0,5");
    assert_eq!(n("ja", 1234.5, 1), "1,234.5");
}

#[test]
fn dates_across_years_leap_days_and_offsets() {
    let f = BasicFormatter::default();
    let d = |tag: &str, t: f64, s: DateStyle| f.date(&Locale::new(tag), t, s);
    let leap = 1_709_249_400.; // 2024-02-29 23:30 UTC
    assert_eq!(d("en-US", leap, DateStyle::Long), "February 29, 2024");
    assert_eq!(d("en-GB", leap, DateStyle::Medium), "29 Feb 2024");
    assert_eq!(d("en-GB", leap, DateStyle::Short), "29/02/2024");
    assert_eq!(d("de", leap, DateStyle::Short), "29.02.2024");
    assert_eq!(d("ja", leap, DateStyle::Long), "2024年2月29日");
    assert_eq!(d("ja", leap, DateStyle::Short), "2024/02/29");
    assert_eq!(d("es", leap, DateStyle::Long), "29 de febrero de 2024");
    assert_eq!(d("it", leap, DateStyle::Medium), "29 feb. 2024");
    // Unknown languages: ISO dates.
    assert_eq!(d("sv", leap, DateStyle::Long), "2024-02-29");
    // Before 1970.
    assert_eq!(d("en-US", -60., DateStyle::Short), "12/31/69");
    // 2000 is a leap year (divisible by 400): March 1st follows Feb 29.
    assert_eq!(d("fr", 951_868_800., DateStyle::Long), "1 mars 2000");
    // One hour east of UTC: the next day.
    let paris = BasicFormatter { utc_offset: 3600 };
    assert_eq!(
        paris.date(&Locale::new("fr"), leap, DateStyle::Long),
        "1 mars 2024"
    );
    // Short French month names stay whole: "mai", "juin".
    let june = 1_783_166_400.; // 2026-07-04 12:00 UTC
    assert_eq!(d("fr", june, DateStyle::Long), "4 juillet 2026");
}

#[test]
fn times_in_12_and_24_hours() {
    let f = BasicFormatter::default();
    let t = |tag: &str, s: f64| f.time(&Locale::new(tag), s);
    let midnight = 1_767_225_900.; // 2026-01-01 00:05 UTC
    assert_eq!(t("en-US", midnight), "12:05 AM");
    assert_eq!(t("en-US", 1_783_166_400.), "12:00 PM");
    assert_eq!(t("en-GB", midnight), "00:05");
    assert_eq!(t("de", 1_783_166_400.), "12:00");
    let west = BasicFormatter { utc_offset: -3600 };
    assert_eq!(west.time(&Locale::new("fr"), midnight), "23:05");
}

fn app_with(translations: Translations, locale: &str) -> App {
    let mut app = App::new();
    app.set_translations(translations);
    app.set_locale(Some(Locale::new(locale)));
    app
}

#[test]
fn lookups_fall_back_from_region_to_language_to_default() {
    let t = Translations::new()
        .add("pt-BR", &[("bus", "ônibus")])
        .add("pt", &[("bus", "autocarro"), ("train", "comboio")])
        .add(
            "en",
            &[("bus", "bus"), ("train", "train"), ("boat", "boat")],
        );
    let app = app_with(t.clone(), "pt-BR");
    assert_eq!(
        (app.t("bus"), app.t("train"), app.t("boat")),
        ("ônibus".into(), "comboio".into(), "boat".into())
    );
    let app = app_with(t.clone(), "pt-PT");
    assert_eq!(app.t("bus"), "autocarro");
    // Another fallback language.
    let app = app_with(t.fallback("pt").add("de", &[]), "de");
    assert_eq!(app.t("train"), "comboio");
}

#[test]
fn catalogs_parse_comments_escapes_and_plural_keys() {
    let t = Translations::new().parse(
        "ar",
        "# Arabic\n\n items.zero = لا عناصر \nitems.one = عنصر واحد\nitems.two = عنصران\nitems.other = {count} عنصر\nmulti = a\\nb\nbroken line\n",
    );
    assert_eq!(t.languages(), ["ar"]);
    let app = app_with(t, "ar-EG");
    assert_eq!(app.t_count("items", 0), "لا عناصر");
    assert_eq!(app.t_count("items", 2), "عنصران");
    // No `few` form: `other` is used.
    assert_eq!(app.t_count("items", 5), "5 عنصر");
    assert_eq!(app.t("multi"), "a\nb");
    assert_eq!(app.t("broken line"), "broken line");
    assert_eq!(app.direction(), Direction::Rtl);
}

#[test]
fn placeholders_counts_and_extra_arguments() {
    let t = Translations::new().add(
        "en",
        &[
            ("left.one", "{count} task left for {who}"),
            ("left.other", "{count} tasks left for {who}"),
            ("odd", "{missing} and {unclosed"),
        ],
    );
    let app = app_with(t, "en-US");
    assert_eq!(
        app.t_count_with("left", 1200, &[("who", &"Ada")]),
        "1,200 tasks left for Ada"
    );
    assert_eq!(
        app.t_count_with("left", 1, &[("who", &"Ada")]),
        "1 task left for Ada"
    );
    // Unknown placeholders stay visible; an unclosed brace is kept as is.
    assert_eq!(app.t("odd"), "{missing} and {unclosed");
    // No plural forms at all: the key.
    assert_eq!(app.t_count("nothing", 3), "nothing");
}

#[test]
fn chrome_texts_are_built_in_and_overridable() {
    let fr = Locale::new("fr-BE");
    assert_eq!(chrome_text(None, &fr, "back"), "Retour");
    assert_eq!(chrome_text(None, &Locale::new("sw"), "close"), "Close");
    assert_eq!(
        chrome_text(None, &Locale::new("ja"), "no_route"),
        "{path} には何もありません"
    );
    let t = Translations::new().add("fr", &[("prism.back", "Précédent")]);
    assert_eq!(chrome_text(Some(&t), &fr, "back"), "Précédent");
    assert_eq!(chrome_text(Some(&t), &fr, "close"), "Fermer");
    assert_eq!(chrome_text(None, &fr, "clear"), "Effacer");
    assert_eq!(chrome_text(None, &Locale::new("sw"), "clear"), "Clear");
}

#[test]
fn the_app_formats_with_its_locale_and_follows_changes() {
    let mut app = App::new();
    app.set_locale(Some(Locale::new("de-DE")));
    assert_eq!(app.format_number(1234.5, 1), "1.234,5");
    assert_eq!(app.locale_choice().map(|l| l.tag()), Some("de-DE"));
    app.set_locale(None);
    assert_eq!(app.locale().tag(), "en-US");
    assert_eq!(app.format_number(1234.5, 1), "1,234.5");
}
