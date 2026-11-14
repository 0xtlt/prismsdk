//! Translations (English, French, Arabic — the last one is right-to-left).

use prism::prelude::*;

const EN: &str = r#"
home.info = Rust UI · running on {backend}
home.open.one = {count} open todo
home.open.other = {count} open todos
home.synced = Synced {count}×
home.today = Today is {date}
todos.heading = Todos
todos.placeholder = What needs doing?
todos.new = New…
todos.add = Add
todo.done = Done
todo.open = Open
counter.label = Count: {count}
settings.language = Language
settings.system = System
settings.sample = {number} · {date} · {time}
"#;

const FR: &str = r#"
home.info = Interface Rust · sur {backend}
home.open.one = {count} tâche ouverte
home.open.other = {count} tâches ouvertes
home.synced = Synchronisé {count}×
home.today = Nous sommes le {date}
todos.heading = Tâches
todos.placeholder = Que faut-il faire ?
todos.new = Nouvelle…
todos.add = Ajouter
todo.done = Terminée
todo.open = À faire
counter.label = Compteur : {count}
settings.language = Langue
settings.system = Système
settings.sample = {number} · {date} · {time}
"#;

const AR: &str = r#"
home.info = واجهة Rust · تعمل على {backend}
home.open.zero = لا توجد مهام مفتوحة
home.open.one = مهمة واحدة مفتوحة
home.open.two = مهمتان مفتوحتان
home.open.few = {count} مهام مفتوحة
home.open.many = {count} مهمة مفتوحة
home.open.other = {count} مهمة مفتوحة
home.synced = تمت المزامنة {count}×
home.today = اليوم {date}
todos.heading = المهام
todos.placeholder = ما الذي يجب فعله؟
todos.new = جديدة…
todos.add = إضافة
todo.done = منجزة
todo.open = مفتوحة
counter.label = العدد: {count}
settings.language = اللغة
settings.system = النظام
settings.sample = {number} · {date} · {time}
"#;

/// The languages offered in Settings (tag, native name).
pub const LANGUAGES: [(&str, &str); 3] = [("en", "English"), ("fr", "Français"), ("ar", "العربية")];

/// Installs the catalogs and the language saved in Settings.
pub fn install(cx: &mut App) {
    cx.set_translations(
        Translations::new()
            .parse("en", EN)
            .parse("fr", FR)
            .parse("ar", AR),
    );
    if let Some(tag) = cx.storage().get("locale") {
        cx.set_locale(Some(Locale::new(&tag)));
    }
}

/// Switches language (`None`: follow the system) and remembers it.
pub fn choose(cx: &mut App, tag: Option<&str>) {
    match tag {
        Some(tag) => cx.storage().set("locale", tag),
        None => cx.storage().remove("locale"),
    }
    cx.set_locale(tag.map(Locale::new));
}

pub fn now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0., |d| d.as_secs_f64())
}
