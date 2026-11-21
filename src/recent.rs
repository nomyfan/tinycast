//! When each launcher item (an app, a command) was chosen lately, kept on
//! disk: a search ranks the recent and frequent ones higher, and the empty
//! launcher lists them first.

use std::{
    collections::HashMap,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use nsh_base::CommandItem;
use nsh_core::prelude::*;

/// Each item's id and when it was chosen lately, in seconds since the
/// epoch, oldest first.
pub(crate) type Recent = HashMap<String, Vec<u64>>;

const HOUR: u64 = 60 * 60;
const DAY: u64 = 24 * HOUR;
/// Choices older than this count for nothing, and are forgotten.
const KEPT: u64 = 30 * DAY;
/// The choices of an item that count (and are kept): more add nothing.
const COUNTED: usize = 5;
/// What each choice counted adds to the bonus.
const PER_CHOICE: i32 = 2;
/// How many items the empty launcher lists under "Recent".
const SHOWN: usize = 5;

fn recent_file() -> PathBuf {
    crate::clipboard::data_dir().join("recent.json")
}

pub(crate) fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// The choices saved last time, none when there are none.
pub(crate) fn load() -> Recent {
    std::fs::read(recent_file())
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

/// Notes that item `id` was chosen now, forgets the old choices and saves.
pub(crate) fn record(recent: Signal<Recent>, id: &str) {
    let now = now();
    recent.update(|r| {
        r.entry(id.to_owned()).or_default().push(now);
        r.retain(|_, times| {
            times.retain(|&at| now.saturating_sub(at) < KEPT);
            let extra = times.len().saturating_sub(COUNTED);
            times.drain(..extra);
            !times.is_empty()
        });
    });
    let recent = recent.get_untracked();
    blocking(move || {
        let result = std::fs::create_dir_all(crate::clipboard::data_dir())
            .and_then(|()| std::fs::write(recent_file(), serde_json::to_vec(&recent)?));
        if let Err(error) = result {
            log::warn!("could not save the recent choices: {error}");
        }
    });
}

/// What a search adds to the score of an item chosen at `times`
/// (`search::score`'s scale, where each character matched is 16): more the
/// more recent its last choice, plus some for each choice in the last 30
/// days (up to `COUNTED`). It lifts an item over a match about as good,
/// not over a much better one.
pub(crate) fn bonus(times: &[u64], now: u64) -> i32 {
    let ages = times
        .iter()
        .map(|&at| now.saturating_sub(at))
        .filter(|&age| age < KEPT);
    let Some(last) = ages.clone().min() else {
        return 0;
    };
    let recency = match last {
        age if age < HOUR => 20,
        age if age < DAY => 15,
        age if age < 7 * DAY => 10,
        _ => 5,
    };
    recency + PER_CHOICE * ages.count().min(COUNTED) as i32
}

/// `items` with the most recent and frequent ones (`bonus`) moved first,
/// under "Recent".
pub(crate) fn recent_first(items: Vec<CommandItem>, recent: &Recent, now: u64) -> Vec<CommandItem> {
    let rank = |item: &CommandItem| {
        let times = recent.get(item.id.as_ref()).map_or(&[][..], Vec::as_slice);
        (bonus(times, now), times.last().copied().unwrap_or(0))
    };
    let mut picked: Vec<_> = items
        .iter()
        .enumerate()
        .map(|(i, item)| (rank(item), i))
        .filter(|((bonus, _), _)| *bonus > 0)
        .collect();
    picked.sort_by_key(|&(bonus, _)| std::cmp::Reverse(bonus));
    picked.truncate(SHOWN);

    let mut slots: Vec<Option<CommandItem>> = items.into_iter().map(Some).collect();
    let first = picked
        .iter()
        .filter_map(|&(_, i)| slots[i].take())
        .map(|item| item.group("Recent"));
    first
        .collect::<Vec<_>>()
        .into_iter()
        .chain(slots.into_iter().flatten())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bonus_fades_with_age_and_grows_with_use() {
        let now = 100 * DAY;
        assert_eq!(bonus(&[], now), 0);
        assert_eq!(bonus(&[now - 60], now), 22);
        assert_eq!(bonus(&[now - 2 * HOUR], now), 17);
        assert_eq!(bonus(&[now - 3 * DAY], now), 12);
        assert_eq!(bonus(&[now - 10 * DAY], now), 7);
        assert_eq!(bonus(&[now - 40 * DAY], now), 0);
        // The choices past 30 days count for nothing; more than five add
        // nothing.
        assert_eq!(bonus(&[now - 40 * DAY, now - 3 * DAY, now - 60], now), 24);
        assert_eq!(bonus(&[now - 60; 7], now), 30);
    }

    #[test]
    fn recent_items_come_first() {
        let now = 100 * DAY;
        let items = vec![
            CommandItem::new("a", "A").group("Applications"),
            CommandItem::new("b", "B").group("Applications"),
            CommandItem::new("c", "C").group("Commands"),
        ];
        let recent = Recent::from([
            ("c".to_owned(), vec![now - 60]),
            (
                "b".to_owned(),
                vec![now - 3 * DAY, now - 2 * DAY, now - DAY - HOUR],
            ),
            ("gone".to_owned(), vec![now]),
        ]);
        let shown = recent_first(items, &recent, now);
        let ids: Vec<_> = shown.iter().map(|i| i.id.as_ref()).collect();
        assert_eq!(ids, ["c", "b", "a"]);
        let groups: Vec<_> = shown.iter().map(|i| i.group.as_deref()).collect();
        assert_eq!(
            groups,
            [Some("Recent"), Some("Recent"), Some("Applications")]
        );
    }
}
