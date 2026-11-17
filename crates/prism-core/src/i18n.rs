//! Internationalization: locale and layout direction, translations with
//! placeholders and plurals, number and date formatting.
//!
//! ```ignore
//! cx.set_translations(
//!     Translations::new()
//!         .parse("en", "greeting = Hello {name}!\ntodos.one = {count} todo\ntodos.other = {count} todos")
//!         .parse("fr", "greeting = Bonjour {name} !\ntodos.one = {count} tâche\ntodos.other = {count} tâches"),
//! );
//! text(cx.t_with("greeting", &[("name", &"Ada")]));
//! text(cx.t_count("todos", 3));             // "3 tâches" in French
//! text(cx.format_number(1234.5, 1));       // "1 234,5"
//! text(cx.format_date(now, DateStyle::Long)); // "14 novembre 2026"
//! ```
//!
//! The locale comes from the system (native) or `Accept-Language` (web) and
//! can be overridden with [`App::set_locale`]. Right-to-left languages flip
//! layouts on every backend.

use std::collections::HashMap;
use std::fmt::Display;

use crate::app::App;

/// A BCP 47 language tag such as `en-US`, `fr-FR` or `ar`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Locale(String);

impl Default for Locale {
    fn default() -> Self {
        Locale("en-US".into())
    }
}

impl Locale {
    /// Normalizes `fr_FR` / `fr-fr` to `fr-FR`.
    pub fn new(tag: &str) -> Self {
        let tag = tag.split(['.', '@']).next().unwrap_or("").replace('_', "-");
        let mut parts = tag.split('-').filter(|p| !p.is_empty());
        let Some(lang) = parts.next() else {
            return Locale::default();
        };
        let mut out = lang.to_ascii_lowercase();
        for p in parts {
            out.push('-');
            match p.len() {
                2 => out.push_str(&p.to_ascii_uppercase()),
                // Script (Hant, Latn…).
                4 => {
                    out.push_str(&p[..1].to_ascii_uppercase());
                    out.push_str(&p[1..].to_ascii_lowercase());
                }
                _ => out.push_str(p),
            }
        }
        Locale(out)
    }

    /// The first language of an HTTP `Accept-Language` header.
    pub fn from_accept_language(header: &str) -> Option<Self> {
        let mut best: Option<(f32, &str)> = None;
        for part in header.split(',') {
            let mut it = part.trim().split(';');
            let tag = it.next()?.trim();
            if tag.is_empty() || tag == "*" {
                continue;
            }
            let q = it
                .find_map(|p| p.trim().strip_prefix("q="))
                .and_then(|q| q.parse().ok())
                .unwrap_or(1.0);
            if best.is_none_or(|(bq, _)| q > bq) {
                best = Some((q, tag));
            }
        }
        best.map(|(_, tag)| Locale::new(tag))
    }

    /// The BCP 47 tag (`fr-FR`).
    pub fn tag(&self) -> &str {
        &self.0
    }

    /// The best of `supported` for a user of this locale (#229): the same
    /// tag, else the same language, else the first one. `None` when the
    /// list is empty.
    pub fn best_match(&self, supported: &[Locale]) -> Option<Locale> {
        supported
            .iter()
            .find(|l| l.tag().eq_ignore_ascii_case(self.tag()))
            .or_else(|| {
                supported
                    .iter()
                    .find(|l| l.language().eq_ignore_ascii_case(self.language()))
            })
            .or_else(|| supported.first())
            .cloned()
    }

    /// `fr` for `fr-FR`.
    pub fn language(&self) -> &str {
        self.0.split('-').next().unwrap_or("en")
    }

    /// `FR` for `fr-FR`.
    pub fn region(&self) -> Option<&str> {
        self.0.split('-').skip(1).find(|p| p.len() == 2)
    }

    /// Right to left for Arabic, Hebrew, Persian, Urdu and other RTL scripts.
    pub fn direction(&self) -> Direction {
        match self.language() {
            "ar" | "he" | "iw" | "fa" | "ur" | "ps" | "yi" | "dv" | "ckb" | "sd" | "ug" => {
                Direction::Rtl
            }
            _ => Direction::Ltr,
        }
    }
}

impl Display for Locale {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Layout direction: right-to-left mirrors rows, paddings and text alignment.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Direction {
    #[default]
    /// Left to right.
    Ltr,
    /// Right to left (Arabic, Hebrew…).
    Rtl,
}

/// Translated strings by language.
///
/// Keys are looked up in the full tag (`pt-BR`), then the language (`pt`),
/// then the fallback language. Missing keys render as the key itself.
#[derive(Clone, Debug)]
pub struct Translations {
    catalogs: HashMap<String, HashMap<String, String>>,
    fallback: String,
}

impl Default for Translations {
    fn default() -> Self {
        Self::new()
    }
}

impl Translations {
    /// No languages yet, falling back to English.
    pub fn new() -> Self {
        Translations {
            catalogs: HashMap::new(),
            fallback: "en".into(),
        }
    }

    /// Language used when a key is missing in the current one (default `en`).
    pub fn fallback(mut self, lang: &str) -> Self {
        self.fallback = Locale::new(lang).tag().to_string();
        self
    }

    /// Adds `key → text` entries for `lang`.
    pub fn add(mut self, lang: &str, entries: &[(&str, &str)]) -> Self {
        let catalog = self
            .catalogs
            .entry(Locale::new(lang).tag().to_string())
            .or_default();
        for (k, v) in entries {
            catalog.insert(k.to_string(), v.to_string());
        }
        self
    }

    /// Adds entries from a simple text catalog: one `key = text` per line,
    /// `#` comments, `\n` for line breaks. Plural forms are keys with a
    /// `.zero`, `.one`, `.two`, `.few`, `.many` or `.other` suffix.
    pub fn parse(mut self, lang: &str, source: &str) -> Self {
        let catalog = self
            .catalogs
            .entry(Locale::new(lang).tag().to_string())
            .or_default();
        for line in source.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some((k, v)) = line.split_once('=') {
                catalog.insert(k.trim().to_string(), v.trim().replace("\\n", "\n"));
            }
        }
        self
    }

    /// Languages with at least one entry.
    pub fn languages(&self) -> Vec<&str> {
        let mut langs: Vec<&str> = self.catalogs.keys().map(String::as_str).collect();
        langs.sort_unstable();
        langs
    }

    /// The text of `key` for `locale`: its exact tag, then its language, then the fallback language.
    pub fn lookup(&self, locale: &Locale, key: &str) -> Option<&str> {
        [locale.tag(), locale.language(), self.fallback.as_str()]
            .into_iter()
            .find_map(|lang| self.catalogs.get(lang)?.get(key))
            .map(String::as_str)
    }
}

/// CLDR plural category of `n` in `lang` (integers).
pub fn plural_category(lang: &str, n: i64) -> &'static str {
    let n = n.unsigned_abs();
    let (m10, m100) = (n % 10, n % 100);
    match lang {
        "ja" | "zh" | "ko" | "th" | "vi" | "id" | "ms" | "tr" => "other",
        "fr" | "pt" | "hi" | "fa" => {
            if n <= 1 {
                "one"
            } else {
                "other"
            }
        }
        "ru" | "uk" | "be" | "sr" | "hr" | "bs" => {
            if m10 == 1 && m100 != 11 {
                "one"
            } else if (2..=4).contains(&m10) && !(12..=14).contains(&m100) {
                "few"
            } else {
                "many"
            }
        }
        "pl" => {
            if n == 1 {
                "one"
            } else if (2..=4).contains(&m10) && !(12..=14).contains(&m100) {
                "few"
            } else {
                "many"
            }
        }
        "cs" | "sk" => match n {
            1 => "one",
            2..=4 => "few",
            _ => "other",
        },
        "ar" => match (n, m100) {
            (0, _) => "zero",
            (1, _) => "one",
            (2, _) => "two",
            (_, 3..=10) => "few",
            (_, 11..=99) => "many",
            _ => "other",
        },
        "he" | "iw" => match n {
            1 => "one",
            2 => "two",
            _ => "other",
        },
        _ => {
            if n == 1 {
                "one"
            } else {
                "other"
            }
        }
    }
}

/// Replaces `{name}` placeholders.
fn interpolate(template: &str, args: &[(&str, &dyn Display)]) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        match after.find('}') {
            Some(end) => {
                let name = &after[..end];
                match args.iter().find(|(k, _)| *k == name) {
                    Some((_, v)) => out.push_str(&v.to_string()),
                    None => {
                        out.push('{');
                        out.push_str(name);
                        out.push('}');
                    }
                }
                rest = &after[end + 1..];
            }
            None => {
                out.push_str(&rest[start..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

/// How much detail a formatted date shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DateStyle {
    /// `11/14/26`, `14/11/2026`
    Short,
    /// `Nov 14, 2026`, `14 nov. 2026`
    Medium,
    /// `November 14, 2026`, `14 novembre 2026`
    Long,
}

/// Formats numbers and dates for a locale. Apple backends use Foundation's
/// formatters; [`BasicFormatter`] is the portable fallback (web, tests).
pub trait Formatter {
    /// `value` with `decimals` digits after the separator, in the locale's style.
    fn number(&self, locale: &Locale, value: f64, decimals: usize) -> String;
    /// `unix` is seconds since 1970 (UTC).
    fn date(&self, locale: &Locale, unix: f64, style: DateStyle) -> String;
    /// The time of day of `unix` (seconds since 1970, UTC).
    fn time(&self, locale: &Locale, unix: f64) -> String;
}

/// Pure-Rust formatting for common locales, in a fixed UTC offset.
#[derive(Clone, Copy, Debug, Default)]
pub struct BasicFormatter {
    /// Seconds east of UTC.
    pub utc_offset: i32,
}

const MONTHS: &[(&str, [&str; 12])] = &[
    (
        "en",
        [
            "January",
            "February",
            "March",
            "April",
            "May",
            "June",
            "July",
            "August",
            "September",
            "October",
            "November",
            "December",
        ],
    ),
    (
        "fr",
        [
            "janvier",
            "février",
            "mars",
            "avril",
            "mai",
            "juin",
            "juillet",
            "août",
            "septembre",
            "octobre",
            "novembre",
            "décembre",
        ],
    ),
    (
        "de",
        [
            "Januar",
            "Februar",
            "März",
            "April",
            "Mai",
            "Juni",
            "Juli",
            "August",
            "September",
            "Oktober",
            "November",
            "Dezember",
        ],
    ),
    (
        "es",
        [
            "enero",
            "febrero",
            "marzo",
            "abril",
            "mayo",
            "junio",
            "julio",
            "agosto",
            "septiembre",
            "octubre",
            "noviembre",
            "diciembre",
        ],
    ),
    (
        "it",
        [
            "gennaio",
            "febbraio",
            "marzo",
            "aprile",
            "maggio",
            "giugno",
            "luglio",
            "agosto",
            "settembre",
            "ottobre",
            "novembre",
            "dicembre",
        ],
    ),
    (
        "pt",
        [
            "janeiro",
            "fevereiro",
            "março",
            "abril",
            "maio",
            "junho",
            "julho",
            "agosto",
            "setembro",
            "outubro",
            "novembro",
            "dezembro",
        ],
    ),
];

/// (year, month 1-12, day) of a day count since 1970-01-01.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

impl BasicFormatter {
    fn local(&self, unix: f64) -> (i64, u32, u32, u32, u32) {
        let secs = unix.floor() as i64 + i64::from(self.utc_offset);
        let (y, m, d) = civil_from_days(secs.div_euclid(86_400));
        let sod = secs.rem_euclid(86_400);
        (y, m, d, (sod / 3600) as u32, (sod % 3600 / 60) as u32)
    }

    fn separators(locale: &Locale) -> (&'static str, &'static str) {
        match locale.language() {
            "fr" => ("\u{202f}", ","),
            "de" | "es" | "it" | "nl" | "pt" | "id" | "tr" | "da" | "el" => (".", ","),
            "ru" | "uk" | "pl" | "cs" | "sk" | "sv" | "fi" | "nb" | "no" | "hu" | "bg" => {
                ("\u{a0}", ",")
            }
            _ => (",", "."),
        }
    }
}

impl Formatter for BasicFormatter {
    fn number(&self, locale: &Locale, value: f64, decimals: usize) -> String {
        let (group, decimal) = Self::separators(locale);
        let raw = format!("{:.*}", decimals, value.abs());
        let (int, frac) = raw.split_once('.').unwrap_or((&raw, ""));
        let mut out = String::new();
        if value < 0. && raw.chars().any(|c| c.is_ascii_digit() && c != '0') {
            out.push('-');
        }
        for (i, c) in int.chars().enumerate() {
            if i > 0 && (int.len() - i).is_multiple_of(3) {
                out.push_str(group);
            }
            out.push(c);
        }
        if !frac.is_empty() {
            out.push_str(decimal);
            out.push_str(frac);
        }
        out
    }

    fn date(&self, locale: &Locale, unix: f64, style: DateStyle) -> String {
        let (y, m, d, _, _) = self.local(unix);
        let lang = locale.language();
        let months = MONTHS.iter().find(|(l, _)| *l == lang).map(|(_, m)| m);
        let us = lang == "en" && matches!(locale.region(), None | Some("US"));
        match (style, months) {
            (DateStyle::Short, _) => match lang {
                "en" if us => format!("{m}/{d}/{:02}", y % 100),
                "en" | "fr" | "es" | "it" | "pt" => format!("{d:02}/{m:02}/{y}"),
                "de" | "ru" | "pl" => format!("{d:02}.{m:02}.{y}"),
                "ja" | "zh" => format!("{y}/{m:02}/{d:02}"),
                _ => format!("{y}-{m:02}-{d:02}"),
            },
            (_, None) => match lang {
                "ja" | "zh" => format!("{y}年{m}月{d}日"),
                _ => format!("{y}-{m:02}-{d:02}"),
            },
            (style, Some(months)) => {
                let full = months[m as usize - 1];
                let name = if style == DateStyle::Long {
                    full.to_string()
                } else {
                    let short: String = full.chars().take(3).collect();
                    match lang {
                        "en" => short,
                        "de" => short,
                        _ if full.chars().count() > 4 => format!("{short}."),
                        _ => full.to_string(),
                    }
                };
                match lang {
                    "en" if us => format!("{name} {d}, {y}"),
                    "en" => format!("{d} {name} {y}"),
                    "de" => format!("{d}. {name} {y}"),
                    "es" | "pt" if style == DateStyle::Long => {
                        format!("{d} de {name} de {y}")
                    }
                    _ => format!("{d} {name} {y}"),
                }
            }
        }
    }

    fn time(&self, locale: &Locale, unix: f64) -> String {
        let (_, _, _, h, min) = self.local(unix);
        if locale.language() == "en" && matches!(locale.region(), None | Some("US")) {
            let h12 = if h % 12 == 0 { 12 } else { h % 12 };
            format!("{h12}:{min:02} {}", if h < 12 { "AM" } else { "PM" })
        } else {
            format!("{h:02}:{min:02}")
        }
    }
}

/// Texts drawn by Prism itself (stack headers, default screens, the clear
/// action of optional date pickers, the question before leaving with
/// unsaved changes), built in for common languages. Apps override them with
/// `prism.back`, `prism.close`, `prism.not_found`, `prism.no_route`,
/// `prism.clear`, `prism.unsaved_title`, `prism.unsaved_message`,
/// `prism.discard` and `prism.cancel` keys.
const CHROME: &[(&str, [&str; 12])] = &[
    (
        "en",
        [
            "Back",
            "Close",
            "Not found",
            "Nothing at {path}",
            "Clear",
            "Leave without saving?",
            "Your changes will be lost.",
            "Discard Changes",
            "Cancel",
            "Reorder",
            "Drag, or use the up and down arrow keys, to move the row.",
            "{n} of {count}",
        ],
    ),
    (
        "fr",
        [
            "Retour",
            "Fermer",
            "Introuvable",
            "Rien à l’adresse {path}",
            "Effacer",
            "Quitter sans enregistrer ?",
            "Vos modifications seront perdues.",
            "Ignorer les modifications",
            "Annuler",
            "Réordonner",
            "Faites glisser, ou utilisez les flèches haut et bas, pour déplacer la ligne.",
            "{n} sur {count}",
        ],
    ),
    (
        "de",
        [
            "Zurück",
            "Schließen",
            "Nicht gefunden",
            "Nichts unter {path}",
            "Löschen",
            "Ohne Sichern verlassen?",
            "Deine Änderungen gehen verloren.",
            "Änderungen verwerfen",
            "Abbrechen",
            "Neu anordnen",
            "Ziehen oder die Pfeiltasten nach oben und unten verwenden, um die Zeile zu verschieben.",
            "{n} von {count}",
        ],
    ),
    (
        "es",
        [
            "Atrás",
            "Cerrar",
            "No encontrado",
            "Nada en {path}",
            "Borrar",
            "¿Salir sin guardar?",
            "Se perderán tus cambios.",
            "Descartar cambios",
            "Cancelar",
            "Reordenar",
            "Arrastra, o usa las flechas arriba y abajo, para mover la fila.",
            "{n} de {count}",
        ],
    ),
    (
        "it",
        [
            "Indietro",
            "Chiudi",
            "Non trovato",
            "Niente in {path}",
            "Cancella",
            "Uscire senza salvare?",
            "Le modifiche andranno perse.",
            "Scarta le modifiche",
            "Annulla",
            "Riordina",
            "Trascina, o usa le frecce su e giù, per spostare la riga.",
            "{n} di {count}",
        ],
    ),
    (
        "pt",
        [
            "Voltar",
            "Fechar",
            "Não encontrado",
            "Nada em {path}",
            "Limpar",
            "Sair sem salvar?",
            "Suas alterações serão perdidas.",
            "Descartar alterações",
            "Cancelar",
            "Reordenar",
            "Arraste, ou use as setas para cima e para baixo, para mover a linha.",
            "{n} de {count}",
        ],
    ),
    (
        "nl",
        [
            "Terug",
            "Sluit",
            "Niet gevonden",
            "Niets op {path}",
            "Wissen",
            "Verlaten zonder opslaan?",
            "Je wijzigingen gaan verloren.",
            "Wijzigingen negeren",
            "Annuleer",
            "Herschikken",
            "Sleep, of gebruik de pijltjestoetsen omhoog en omlaag, om de rij te verplaatsen.",
            "{n} van {count}",
        ],
    ),
    (
        "ar",
        [
            "رجوع",
            "إغلاق",
            "غير موجود",
            "لا شيء في {path}",
            "مسح",
            "المغادرة دون حفظ؟",
            "ستُفقد تغييراتك.",
            "تجاهل التغييرات",
            "إلغاء",
            "إعادة الترتيب",
            "اسحب، أو استخدم مفتاحي السهم لأعلى ولأسفل، لنقل الصف.",
            "{n} من {count}",
        ],
    ),
    (
        "he",
        [
            "חזרה",
            "סגירה",
            "לא נמצא",
            "אין כלום ב־{path}",
            "ניקוי",
            "לצאת בלי לשמור?",
            "השינויים שלך יאבדו.",
            "מחיקת השינויים",
            "ביטול",
            "סידור מחדש",
            "גררו, או השתמשו במקשי החצים למעלה ולמטה, כדי להזיז את השורה.",
            "{n} מתוך {count}",
        ],
    ),
    (
        "ja",
        [
            "戻る",
            "閉じる",
            "見つかりません",
            "{path} には何もありません",
            "消去",
            "保存せずに閉じますか？",
            "変更内容は失われます。",
            "変更を破棄",
            "キャンセル",
            "並べ替え",
            "ドラッグするか、上下の矢印キーで行を移動します。",
            "{count} 件中 {n} 件目",
        ],
    ),
    (
        "zh",
        [
            "返回",
            "关闭",
            "未找到",
            "{path} 没有内容",
            "清除",
            "不保存就离开吗？",
            "你的更改将会丢失。",
            "放弃更改",
            "取消",
            "重新排序",
            "拖动，或使用上下方向键，移动该行。",
            "第 {n} 项，共 {count} 项",
        ],
    ),
    (
        "ko",
        [
            "뒤로",
            "닫기",
            "찾을 수 없음",
            "{path}에 아무것도 없습니다",
            "지우기",
            "저장하지 않고 나가시겠습니까?",
            "변경 사항이 사라집니다.",
            "변경 사항 버리기",
            "취소",
            "순서 변경",
            "드래그하거나 위쪽 및 아래쪽 화살표 키를 사용하여 행을 이동합니다.",
            "{count}개 중 {n}번째",
        ],
    ),
];

/// A built-in Prism text: `back`, `close`, `not_found`, `no_route`,
/// `clear`, `unsaved_title`, `unsaved_message`, `discard`, `cancel`,
/// `reorder`, `reorder_hint` or `reorder_position` (`{n}` of `{count}`).
pub fn chrome_text(translations: Option<&Translations>, locale: &Locale, key: &str) -> String {
    if let Some(t) = translations.and_then(|t| t.lookup(locale, &format!("prism.{key}"))) {
        return t.to_string();
    }
    let index = match key {
        "back" => 0,
        "close" => 1,
        "not_found" => 2,
        "clear" => 4,
        "unsaved_title" => 5,
        "unsaved_message" => 6,
        "discard" => 7,
        "cancel" => 8,
        "reorder" => 9,
        "reorder_hint" => 10,
        "reorder_position" => 11,
        _ => 3,
    };
    CHROME
        .iter()
        .find(|(l, _)| *l == locale.language())
        .unwrap_or(&CHROME[0])
        .1[index]
        .to_string()
}

impl App {
    /// A built-in Prism text (`back`, `close`, `not_found`, `no_route`,
    /// `clear`) in the current language.
    pub fn chrome_text(&self, key: &str) -> String {
        chrome_text(self.try_global::<Translations>(), self.locale(), key)
    }

    /// Installs the app's translations and re-renders everything.
    pub fn set_translations(&mut self, translations: Translations) {
        self.set_global(translations);
    }

    /// The locale strings and formats use: the user's choice
    /// ([`App::set_locale`]) or the system's.
    pub fn locale(&self) -> &Locale {
        self.locale_override
            .as_ref()
            .unwrap_or(&self.environment().locale)
    }

    /// The locale chosen with [`App::set_locale`], if any.
    pub fn locale_choice(&self) -> Option<&Locale> {
        self.locale_override.as_ref()
    }

    /// The app's own locale (#229), applied as the app starts (every
    /// window, every web session): `locale`, else the best match of the
    /// user's among `supported`. `PRISM_LOCALE` wins over both. What
    /// `Application::locale` and `supported_locales` do.
    pub fn apply_app_locale(&mut self, locale: Option<&Locale>, supported: &[Locale]) {
        if std::env::var_os("PRISM_LOCALE").is_some_and(|v| !v.is_empty()) {
            return;
        }
        let chosen = match locale {
            Some(l) => Some(l.clone()),
            None => self.environment().locale.best_match(supported),
        };
        if let Some(l) = chosen {
            self.set_locale(Some(l));
        }
    }

    /// Overrides the system locale (`None` follows the system again).
    pub fn set_locale(&mut self, locale: Option<Locale>) {
        if self.locale_override != locale {
            self.locale_override = locale;
            self.refresh();
        }
    }

    /// Layout direction of the current locale.
    pub fn direction(&self) -> Direction {
        self.locale().direction()
    }

    /// The translation of `key` (the key itself when missing).
    pub fn t(&self, key: &str) -> String {
        self.t_with(key, &[])
    }

    /// The translation of `key` with `{name}` placeholders filled.
    pub fn t_with(&self, key: &str, args: &[(&str, &dyn Display)]) -> String {
        let template = self
            .try_global::<Translations>()
            .and_then(|t| t.lookup(self.locale(), key))
            .unwrap_or(key);
        interpolate(template, args)
    }

    /// The plural form of `key` for `count` (`key.one`, `key.other`…), with
    /// `{count}` formatted for the locale.
    pub fn t_count(&self, key: &str, count: i64) -> String {
        self.t_count_with(key, count, &[])
    }

    /// Like [`t_count`](Self::t_count), with other `{name}` placeholders filled too.
    pub fn t_count_with(&self, key: &str, count: i64, args: &[(&str, &dyn Display)]) -> String {
        let locale = self.locale();
        let category = plural_category(locale.language(), count);
        let translations = self.try_global::<Translations>();
        let template = [
            format!("{key}.{category}"),
            format!("{key}.other"),
            key.to_string(),
        ]
        .iter()
        .find_map(|k| translations.and_then(|t| t.lookup(locale, k)))
        .unwrap_or(key)
        .to_string();
        let formatted = self.format_number(count as f64, 0);
        let mut all: Vec<(&str, &dyn Display)> = vec![("count", &formatted)];
        all.extend_from_slice(args);
        interpolate(&template, &all)
    }

    /// `value` formatted for the current locale, with `decimals` digits.
    pub fn format_number(&self, value: f64, decimals: usize) -> String {
        self.formatter().number(self.locale(), value, decimals)
    }

    /// `unix`: seconds since 1970 (e.g. `SystemTime::now()`).
    pub fn format_date(&self, unix: f64, style: DateStyle) -> String {
        self.formatter().date(self.locale(), unix, style)
    }

    /// The time of day of `unix` (seconds since 1970), for the current locale.
    pub fn format_time(&self, unix: f64) -> String {
        self.formatter().time(self.locale(), unix)
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn best_matches_follow_the_tag_then_the_language() {
        let l = |t: &str| Locale::new(t);
        let supported = [l("fr-FR"), l("en-GB")];
        assert_eq!(l("en-GB").best_match(&supported), Some(l("en-GB")));
        assert_eq!(l("en-US").best_match(&supported), Some(l("en-GB")));
        assert_eq!(l("de-DE").best_match(&supported), Some(l("fr-FR")));
        assert_eq!(l("de-DE").best_match(&[]), None);
    }

    use super::*;

    #[test]
    fn locales_normalize_and_detect_direction() {
        assert_eq!(Locale::new("fr_fr").tag(), "fr-FR");
        assert_eq!(Locale::new("zh-hant-tw").tag(), "zh-Hant-TW");
        assert_eq!(Locale::new("en_US.UTF-8").tag(), "en-US");
        assert_eq!(Locale::new("ar-EG").direction(), Direction::Rtl);
        assert_eq!(Locale::new("fr").direction(), Direction::Ltr);
        let l = Locale::from_accept_language("en;q=0.5, fr-CH, fr;q=0.9").unwrap();
        assert_eq!(l.tag(), "fr-CH");
    }

    #[test]
    fn plurals_follow_cldr() {
        assert_eq!(plural_category("en", 1), "one");
        assert_eq!(plural_category("en", 0), "other");
        assert_eq!(plural_category("fr", 0), "one");
        assert_eq!(plural_category("ru", 22), "few");
        assert_eq!(plural_category("ru", 12), "many");
        assert_eq!(plural_category("ar", 2), "two");
        assert_eq!(plural_category("ja", 1), "other");
    }

    #[test]
    fn basic_formatter() {
        let f = BasicFormatter::default();
        let fr = Locale::new("fr-FR");
        let us = Locale::new("en-US");
        assert_eq!(f.number(&us, 1234567.891, 2), "1,234,567.89");
        assert_eq!(f.number(&fr, 1234.5, 1), "1\u{202f}234,5");
        assert_eq!(f.number(&us, -5., 0), "-5");
        // 2026-11-14 12:20 UTC
        let t = 1_794_658_800.;
        assert_eq!(f.date(&us, t, DateStyle::Short), "11/14/26");
        assert_eq!(f.date(&us, t, DateStyle::Medium), "Nov 14, 2026");
        assert_eq!(f.date(&fr, t, DateStyle::Long), "14 novembre 2026");
        assert_eq!(f.date(&fr, t, DateStyle::Medium), "14 nov. 2026");
        assert_eq!(
            f.date(&Locale::new("de"), t, DateStyle::Long),
            "14. November 2026"
        );
        assert_eq!(f.time(&us, t), "12:20 PM");
        assert_eq!(f.time(&fr, t), "12:20");
    }

    #[test]
    fn translations_with_plurals_and_fallback() {
        let mut app = App::new();
        app.set_translations(
            Translations::new()
                .parse("en", "hi = Hello {name}!\ntodos.one = {count} todo\ntodos.other = {count} todos\nonly_en = English")
                .parse("fr", "hi = Bonjour {name} !\ntodos.one = {count} tâche\ntodos.other = {count} tâches"),
        );
        assert_eq!(app.t_with("hi", &[("name", &"Ada")]), "Hello Ada!");
        assert_eq!(app.t_count("todos", 1), "1 todo");
        assert_eq!(app.t_count("todos", 1500), "1,500 todos");
        app.set_locale(Some(Locale::new("fr-CA")));
        assert_eq!(app.t_with("hi", &[("name", &"Ada")]), "Bonjour Ada !");
        assert_eq!(app.t_count("todos", 0), "0 tâche");
        assert_eq!(app.t_count("todos", 2), "2 tâches");
        assert_eq!(app.t("only_en"), "English");
        assert_eq!(app.t("missing.key"), "missing.key");
    }
}
