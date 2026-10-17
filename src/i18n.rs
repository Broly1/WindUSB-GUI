// ───────────────────────── translations ─────────────────────────
// GUI-only. The root helper always speaks English on its stdout protocol;
// the GUI translates what it shows. Catalogs are gettext .po files from po/,
// embedded at build time by build.rs. See po/README.md for contributors.
//
// msgids are the exact English strings. `{}` marks a value filled in at
// runtime (a path, a number, ...), so helper messages like
// "Writing install image: 120 / 4500 MB" are matched against
// "Writing install image: {} / {} MB".

use std::collections::HashMap;
use std::env;
use std::sync::OnceLock;

include!(concat!(env!("OUT_DIR"), "/catalogs.rs"));

struct Catalog {
    exact: HashMap<String, String>,
    // (msgid split on "{}", msgstr), longest msgid first so the most specific wins.
    templates: Vec<(Vec<String>, String)>,
}

static CATALOG: OnceLock<Option<Catalog>> = OnceLock::new();

/// Parse a .po file into (msgid, msgstr) pairs. Skips the header, fuzzy and
/// untranslated entries. Plurals and msgctxt are not used by this app.
pub fn parse_po(text: &str) -> Vec<(String, String)> {
    #[derive(PartialEq)]
    enum Field {
        None,
        Id,
        Str,
        Other,
    }
    let mut out = Vec::new();
    let (mut id, mut st) = (String::new(), String::new());
    let mut field = Field::None;
    let mut fuzzy = false;
    let mut have_entry = false;

    let mut flush = |id: &mut String, st: &mut String, fuzzy: &mut bool, have: &mut bool| {
        if *have && !*fuzzy && !id.is_empty() && !st.is_empty() {
            out.push((id.clone(), st.clone()));
        }
        id.clear();
        st.clear();
        *fuzzy = false;
        *have = false;
    };

    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with('#') {
            // A comment after msgstr starts the next entry.
            if field == Field::Str || field == Field::Other {
                flush(&mut id, &mut st, &mut fuzzy, &mut have_entry);
                field = Field::None;
            }
            if line.starts_with("#,") && line.contains("fuzzy") {
                fuzzy = true;
            }
            continue;
        }
        if let Some(rest) = line.strip_prefix("msgid ") {
            if field == Field::Str || field == Field::Other {
                flush(&mut id, &mut st, &mut fuzzy, &mut have_entry);
            }
            have_entry = true;
            field = Field::Id;
            id.push_str(&unquote(rest));
        } else if let Some(rest) = line.strip_prefix("msgstr ") {
            field = Field::Str;
            st.push_str(&unquote(rest));
        } else if line.starts_with("msgctxt")
            || line.starts_with("msgid_plural")
            || line.starts_with("msgstr[")
        {
            field = Field::Other;
        } else if line.starts_with('"') {
            match field {
                Field::Id => id.push_str(&unquote(line)),
                Field::Str => st.push_str(&unquote(line)),
                _ => {}
            }
        }
    }
    flush(&mut id, &mut st, &mut fuzzy, &mut have_entry);
    out
}

fn unquote(s: &str) -> String {
    let s = s.trim();
    let s = s.strip_prefix('"').unwrap_or(s);
    let s = s.strip_suffix('"').unwrap_or(s);
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('"') => out.push('"'),
            Some('\\') => out.push('\\'),
            Some(o) => {
                out.push('\\');
                out.push(o);
            }
            None => out.push('\\'),
        }
    }
    out
}

/// Languages the user asked for, most preferred first, e.g.
/// LANG=pt_BR.UTF-8 -> ["pt_BR", "pt"]. LANGUAGE may list several (pt_BR:es).
fn wanted_languages() -> Vec<String> {
    let mut raw: Vec<String> = Vec::new();
    if let Ok(l) = env::var("LANGUAGE") {
        raw.extend(l.split(':').map(|s| s.to_string()));
    }
    for var in ["LC_ALL", "LC_MESSAGES", "LANG"] {
        if let Ok(v) = env::var(var) {
            if !v.is_empty() {
                raw.push(v);
                break;
            }
        }
    }
    let mut out = Vec::new();
    for r in raw {
        // strip .UTF-8 and @modifier
        let base = r.split(['.', '@']).next().unwrap_or("").to_string();
        if base.is_empty() || base == "C" || base == "POSIX" {
            continue;
        }
        let short = base.split('_').next().unwrap_or("").to_string();
        for l in [base, short] {
            if !l.is_empty() && !out.contains(&l) {
                out.push(l);
            }
        }
    }
    out
}

fn load() -> Option<Catalog> {
    for lang in wanted_languages() {
        if lang == "en" || lang.starts_with("en_") {
            return None;
        }
        if let Some((_, text)) = CATALOGS.iter().find(|(l, _)| *l == lang) {
            crate::log("i18n", &format!("using translation {}", lang));
            return Some(build_catalog(text));
        }
    }
    None
}

fn build_catalog(text: &str) -> Catalog {
    let mut exact = HashMap::new();
    let mut templates = Vec::new();
    for (id, st) in parse_po(text) {
        let holes = id.matches("{}").count();
        // A translation with a different number of {} would show wrong data:
        // ignore it and fall back to English for that string.
        if st.matches("{}").count() != holes {
            crate::log("i18n", &format!("placeholder mismatch, ignored: {:?}", id));
            continue;
        }
        if holes > 0 && id != "{}" {
            templates.push((id.split("{}").map(|s| s.to_string()).collect(), st.clone()));
        }
        exact.insert(id, st);
    }
    templates.sort_by_key(|(parts, _): &(Vec<String>, String)| {
        std::cmp::Reverse(parts.iter().map(|p| p.len()).sum::<usize>())
    });
    Catalog { exact, templates }
}

fn catalog() -> Option<&'static Catalog> {
    CATALOG.get_or_init(load).as_ref()
}

/// Translate a fixed UI string. Returns the English text if there is no translation.
pub fn tr(s: &str) -> String {
    catalog()
        .and_then(|c| c.exact.get(s))
        .cloned()
        .unwrap_or_else(|| s.to_string())
}

/// Translate a string with `{}` placeholders and fill them in order.
pub fn trf(s: &str, args: &[&str]) -> String {
    fill(&tr(s), args)
}

fn fill(tpl: &str, args: &[&str]) -> String {
    let mut out = String::new();
    let mut parts = tpl.split("{}");
    out.push_str(parts.next().unwrap_or(""));
    for (i, p) in parts.enumerate() {
        out.push_str(args.get(i).copied().unwrap_or(""));
        out.push_str(p);
    }
    out
}

/// Translate a message that was already formatted (e.g. received from the
/// helper). Values captured by `{}` are translated too, so nested messages
/// like "Failed to mount USB drive: mount failed: ..." work.
pub fn tr_msg(s: &str) -> String {
    match catalog() {
        Some(c) => translate(c, s),
        None => s.to_string(),
    }
}

fn translate(c: &Catalog, s: &str) -> String {
    if let Some(t) = c.exact.get(s) {
        return t.clone();
    }
    for (parts, st) in &c.templates {
        if let Some(caps) = match_template(parts, s) {
            let caps: Vec<String> = caps.iter().map(|v| translate(c, v)).collect();
            let refs: Vec<&str> = caps.iter().map(|v| v.as_str()).collect();
            return fill(st, &refs);
        }
    }
    s.to_string()
}

/// Match "lit0{}lit1{}lit2" against s; returns the text found at each {}.
fn match_template<'a>(parts: &[String], s: &'a str) -> Option<Vec<&'a str>> {
    let mut rest = s.strip_prefix(parts[0].as_str())?;
    let mut caps = Vec::new();
    let last = parts.len() - 1;
    for (i, lit) in parts.iter().enumerate().skip(1) {
        if i == last {
            caps.push(rest.strip_suffix(lit.as_str())?);
            return Some(caps);
        }
        if lit.is_empty() {
            return None; // "{}{}" is ambiguous
        }
        let idx = rest.find(lit.as_str())?;
        caps.push(&rest[..idx]);
        rest = &rest[idx + lit.len()..];
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn pot_ids() -> HashSet<String> {
        let pot = include_str!("../po/windusb.pot");
        // The template has empty msgstrs, so copy the id in to reuse the parser.
        let filled = pot.replace("msgstr \"\"", "msgstr \"x\"");
        parse_po(&filled).into_iter().map(|(id, _)| id).collect()
    }

    #[test]
    fn every_translation_is_valid() {
        let ids = pot_ids();
        assert!(!ids.is_empty());
        for (lang, text) in CATALOGS {
            for (id, st) in parse_po(text) {
                assert!(ids.contains(&id), "{}.po: msgid not in windusb.pot: {:?}", lang, id);
                assert_eq!(
                    id.matches("{}").count(),
                    st.matches("{}").count(),
                    "{}.po: {{}} count differs for {:?}",
                    lang,
                    id
                );
            }
        }
    }

    #[test]
    fn templates_match_helper_output() {
        let c = build_catalog(
            "msgid \"Writing install image: {} / {} MB\"\nmsgstr \"Gravando: {} / {} MB\"\n\n\
             msgid \"Failed to mount USB drive: {}\"\nmsgstr \"Falha ao montar: {}\"\n\n\
             msgid \"mount failed: {}\"\nmsgstr \"falha: {}\"\n",
        );
        let parts = &c.templates.iter().find(|t| t.1.starts_with("Gravando")).unwrap().0;
        assert_eq!(
            match_template(parts, "Writing install image: 12 / 4500 MB"),
            Some(vec!["12", "4500"])
        );
        assert_eq!(match_template(parts, "Writing something else"), None);
    }

    #[test]
    fn real_catalogs_translate_helper_lines() {
        let po = CATALOGS.iter().find(|(l, _)| *l == "pt_BR").unwrap().1;
        let c = build_catalog(po);
        assert_eq!(
            translate(&c, "Writing install image: 120 / 4500 MB"),
            "Gravando a imagem de instalação: 120 / 4500 MB"
        );
        assert_eq!(
            translate(&c, "Failed to mount USB drive: mount failed: No such device (os error 19)"),
            "Falha ao montar o pendrive: falha na montagem: No such device (os error 19)"
        );
        assert_eq!(translate(&c, "Formatting drive /dev/sdb..."), "Formatando o dispositivo /dev/sdb...");
        assert_eq!(translate(&c, "something unknown"), "something unknown");
        let es = build_catalog(CATALOGS.iter().find(|(l, _)| *l == "es").unwrap().1);
        assert_eq!(translate(&es, "Flushing cache: 12.5 MB left"), "Vaciando la caché: quedan 12.5 MB");
    }

    #[test]
    fn parses_multiline_and_skips_fuzzy() {
        let po = "msgid \"\"\nmsgstr \"Content-Type: text/plain\\n\"\n\n\
                  msgid \"A\"\nmsgstr \"\"\n\"B\\\"C\"\n\n\
                  #, fuzzy\nmsgid \"D\"\nmsgstr \"E\"\n\n\
                  msgid \"F\"\nmsgstr \"\"\n";
        assert_eq!(parse_po(po), vec![("A".to_string(), "B\"C".to_string())]);
    }
}
