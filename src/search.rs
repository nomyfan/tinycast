//! The launcher's fuzzy search: a query matches a text holding its
//! characters in order, ignoring case and spaces ("vsc" matches "Visual
//! Studio Code", "qq music" matches "QQMusic"). Matches rank as fzf's do: a
//! prefix, word starts and runs of adjacent characters first.

use nsh_base::CommandItem;

/// Each character matched.
const MATCH: i32 = 16;
/// A match on the text's first character.
const START: i32 = 12;
/// A match at a word's start: after a space or punctuation, or where the
/// script changes ("QQ音乐").
const BOUNDARY: i32 = 8;
/// A match on a camelCase hump ("QQMusic").
const CAMEL: i32 = 6;
/// A match right after the previous one.
const CONSECUTIVE: i32 = 6;
/// Each character skipped between two matches.
const GAP: i32 = 1;

/// How well `text` matches `query`, `None` when it does not: the best
/// alignment of the query's characters over the text's.
pub(crate) fn score(query: &str, text: &str) -> Option<i32> {
    let query: Vec<char> = query.chars().filter(|c| !c.is_whitespace()).map(fold).collect();
    if query.is_empty() {
        return Some(0);
    }
    let chars: Vec<char> = text.chars().collect();
    if query.len() > chars.len() {
        return None;
    }
    let folded: Vec<char> = chars.iter().copied().map(fold).collect();
    let bonus: Vec<i32> = (0..chars.len()).map(|j| bonus(&chars, j)).collect();

    // `best[j]`: the best score of the query's characters so far with the
    // last of them matched at `j`.
    let mut best: Vec<Option<i32>> = folded
        .iter()
        .zip(&bonus)
        .map(|(&c, &b)| (c == query[0]).then_some(MATCH + b))
        .collect();
    for &q in &query[1..] {
        let mut next = vec![None; chars.len()];
        // The best previous match before `j - 1`, less the gap to `j`.
        let mut gapped: Option<i32> = None;
        for j in 1..chars.len() {
            if folded[j] == q {
                let adjacent = best[j - 1].map(|s| s + CONSECUTIVE);
                next[j] = adjacent.max(gapped).map(|s| s + MATCH + bonus[j]);
            }
            gapped = gapped.max(best[j - 1]).map(|s| s - GAP);
        }
        best = next;
    }
    best.into_iter().flatten().max()
}

fn fold(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

fn bonus(chars: &[char], j: usize) -> i32 {
    let c = chars[j];
    let Some(&prev) = j.checked_sub(1).and_then(|i| chars.get(i)) else {
        return START;
    };
    if !prev.is_alphanumeric() && c.is_alphanumeric() || prev.is_ascii() != c.is_ascii() {
        BOUNDARY
    } else if prev.is_lowercase() && c.is_uppercase() {
        CAMEL
    } else {
        0
    }
}

/// The items `query` matches by their label or a keyword, best first (a
/// tie goes to the shorter text, then to the earlier item), out of their
/// groups: one list ranked across them. `boost` adds to the score of an
/// item by its id (a recent one's).
pub(crate) fn rank(items: Vec<CommandItem>, query: &str, boost: impl Fn(&str) -> i32) -> Vec<CommandItem> {
    let mut ranked: Vec<_> = items
        .into_iter()
        .filter_map(|item| {
            let (score, len) = std::iter::once(&item.label)
                .chain(&item.keywords)
                .filter_map(|text| Some((score(query, text)?, text.chars().count())))
                .max_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)))?;
            Some((score + boost(&item.id), len, item))
        })
        .collect();
    ranked.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    ranked
        .into_iter()
        .map(|(_, _, mut item)| {
            item.group = None;
            item
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spaces_are_ignored() {
        assert!(score("qq music", "QQMusic").is_some());
        assert!(score("qqmusic", "QQ Music").is_some());
    }

    #[test]
    fn characters_match_in_order() {
        assert!(score("vsc", "Visual Studio Code").is_some());
        assert!(score("音乐", "QQ音乐").is_some());
        assert!(score("csv", "Visual Studio Code").is_none());
        assert!(score("safarix", "Safari").is_none());
    }

    #[test]
    fn prefix_beats_middle() {
        assert!(score("note", "Notes") > score("note", "Sticky Notes"));
    }

    #[test]
    fn word_starts_beat_scattered() {
        assert!(score("ss", "System Settings") > score("ss", "Mission Control"));
        assert!(score("code", "Visual Studio Code") > score("code", "Xcode"));
    }

    #[test]
    fn rank_orders_and_ungroups() {
        let items = vec![
            CommandItem::new("a", "Sticky Notes").group("Applications"),
            CommandItem::new("b", "Safari").group("Applications"),
            CommandItem::new("c", "Notes").group("Applications"),
            CommandItem::new("d", "QQ音乐").keywords(["QQMusic"]).group("Applications"),
        ];
        let ids = |items: Vec<CommandItem>| items.into_iter().map(|i| i.id).collect::<Vec<_>>();
        let none = |_: &str| 0;
        assert_eq!(ids(rank(items.clone(), "note", none)), ["c", "a"]);
        assert_eq!(ids(rank(items.clone(), "qq music", none)), ["d"]);
        assert!(rank(items.clone(), "note", none).iter().all(|i| i.group.is_none()));
        // A recent item passes a match about as good.
        let recent = |id: &str| if id == "a" { 10 } else { 0 };
        assert_eq!(ids(rank(items, "note", recent)), ["a", "c"]);
    }
}
