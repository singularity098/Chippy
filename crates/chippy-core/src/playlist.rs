//! The play queue: tracks in order, which one plays, and what comes next
//! with shuffle and repeat.

use rand::seq::SliceRandom;

use crate::meta::TrackMeta;
use crate::settings::Repeat;
use crate::vfs::TrackRef;

#[derive(Debug, Clone)]
pub struct Item {
    pub track: TrackRef,
    pub meta: Option<TrackMeta>,
}

#[derive(Debug, Default)]
pub struct Playlist {
    pub items: Vec<Item>,
    pub current: Option<usize>,
    /// Shuffled play order (indices into `items`), when shuffling.
    order: Vec<usize>,
    shuffle: bool,
}

impl Playlist {
    pub fn replace(&mut self, tracks: Vec<TrackRef>, current: Option<usize>) {
        self.items = tracks
            .into_iter()
            .map(|track| Item { track, meta: None })
            .collect();
        self.current = current.filter(|&c| c < self.items.len());
        self.reshuffle();
    }

    pub fn append(&mut self, tracks: Vec<TrackRef>) {
        let start = self.items.len();
        self.items
            .extend(tracks.into_iter().map(|track| Item { track, meta: None }));
        if self.shuffle {
            let mut new: Vec<usize> = (start..self.items.len()).collect();
            new.shuffle(&mut rand::thread_rng());
            self.order.extend(new);
        }
    }

    pub fn remove(&mut self, index: usize) {
        if index >= self.items.len() {
            return;
        }
        self.items.remove(index);
        self.current = match self.current {
            Some(c) if c == index => None,
            Some(c) if c > index => Some(c - 1),
            c => c,
        };
        self.order.retain(|&i| i != index);
        for i in &mut self.order {
            if *i > index {
                *i -= 1;
            }
        }
    }

    pub fn clear(&mut self) {
        self.items.clear();
        self.current = None;
        self.order.clear();
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn set_shuffle(&mut self, on: bool) {
        self.shuffle = on;
        self.reshuffle();
    }

    /// A new random order, starting with the current track.
    fn reshuffle(&mut self) {
        self.order.clear();
        if !self.shuffle {
            return;
        }
        let mut rest: Vec<usize> = (0..self.items.len())
            .filter(|&i| Some(i) != self.current)
            .collect();
        rest.shuffle(&mut rand::thread_rng());
        self.order.extend(self.current);
        self.order.extend(rest);
    }

    fn sequence(&self) -> Vec<usize> {
        if self.shuffle {
            self.order.clone()
        } else {
            (0..self.items.len()).collect()
        }
    }

    /// The track after the current one. `auto` is true when the current
    /// track simply ended (repeat-one then plays it again).
    pub fn next(&self, repeat: Repeat, auto: bool) -> Option<usize> {
        if self.items.is_empty() {
            return None;
        }
        let Some(cur) = self.current else {
            return self.sequence().first().copied();
        };
        if auto && repeat == Repeat::One {
            return Some(cur);
        }
        let seq = self.sequence();
        let pos = seq.iter().position(|&i| i == cur).unwrap_or(0);
        match seq.get(pos + 1) {
            Some(&n) => Some(n),
            None if repeat != Repeat::Off => seq.first().copied(),
            None => None,
        }
    }

    pub fn previous(&self, repeat: Repeat) -> Option<usize> {
        if self.items.is_empty() {
            return None;
        }
        let Some(cur) = self.current else {
            return self.sequence().first().copied();
        };
        let seq = self.sequence();
        let pos = seq.iter().position(|&i| i == cur).unwrap_or(0);
        if pos > 0 {
            Some(seq[pos - 1])
        } else if repeat != Repeat::Off {
            seq.last().copied()
        } else {
            Some(cur)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vfs::Location;

    fn tracks(n: usize) -> Vec<TrackRef> {
        (0..n)
            .map(|i| TrackRef::file(Location::Fs(format!("/{i}.mp3").into())))
            .collect()
    }

    #[test]
    fn order_and_repeat() {
        let mut p = Playlist::default();
        p.replace(tracks(3), Some(1));
        assert_eq!(p.next(Repeat::Off, true), Some(2));
        p.current = Some(2);
        assert_eq!(p.next(Repeat::Off, true), None);
        assert_eq!(p.next(Repeat::All, true), Some(0));
        assert_eq!(p.next(Repeat::One, true), Some(2));
        assert_eq!(p.next(Repeat::One, false), Some(0));
        p.current = Some(0);
        assert_eq!(p.previous(Repeat::Off), Some(0));
        assert_eq!(p.previous(Repeat::All), Some(2));
    }

    #[test]
    fn shuffle_visits_everything_once() {
        let mut p = Playlist::default();
        p.replace(tracks(20), Some(5));
        p.set_shuffle(true);
        let mut seen = vec![5];
        while let Some(n) = p.next(Repeat::Off, true) {
            seen.push(n);
            p.current = Some(n);
        }
        seen.sort();
        assert_eq!(seen, (0..20).collect::<Vec<_>>());
    }

    #[test]
    fn removing_keeps_current() {
        let mut p = Playlist::default();
        p.replace(tracks(4), Some(2));
        p.remove(0);
        assert_eq!(p.current, Some(1));
        p.remove(1);
        assert_eq!(p.current, None);
    }
}
