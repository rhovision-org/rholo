//! **Choosing an archive** (Law 111's consumer half).
//!
//! A consumer with several configured archives tries them in order and falls through on a refusal:
//! `402` (no credit), `429` (rate), or an unreachable host is "try the next one", never "the history
//! does not exist". A **failed possession challenge** is different: the archive is excluded for a
//! cooldown and not offered again until it ends. The exclusion is local policy; the archive's fee
//! settlement is between the consumer and the archive.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

/// How long a failed possession challenge keeps an archive out of selection.
pub const ARCHIVE_COOLDOWN: Duration = Duration::from_secs(3_600);

/// What happened when a consumer used an archive.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ArchiveOutcome {
    Served,
    /// The archive refused the request with this HTTP status (`402`, `429`, `5xx`, …).
    Refused(u16),
    /// The host did not answer.
    Unreachable,
    /// The possession challenge found a missing or mismatched body.
    ChallengeFailed,
    ChallengePassed,
}

/// One configured archive.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Entry {
    url: String,
    excluded_until: Option<Instant>,
    last_failure: Option<String>,
}

/// The consumer's archive set and the exclusions it has learned.
#[derive(Clone, Debug, Default)]
pub struct ArchiveSelector {
    entries: Vec<Entry>,
    /// The last outcome per archive, for the operator surface.
    outcomes: BTreeMap<String, ArchiveOutcome>,
}

impl ArchiveSelector {
    pub fn new(urls: impl IntoIterator<Item = String>) -> Self {
        ArchiveSelector {
            entries: urls
                .into_iter()
                .map(|url| Entry {
                    url,
                    excluded_until: None,
                    last_failure: None,
                })
                .collect(),
            outcomes: BTreeMap::new(),
        }
    }

    /// The archives to try, in configuration order, excluding those inside a cooldown.
    pub fn available(&self, now: Instant) -> Vec<&str> {
        self.entries
            .iter()
            .filter(|entry| !Self::excluded(entry, now))
            .map(|entry| entry.url.as_str())
            .collect()
    }

    /// The first archive to try, or `None` when every one is excluded or the set is empty.
    pub fn selected(&self, now: Instant) -> Option<&str> {
        self.entries
            .iter()
            .find(|entry| !Self::excluded(entry, now))
            .map(|entry| entry.url.as_str())
    }

    /// Record a use. A failed challenge excludes; a pass clears an earlier exclusion; a refusal or
    /// an unreachable host records the reason and leaves the archive available (the consumer falls
    /// through in this attempt, and may try it again later).
    pub fn record(&mut self, url: &str, outcome: ArchiveOutcome, now: Instant) {
        let Some(entry) = self.entries.iter_mut().find(|entry| entry.url == url) else {
            return;
        };
        match &outcome {
            ArchiveOutcome::ChallengeFailed => {
                entry.excluded_until = Some(now + ARCHIVE_COOLDOWN);
                entry.last_failure = Some("the possession challenge failed".to_string());
            }
            ArchiveOutcome::ChallengePassed => {
                entry.excluded_until = None;
                entry.last_failure = None;
            }
            ArchiveOutcome::Refused(status) => {
                entry.last_failure = Some(format!("the archive refused the request with {status}"));
            }
            ArchiveOutcome::Unreachable => {
                entry.last_failure = Some("the archive did not answer".to_string());
            }
            ArchiveOutcome::Served => {
                entry.last_failure = None;
            }
        }
        self.outcomes.insert(url.to_string(), outcome);
    }

    /// The last outcome per archive, for an operator or a status page.
    pub fn outcomes(&self) -> &BTreeMap<String, ArchiveOutcome> {
        &self.outcomes
    }

    fn excluded(entry: &Entry, now: Instant) -> bool {
        entry.excluded_until.is_some_and(|until| now < until)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn selector() -> ArchiveSelector {
        ArchiveSelector::new(["http://a:40403".to_string(), "http://b:40403".to_string()])
    }

    /// A failed possession challenge removes the archive for the cooldown, and the next candidate
    /// takes its place.
    #[test]
    fn a_failed_challenge_removes_an_archive_from_selection() {
        let mut selector = selector();
        let now = Instant::now();
        assert_eq!(selector.selected(now), Some("http://a:40403"));
        selector.record("http://a:40403", ArchiveOutcome::ChallengeFailed, now);
        assert_eq!(selector.selected(now), Some("http://b:40403"));
        assert_eq!(
            selector.available(now),
            vec!["http://b:40403"],
            "the failed archive is not offered"
        );
    }

    /// The exclusion ends with the cooldown; a passed challenge also clears it.
    #[test]
    fn an_archive_is_selected_again_after_the_cooldown() {
        let mut selector = selector();
        let now = Instant::now();
        selector.record("http://a:40403", ArchiveOutcome::ChallengeFailed, now);
        assert_eq!(
            selector.selected(now + ARCHIVE_COOLDOWN),
            Some("http://a:40403"),
            "the cooldown has elapsed"
        );

        selector.record("http://a:40403", ArchiveOutcome::ChallengeFailed, now);
        selector.record("http://a:40403", ArchiveOutcome::ChallengePassed, now);
        assert_eq!(
            selector.selected(now),
            Some("http://a:40403"),
            "a pass clears an earlier exclusion"
        );
    }

    /// A refusal or an unreachable host is a fall-through, not an exclusion: the consumer tries the
    /// next archive now and may retry the failed one later.
    #[test]
    fn a_consumer_falls_through_on_a_refusal_and_retries_later() {
        let mut selector = selector();
        let now = Instant::now();
        selector.record("http://a:40403", ArchiveOutcome::Refused(402), now);
        assert_eq!(selector.selected(now), Some("http://a:40403"));
        selector.record("http://b:40403", ArchiveOutcome::Unreachable, now);
        assert_eq!(
            selector.available(now + Duration::from_secs(1)).len(),
            2,
            "neither refusal excludes"
        );
        assert_eq!(
            selector.outcomes().get("http://b:40403"),
            Some(&ArchiveOutcome::Unreachable)
        );
    }
}
