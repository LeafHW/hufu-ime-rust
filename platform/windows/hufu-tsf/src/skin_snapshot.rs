//! Immutable skin snapshots and bounded per-frame rendering projections.
use serde_json::Value;
use std::time::{Duration, Instant};

pub(crate) fn skin_retry_ready(failed_at: Option<Instant>, now: Instant) -> bool {
    failed_at.is_none_or(|at| now.saturating_duration_since(at) >= Duration::from_millis(400))
}

/// Copy the renderer's configuration without duplicating embedded widget assets.
/// Widget synchronization must keep borrowing the original snapshot.
pub(crate) fn clone_render_skin(skin: &Value) -> Value {
    match skin {
        Value::Object(fields) => Value::Object(
            fields
                .iter()
                .filter_map(|(key, value)| {
                    if key == "overlay" || key == "presets" {
                        None
                    } else {
                        let copy = if key == "skin" {
                            clone_render_skin(value)
                        } else {
                            value.clone()
                        };
                        Some((key.clone(), copy))
                    }
                })
                .collect(),
        ),
        other => other.clone(),
    }
}

pub(crate) fn key_skin_refresh_needed(
    loaded_version: Option<u64>,
    current_version: Option<u64>,
    loaded_schema: &str,
    current_schema: Option<&str>,
    schema_changed: bool,
    legacy_refresh_due: bool,
) -> bool {
    match current_version {
        Some(version) => {
            loaded_version != Some(version)
                || schema_changed
                || current_schema.is_some_and(|schema| schema != loaded_schema)
        }
        None => legacy_refresh_due,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn render_projection_omits_large_assets_but_preserves_wrapped_and_direct_settings() {
        let asset = "A".repeat(4 * 1024 * 1024);
        for wrapped in [false, true] {
            let body = serde_json::json!({
                "layout": {"font_point": 21, "show_index": false},
                "colors": {"code_color": "#123456FF"},
                "material": {"master_alpha": 0.75},
                "display": {"font_size": 16},
                "overlay": {"enabled": true, "image": asset},
                "presets": [{"overlay": {"image": asset}}]
            });
            let source = Arc::new(if wrapped {
                serde_json::json!({"skin": body, "fp": "same", "first_show_slide": true})
            } else {
                body
            });
            let shared = Arc::clone(&source);
            assert!(Arc::ptr_eq(&source, &shared));
            let projection = clone_render_skin(&shared);
            let rendered = if wrapped {
                &projection["skin"]
            } else {
                &projection
            };
            let original = if wrapped { &source["skin"] } else { &source };
            for field in ["layout", "colors", "material", "display"] {
                assert_eq!(rendered[field], original[field]);
            }
            assert!(rendered.get("overlay").is_none());
            assert!(rendered.get("presets").is_none());
            assert_eq!(
                original["overlay"]["image"].as_str().unwrap().len(),
                4 * 1024 * 1024
            );
            assert!(serde_json::to_vec(&projection).unwrap().len() < 512);
            if wrapped {
                assert_eq!(projection["first_show_slide"], true);
                assert_eq!(projection["fp"], "same");
            }
        }
    }

    #[test]
    fn key_version_skips_age_refresh_and_detects_version_schema_and_missing_snapshot() {
        assert!(!key_skin_refresh_needed(
            Some(7),
            Some(7),
            "a",
            Some("a"),
            false,
            true
        ));
        assert!(key_skin_refresh_needed(
            Some(7),
            Some(8),
            "a",
            Some("a"),
            false,
            false
        ));
        assert!(key_skin_refresh_needed(
            None,
            Some(7),
            "a",
            Some("a"),
            false,
            false
        ));
        assert!(key_skin_refresh_needed(
            Some(7),
            Some(7),
            "a",
            Some("b"),
            false,
            false
        ));
        assert!(key_skin_refresh_needed(
            Some(7),
            Some(7),
            "a",
            Some("a"),
            true,
            false
        ));
        // An old server without skin_ver retains the 500ms first-show rule.
        assert!(key_skin_refresh_needed(
            Some(7),
            None,
            "a",
            Some("a"),
            false,
            true
        ));
        assert!(!key_skin_refresh_needed(
            Some(7),
            None,
            "a",
            Some("a"),
            false,
            false
        ));
    }

    #[test]
    fn failed_skin_refresh_has_a_bounded_retry_cooldown() {
        let now = Instant::now();
        assert!(skin_retry_ready(None, now));
        assert!(!skin_retry_ready(Some(now), now));
        assert!(!skin_retry_ready(
            Some(now),
            now + Duration::from_millis(399)
        ));
        assert!(skin_retry_ready(
            Some(now),
            now + Duration::from_millis(400)
        ));
    }
}
