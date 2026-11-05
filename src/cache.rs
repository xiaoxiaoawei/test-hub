//! Small, memory-only LRU for completed translations. No selection or credential
//! is written to disk. A cache belongs to one service/model/credential and
//! source/target language pair at a time.

use crate::config::{parse_endpoint, Endpoint, Language, Settings};
use std::collections::VecDeque;
use std::time::{Duration, Instant};

const MAX_ENTRIES: usize = 64;
const MAX_BYTES: usize = 512 * 1024;
const TTL: Duration = Duration::from_secs(10 * 60);

// Intentionally not Debug: this contains a credential and must never enter logs.
struct Namespace {
    endpoint: Endpoint,
    model: String,
    api_key: String,
    source_language: Language,
    target_language: Language,
}

impl Namespace {
    fn matches(&self, endpoint: &Endpoint, settings: &Settings) -> bool {
        self.endpoint == *endpoint
            && self.model == settings.model.trim()
            && self.api_key == settings.api_key
            && self.source_language == settings.source_language
            && self.target_language == settings.target_language
    }
}

struct Entry {
    source: String,
    translated: String,
    inserted_at: Instant,
    bytes: usize,
}

/// Completed translations only, with at most 64 items and 512 KiB of source and
/// result text. Expiry is absolute, not extended by reads; expired data is purged
/// on the next lookup or insertion, without a polling thread.
pub struct TranslationCache {
    namespace: Option<Namespace>,
    entries: VecDeque<Entry>,
    bytes: usize,
}

impl Default for TranslationCache {
    fn default() -> Self {
        Self::new()
    }
}

impl TranslationCache {
    pub fn new() -> Self {
        Self {
            namespace: None,
            entries: VecDeque::new(),
            bytes: 0,
        }
    }

    pub fn get(&mut self, settings: &Settings, text: &str) -> Option<String> {
        self.get_at(settings, text, Instant::now())
    }

    pub fn insert(&mut self, settings: &Settings, text: &str, translated: &str) {
        self.insert_at(settings, text, translated, Instant::now());
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.bytes = 0;
        self.namespace = None;
    }

    fn namespace_matches(&mut self, settings: &Settings) -> bool {
        let Ok(mut endpoint) = parse_endpoint(&settings.base_url) else {
            self.clear();
            return false;
        };
        // Canonicalize equivalent textual IP spellings in addition to the URL
        // parser's host case, default port and base-route normalization.
        if let Ok(address) = endpoint.host.parse::<std::net::IpAddr>() {
            endpoint.host = address.to_string();
        }
        if self
            .namespace
            .as_ref()
            .is_some_and(|namespace| namespace.matches(&endpoint, settings))
        {
            return true;
        }
        // Drop the previous namespace before cloning the next credential. Keys
        // are held once for the cache namespace, never copied into each entry.
        self.clear();
        self.namespace = Some(Namespace {
            endpoint,
            model: settings.model.trim().to_owned(),
            api_key: settings.api_key.clone(),
            source_language: settings.source_language,
            target_language: settings.target_language,
        });
        true
    }

    fn get_at(&mut self, settings: &Settings, text: &str, now: Instant) -> Option<String> {
        if !self.namespace_matches(settings) {
            return None;
        }
        self.purge_expired(now);
        let source = text.trim();
        let index = self
            .entries
            .iter()
            .position(|entry| entry.source == source)?;
        let entry = self.entries.remove(index)?;
        let result = entry.translated.clone();
        self.entries.push_front(entry);
        Some(result)
    }

    fn insert_at(&mut self, settings: &Settings, text: &str, translated: &str, now: Instant) {
        if !self.namespace_matches(settings) {
            return;
        }
        self.purge_expired(now);
        let source = text.trim();
        if source.is_empty() || translated.trim().is_empty() {
            return;
        }
        if let Some(index) = self.entries.iter().position(|entry| entry.source == source) {
            if let Some(previous) = self.entries.remove(index) {
                self.bytes -= previous.bytes;
            }
        }
        let Some(bytes) = source.len().checked_add(translated.len()) else {
            return;
        };
        if bytes > MAX_BYTES {
            return;
        }
        while self.entries.len() >= MAX_ENTRIES || self.bytes + bytes > MAX_BYTES {
            let Some(oldest) = self.entries.pop_back() else {
                break;
            };
            self.bytes -= oldest.bytes;
        }
        self.entries.push_front(Entry {
            source: source.to_owned(),
            translated: translated.to_owned(),
            inserted_at: now,
            bytes,
        });
        self.bytes += bytes;
    }

    fn purge_expired(&mut self, now: Instant) {
        self.entries.retain(|entry| {
            let keep = now.saturating_duration_since(entry.inserted_at) < TTL;
            if !keep {
                self.bytes -= entry.bytes;
            }
            keep
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{IconShape, IconStyle, SelectionMode, ThemeMode};

    fn settings() -> Settings {
        Settings {
            base_url: "https://example.com/v1".into(),
            model: "translation-model".into(),
            api_key: "cache-test-placeholder-not-a-secret".into(),
            ..Settings::default()
        }
    }

    #[test]
    fn repeated_selection_hits_without_changing_case_code_or_paragraphs() {
        let settings = settings();
        let mut cache = TranslationCache::new();
        cache.insert(
            &settings,
            "  First line.\n  let x = 1;  ",
            "第一行。\n  let x = 1;\n",
        );
        assert_eq!(
            cache.get(&settings, "First line.\n  let x = 1;"),
            Some("第一行。\n  let x = 1;\n".into())
        );
        for other in [
            "first line.\n  let x = 1;",
            "First line.\nlet x = 1;",
            "First line. let x = 1;",
        ] {
            assert!(cache.get(&settings, other).is_none());
        }
        cache.insert(&settings, "Other selection.", "另一段。");
        assert_eq!(
            cache.get(&settings, "Other selection."),
            Some("另一段。".into())
        );
    }

    #[test]
    fn repeatedly_read_translation_survives_lru_pressure() {
        let settings = settings();
        let mut cache = TranslationCache::new();
        for index in 0..MAX_ENTRIES {
            cache.insert(
                &settings,
                &format!("Selection {index}"),
                &format!("译文 {index}"),
            );
        }
        assert!(cache.get(&settings, "Selection 0").is_some());
        cache.insert(&settings, "Newest selection", "最新译文");
        assert!(cache.get(&settings, "Selection 1").is_none());
        assert!(cache.get(&settings, "Selection 0").is_some());
        assert_eq!(cache.entries.len(), MAX_ENTRIES);
        assert!(cache.bytes <= MAX_BYTES);
    }

    #[test]
    fn large_translations_evict_by_bytes_and_oversized_replacement_is_not_cached() {
        let settings = settings();
        let mut cache = TranslationCache::new();
        let result = "中".repeat(65_536); // 192 KiB, not 65,536 UTF-8 bytes.
        for source in ["First", "Second", "Third"] {
            cache.insert(&settings, source, &result);
        }
        assert!(cache.get(&settings, "First").is_none());
        assert!(cache.get(&settings, "Second").is_some());
        assert!(cache.get(&settings, "Third").is_some());
        assert!(cache.bytes <= MAX_BYTES);
        assert_eq!(cache.entries.len(), 2);

        cache.insert(&settings, "Third", &"x".repeat(MAX_BYTES));
        assert!(cache.get(&settings, "Third").is_none());
        assert!(cache.get(&settings, "Second").is_some());
        assert_eq!(cache.entries.len(), 1);
        assert!(cache.bytes <= MAX_BYTES);
    }

    #[test]
    fn exact_byte_budget_is_accepted_without_exceeding_memory_cap() {
        let settings = settings();
        let mut cache = TranslationCache::new();
        cache.insert(&settings, "x", &"a".repeat(MAX_BYTES - 1));
        assert_eq!(cache.bytes, MAX_BYTES);
        assert!(cache.get(&settings, "x").is_some());
        cache.insert(&settings, "next", "译文");
        assert!(cache.get(&settings, "x").is_none());
        assert!(cache.get(&settings, "next").is_some());
        assert!(cache.bytes <= MAX_BYTES);
    }

    #[test]
    fn cache_expires_at_ten_minutes_even_if_it_was_recently_read() {
        let settings = settings();
        let start = Instant::now();
        let mut cache = TranslationCache::new();
        cache.insert_at(&settings, "Time-sensitive text", "有时效的译文", start);
        assert!(cache
            .get_at(
                &settings,
                "Time-sensitive text",
                start + TTL - Duration::from_millis(1)
            )
            .is_some());
        assert!(cache
            .get_at(&settings, "Time-sensitive text", start + TTL)
            .is_none());
        assert!(cache.entries.is_empty());
        assert_eq!(cache.bytes, 0);
    }

    #[test]
    fn credentials_models_and_actual_endpoints_never_share_cached_results() {
        let original = settings();
        let mut variants = Vec::new();
        for base in [
            "https://other.example.com/v1",
            "https://example.com:8443/v1",
            "https://example.com/other",
        ] {
            variants.push(Settings {
                base_url: base.into(),
                ..original.clone()
            });
        }
        variants.push(Settings {
            model: "other-model".into(),
            ..original.clone()
        });
        variants.push(Settings {
            api_key: "different-cache-placeholder".into(),
            ..original.clone()
        });
        variants.push(Settings {
            base_url: "http://127.0.0.1/v1".into(),
            ..original.clone()
        });
        for changed in variants {
            let mut cache = TranslationCache::new();
            cache.insert(&original, "Same source", "原服务译文");
            assert!(cache.get(&changed, "Same source").is_none());
            assert!(cache.get(&original, "Same source").is_none());
        }
        let mut cache = TranslationCache::new();
        let local_http = Settings {
            base_url: "http://127.0.0.1:443/v1".into(),
            ..original.clone()
        };
        let local_https = Settings {
            base_url: "https://127.0.0.1:443/v1".into(),
            ..original
        };
        cache.insert(&local_http, "Same source", "本地 HTTP 译文");
        assert!(cache.get(&local_https, "Same source").is_none());
    }

    #[test]
    fn equivalent_urls_and_appearance_changes_keep_the_same_namespace() {
        let original = settings();
        let mut cache = TranslationCache::new();
        cache.insert(&original, "Reuse this text", "复用译文");
        let changed = Settings {
            base_url: " https://EXAMPLE.com:443/v1/chat/completions/ ".into(),
            model: " translation-model ".into(),
            icon_style: IconStyle::Letter,
            icon_color: 0x123456,
            icon_shape: IconShape::Circle,
            icon_size: 48,
            theme_mode: ThemeMode::Light,
            timeout_seconds: 55,
            selection_button: false,
            selection_mode: SelectionMode::Direct,
            compatibility_capture: true,
            ..original
        };
        assert_eq!(
            cache.get(&changed, "Reuse this text"),
            Some("复用译文".into())
        );

        let short_ip = Settings {
            base_url: "http://[::1]:11434/v1".into(),
            ..settings()
        };
        let long_ip = Settings {
            base_url: "http://[0:0:0:0:0:0:0:1]:11434/v1/".into(),
            ..short_ip.clone()
        };
        cache.insert(&short_ip, "Local text", "本地译文");
        assert_eq!(cache.get(&long_ip, "Local text"), Some("本地译文".into()));
    }

    #[test]
    fn changing_either_language_never_reuses_another_translation_pair() {
        let original = settings();
        for changed in [
            Settings {
                source_language: Language::Auto,
                ..original.clone()
            },
            Settings {
                source_language: Language::Japanese,
                ..original.clone()
            },
            Settings {
                target_language: Language::English,
                ..original.clone()
            },
            Settings {
                target_language: Language::TraditionalChinese,
                ..original.clone()
            },
        ] {
            let mut cache = TranslationCache::new();
            cache.insert(&original, "Same text", "简体中文译文");
            assert!(cache.get(&changed, "Same text").is_none());
            cache.insert(&changed, "Same text", "Different language result");
            assert_eq!(
                cache.get(&changed, "Same text"),
                Some("Different language result".into())
            );
            assert!(cache.get(&original, "Same text").is_none());
        }
    }

    #[test]
    fn invalid_endpoint_or_explicit_clear_drops_results_and_namespace() {
        let original = settings();
        let mut cache = TranslationCache::new();
        cache.insert(&original, "Known text", "已有译文");
        let invalid = Settings {
            base_url: "http://example.com/v1".into(),
            ..original.clone()
        };
        assert!(cache.get(&invalid, "Known text").is_none());
        assert!(cache.namespace.is_none());
        assert_eq!(cache.bytes, 0);
        cache.insert(&original, "Known text", "已有译文");
        cache.clear();
        assert!(cache.namespace.is_none());
        assert!(cache.get(&original, "Known text").is_none());
    }
}
