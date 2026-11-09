//! Marcar alguém com "@nome" no chat, e as datas das mensagens.
//!
//! O texto é cifrado (F2-08), então o servidor não sabe quem foi marcado:
//! cada app, depois de abrir a mensagem, procura o próprio nome nela.

use chrono::{Datelike, Local, NaiveDate, TimeZone};

/// Marca todo mundo da conversa.
pub const EVERYONE: &str = "todos";

/// Quantas sugestões aparecem acima do campo.
const MAX_SUGGESTIONS: usize = 6;

/// Os nomes de usuário só têm letras sem acento, números, `_` e `.`.
fn is_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '.'
}

/// O texto marca `name` (ou `@todos`)? Sem diferença de maiúsculas, e só a
/// palavra inteira: "@ana" não marca a "anabela".
pub fn mentions(text: &str, name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    let lower = text.to_lowercase();
    let name = name.to_lowercase();
    lower.match_indices('@').any(|(at, _)| {
        let before_ok = lower[..at]
            .chars()
            .next_back()
            .is_none_or(|c| !is_name_char(c));
        let word: String = lower[at + 1..]
            .chars()
            .take_while(|&c| is_name_char(c))
            .collect();
        // "@ana." no fim da frase ainda é a ana.
        let word = word.trim_end_matches('.');
        before_ok && (word == name || word == EVERYONE)
    })
}

/// O pedaço "@an" que está sendo digitado no fim do campo, sem o "@".
pub fn query(text: &str) -> Option<&str> {
    let at = text.rfind('@')?;
    let before_ok = text[..at]
        .chars()
        .next_back()
        .is_none_or(char::is_whitespace);
    let typed = &text[at + 1..];
    (before_ok && typed.chars().all(is_name_char)).then_some(typed)
}

/// Quem combina com o que foi digitado depois do "@", em ordem: primeiro os
/// que começam assim, depois os que só contêm.
pub fn suggest(candidates: &[String], typed: &str) -> Vec<String> {
    let typed = typed.to_lowercase();
    let mut starts: Vec<String> = Vec::new();
    let mut contains: Vec<String> = Vec::new();
    for name in candidates {
        let lower = name.to_lowercase();
        if starts
            .iter()
            .chain(&contains)
            .any(|n| n.eq_ignore_ascii_case(name))
        {
            continue;
        }
        if lower.starts_with(&typed) {
            starts.push(name.clone());
        } else if lower.contains(&typed) {
            contains.push(name.clone());
        }
    }
    starts.extend(contains);
    starts.truncate(MAX_SUGGESTIONS);
    starts
}

/// Troca o "@an" do fim do campo por "@ana ", pronto para continuar.
pub fn complete(text: &str, name: &str) -> String {
    let start = query(text).map_or(text.len(), |typed| text.len() - typed.len() - 1);
    format!("{}@{name} ", &text[..start])
}

fn local_date(ms: u64) -> Option<NaiveDate> {
    Local
        .timestamp_millis_opt(ms as i64)
        .single()
        .map(|t| t.date_naive())
}

const WEEKDAYS: [&str; 7] = ["seg", "ter", "qua", "qui", "sex", "sáb", "dom"];
const MONTHS: [&str; 12] = [
    "jan", "fev", "mar", "abr", "mai", "jun", "jul", "ago", "set", "out", "nov", "dez",
];

/// O separador de dia no chat: "hoje", "ontem", "seg, 3 de nov" ou, de
/// outro ano, "3 de nov de 2025".
pub fn day_label(ms: u64, now_ms: u64) -> String {
    let (Some(day), Some(today)) = (local_date(ms), local_date(now_ms)) else {
        return String::new();
    };
    day_label_of(day, today)
}

fn day_label_of(day: NaiveDate, today: NaiveDate) -> String {
    let month = MONTHS[day.month0() as usize];
    if day == today {
        "hoje".into()
    } else if today.pred_opt() == Some(day) {
        "ontem".into()
    } else if day.year() == today.year() {
        let weekday = WEEKDAYS[day.weekday().num_days_from_monday() as usize];
        format!("{weekday}, {} de {month}", day.day())
    } else {
        format!("{} de {month} de {}", day.day(), day.year())
    }
}

/// O horário na lista de conversas: "21:06" hoje, "ontem", "03/11" e, de
/// outro ano, "03/11/25".
pub fn list_time(ms: u64, now_ms: u64) -> String {
    let Some(t) = Local.timestamp_millis_opt(ms as i64).single() else {
        return String::new();
    };
    let Some(today) = local_date(now_ms) else {
        return String::new();
    };
    let day = t.date_naive();
    if day == today {
        t.format("%H:%M").to_string()
    } else if today.pred_opt() == Some(day) {
        "ontem".into()
    } else if day.year() == today.year() {
        t.format("%d/%m").to_string()
    } else {
        t.format("%d/%m/%y").to_string()
    }
}

/// Para cada mensagem, o separador de dia que vai em cima dela (vazio se é
/// o mesmo dia da anterior).
pub fn day_separators(times_ms: impl IntoIterator<Item = u64>, now_ms: u64) -> Vec<String> {
    let mut prev = None;
    times_ms
        .into_iter()
        .map(|ms| {
            let day = local_date(ms);
            let label = if day != prev {
                day_label(ms, now_ms)
            } else {
                String::new()
            };
            prev = day;
            label
        })
        .collect()
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_whole_name_only() {
        assert!(mentions("oi @ana, bora?", "ana"));
        assert!(mentions("@Ana", "ana"));
        assert!(mentions("vem @ana.", "ana"));
        assert!(mentions("@todos reunião", "ana"));
        assert!(!mentions("oi @anabela", "ana"));
        assert!(!mentions("mail ze@ana", "ana"));
        assert!(!mentions("ana sem arroba", "ana"));
        assert!(!mentions("@ana", ""));
    }

    #[test]
    fn query_is_the_word_being_typed() {
        assert_eq!(query("oi @an"), Some("an"));
        assert_eq!(query("@"), Some(""));
        assert_eq!(query("oi @ana bora"), None);
        assert_eq!(query("ze@an"), None);
        assert_eq!(query("sem nada"), None);
    }

    #[test]
    fn suggest_prefers_prefix_and_skips_repeats() {
        let names: Vec<String> = ["bia", "ana", "joana", "Ana"].map(String::from).into();
        assert_eq!(suggest(&names, "an"), ["ana", "joana"]);
        assert_eq!(suggest(&names, "").len(), 3);
    }

    #[test]
    fn complete_replaces_the_typed_part() {
        assert_eq!(complete("oi @an", "ana"), "oi @ana ");
        assert_eq!(complete("@", "bia"), "@bia ");
    }

    #[test]
    fn day_labels() {
        let today = NaiveDate::from_ymd_opt(2026, 11, 9).unwrap();
        let d = |y, m, day| NaiveDate::from_ymd_opt(y, m, day).unwrap();
        assert_eq!(day_label_of(today, today), "hoje");
        assert_eq!(day_label_of(d(2026, 11, 8), today), "ontem");
        assert_eq!(day_label_of(d(2026, 11, 3), today), "ter, 3 de nov");
        assert_eq!(day_label_of(d(2025, 12, 25), today), "25 de dez de 2025");
    }

    #[test]
    fn separators_only_when_day_changes() {
        let hour = 3_600_000;
        let base = Local
            .with_ymd_and_hms(2026, 11, 8, 10, 0, 0)
            .unwrap()
            .timestamp_millis() as u64;
        let now = base + 24 * hour;
        let seps = day_separators([base, base + hour, base + 24 * hour], now);
        assert_eq!(seps, ["ontem", "", "hoje"]);
    }
}
