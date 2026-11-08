//! Prepare configuration-dependent resources without holding the input host lock.

use super::{Engine, SupplementCache};
use hufu_config::Config;
use hufu_dict::{annotation::AnnotationTable, schema::Schema, OpenCc, ShuangpinRule};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

#[derive(PartialEq, Eq)]
struct AssetKey {
    data_dir: PathBuf,
    reverse: String,
    split: String,
    opencc: bool,
    traditional: bool,
    emoji: bool,
}

impl AssetKey {
    fn new(data_dir: &Path, config: &Config) -> Self {
        Self {
            data_dir: data_dir.to_path_buf(),
            reverse: config.reverse.scheme.trim().to_string(),
            split: config.candidates.split_scheme.trim().to_string(),
            opencc: config.opencc.enabled,
            traditional: config.opencc.to_traditional,
            emoji: config.opencc.emoji,
        }
    }
}

enum ReverseOverride {
    Keep,
    Clear,
    Path(PathBuf, Option<ShuangpinRule>),
}

/// Global overrides plus enabled conversion dictionaries, loaded from one
/// configuration snapshot. A mismatched snapshot cannot overwrite newer settings.
pub struct PreparedGlobalAssets {
    key: AssetKey,
    reverse: ReverseOverride,
    pinyin: Option<AnnotationTable>,
    unicode: Option<AnnotationTable>,
    // None = retain schema fallback; Some(None) = explicitly disabled.
    split: Option<Option<AnnotationTable>>,
    opencc: Option<(bool, Option<OpenCc>)>,
    emoji: Option<Option<OpenCc>>,
}

impl PreparedGlobalAssets {
    pub fn matches(&self, data_dir: &Path, config: &Config) -> bool {
        self.key == AssetKey::new(data_dir, config)
    }

    fn apply_to_schema(&mut self, schema: &mut Schema) {
        match std::mem::replace(&mut self.reverse, ReverseOverride::Keep) {
            ReverseOverride::Keep => {}
            ReverseOverride::Clear => {
                schema.reverse = None;
                schema.reverse_path = None;
                schema.reverse_rule = None;
            }
            ReverseOverride::Path(path, rule) => {
                schema.reverse = None;
                schema.reverse_path = Some(path);
                schema.reverse_rule = rule;
            }
        }
        if let Some(table) = self.pinyin.take() {
            schema.pinyin = Some(table);
        }
        if let Some(table) = self.unicode.take() {
            schema.unicode_block = Some(table);
        }
        if let Some(table) = self.split.take() {
            schema.split = table;
        }
    }

    fn install(mut self, engine: &mut Engine) {
        self.apply_to_schema(&mut engine.schema);
        if let Some((direction, table)) = self.opencc {
            engine.opencc = table;
            engine.opencc_loaded = true;
            engine.opencc_dir = Some(direction);
        }
        if let Some(table) = self.emoji {
            engine.opencc_emoji = table;
            engine.opencc_emoji_loaded = true;
        }
        engine.cand_ver = engine.cand_ver.wrapping_add(1);
    }
}

/// Complete schema snapshot, including global overrides and the supplement index.
/// The public schema also lets callers validate its resolved directory before use.
pub struct PreparedSchema {
    pub schema: Schema,
    pub(super) target: String,
    schema_dir: String,
    pub(super) assets: PreparedGlobalAssets,
    pub(super) supplement: Arc<SupplementCache>,
}

impl PreparedSchema {
    pub fn matches(&self, data_dir: &Path, config: &Config) -> bool {
        self.schema_dir == config.schema.dir && self.assets.matches(data_dir, config)
    }
}

impl Engine {
    pub fn prepare_global_assets(data_dir: &Path, config: &Config) -> PreparedGlobalAssets {
        let key = AssetKey::new(data_dir, config);
        let reverse = if key.reverse.is_empty() {
            ReverseOverride::Clear
        } else {
            let path = data_dir
                .join("拼音反查")
                .join(format!("{}.txt", key.reverse));
            if path.exists() {
                ReverseOverride::Path(path, None)
            } else if let Some(rule) = ShuangpinRule::from_scheme(&key.reverse) {
                let path = data_dir.join("拼音反查").join("全拼.txt");
                if path.exists() {
                    ReverseOverride::Path(path, Some(rule))
                } else {
                    ReverseOverride::Keep
                }
            } else {
                ReverseOverride::Keep
            }
        };
        let pinyin = AnnotationTable::load(&data_dir.join("注释").join("拼音.注释")).ok();
        let unicode = AnnotationTable::load(&data_dir.join("注释").join("unicode.注释")).ok();
        let split = if key.split.is_empty() {
            Some(None)
        } else {
            AnnotationTable::load(&data_dir.join("拆分").join(format!("{}.拆分", key.split)))
                .ok()
                .map(Some)
        };
        let dir = data_dir.join("转换词典");
        let opencc = if key.opencc {
            let tables = if key.traditional {
                &["STPhrases", "STCharacters", "STCharacters_Tu"][..]
            } else {
                &["TSPhrases", "TSCharacters"][..]
            };
            let table = OpenCc::load_dir(&dir, tables);
            if table.is_empty() {
                Self::log_opencc_missing_at(data_dir, key.traditional);
            }
            Some((
                key.traditional,
                if table.is_empty() { None } else { Some(table) },
            ))
        } else {
            None
        };
        let emoji = if key.emoji {
            let table = OpenCc::load_dir_full(&dir, &["emoji"]);
            Some(if table.is_empty() { None } else { Some(table) })
        } else {
            None
        };
        PreparedGlobalAssets {
            key,
            reverse,
            pinyin,
            unicode,
            split,
            opencc,
            emoji,
        }
    }

    /// Install already-read resources. False means relevant settings changed;
    /// callers should prepare again from the latest snapshot, outside their lock.
    pub fn apply_prepared_global_assets(&mut self, prepared: PreparedGlobalAssets) -> bool {
        if !prepared.matches(&self.data_dir, &self.config) {
            return false;
        }
        prepared.install(self);
        true
    }

    pub fn prepare_schema_for(
        data_dir: &Path,
        config: &Config,
        name: &str,
    ) -> std::io::Result<PreparedSchema> {
        let mut schema = Self::load_schema_for(data_dir, &config.schema.dir, name)?;
        let mut assets = Self::prepare_global_assets(data_dir, config);
        // Replacing schema-local fallback tables also happens on the worker.
        assets.apply_to_schema(&mut schema);
        let supplement = Arc::new(Self::build_supplement_cache(&schema));
        Ok(PreparedSchema {
            schema,
            target: name.to_string(),
            schema_dir: config.schema.dir.clone(),
            assets,
            supplement,
        })
    }

    pub fn apply_prepared_schema(
        &mut self,
        name: &str,
        prepared: PreparedSchema,
    ) -> std::io::Result<()> {
        if prepared.target != name || !prepared.matches(&self.data_dir, &self.config) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "stale prepared schema assets",
            ));
        }
        let _ = self.switch_pending.take();
        self.install_prepared_schema(name, prepared);
        Ok(())
    }

    pub(super) fn install_prepared_schema(
        &mut self,
        name: &str,
        prepared: PreparedSchema,
    ) -> Schema {
        self.rerank_generation = self.rerank_generation.wrapping_add(1);
        let old_name = self.config.schema.current.clone();
        let old_schema = std::mem::replace(&mut self.schema, prepared.schema);
        self.session_learned.clear();
        *self.supp_hold.lock().unwrap_or_else(|p| p.into_inner()) = Some(prepared.supplement);
        prepared.assets.install(self);
        self.config.schema.current = name.to_string();
        if old_name != name && !old_name.is_empty() {
            self.config.schema.recent_pair = Some((old_name, name.to_string()));
        }
        old_schema
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Session;
    use hufu_types::KeyInput;

    fn fixture(tag: &str) -> (Engine, PathBuf) {
        let root =
            std::env::temp_dir().join(format!("hufu-prepared-assets-{}-{tag}", std::process::id()));
        for name in ["one", "two"] {
            let dir = root.join("schemas").join(name);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("main.txt"), "#hufu-dict v1 name=t\nh\t后\n").unwrap();
            std::fs::write(dir.join("拼音.注释"), "后\tlocal-pinyin\n").unwrap();
            std::fs::write(dir.join("本地.拆分"), "后\tlocal-split\n").unwrap();
            std::fs::write(dir.join("反查.txt"), "后\thou\n").unwrap();
        }
        let mut config = Config::default();
        config.schema.dir = "schemas".into();
        config.schema.current = "one".into();
        config.reverse.scheme = "missing-global".into();
        config.candidates.split_scheme = "missing-global".into();
        config.user.auto_frequency = false;
        config.user.log_adjust = false;
        let mut engine = Engine::with_schema_dir(&root.join("schemas/one"), config).unwrap();
        engine.data_dir = root.clone();
        (engine, root)
    }

    #[test]
    fn prepared_schema_keeps_fallback_and_applies_without_source_files() {
        let (mut engine, root) = fixture("no-read");
        std::fs::create_dir_all(root.join("注释")).unwrap();
        std::fs::write(root.join("注释/拼音.注释"), "后\tglobal-pinyin\n").unwrap();
        let prepared = Engine::prepare_schema_for(&root, &engine.config, "two").unwrap();
        assert_eq!(
            prepared.schema.pinyin.as_ref().unwrap().get('后'),
            Some("global-pinyin")
        );
        assert_eq!(
            prepared.schema.split.as_ref().unwrap().get('后'),
            Some("local-split")
        );
        assert!(prepared
            .schema
            .reverse_path
            .as_ref()
            .unwrap()
            .ends_with("反查.txt"));
        // Prepared data must remain usable if files disappear after preparation.
        std::fs::remove_dir_all(root.join("schemas/two")).unwrap();
        std::fs::remove_dir_all(root.join("注释")).unwrap();
        engine.apply_prepared_schema("two", prepared).unwrap();
        assert_eq!(engine.config.schema.current, "two");
        assert_eq!(
            engine.schema.pinyin.as_ref().unwrap().get('后'),
            Some("global-pinyin")
        );
        assert_eq!(
            engine.schema.split.as_ref().unwrap().get('后'),
            Some("local-split")
        );
        let out = engine.process_key(&mut Session::new(true), KeyInput::char_key('h'));
        assert_eq!(out.state.unwrap().candidates[0].text, "后");
        drop(engine);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn stale_preparation_cannot_replace_new_settings_or_schema() {
        let (mut engine, root) = fixture("stale");
        let prepared = Engine::prepare_schema_for(&root, &engine.config, "two").unwrap();
        engine.config.candidates.split_scheme = "new-choice".into();
        assert!(!prepared.matches(&root, &engine.config));
        assert!(engine.apply_prepared_schema("two", prepared).is_err());
        assert_eq!(engine.config.schema.current, "one");
        assert!(engine.schema.dir.ends_with("one"));
        let prepared = Engine::prepare_global_assets(&root, &engine.config);
        engine.config.reverse.scheme.clear();
        assert!(!engine.apply_prepared_global_assets(prepared));
        let prepared = Engine::prepare_schema_for(&root, &engine.config, "two").unwrap();
        engine.config.schema.dir = "other-root".into();
        assert!(!prepared.matches(&root, &engine.config));
        drop(engine);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn prepared_conversion_is_ready_and_disabled_resources_stay_disabled() {
        let (mut engine, root) = fixture("conversion");
        std::fs::create_dir_all(root.join("转换词典")).unwrap();
        std::fs::write(root.join("转换词典/STCharacters.txt"), "后\t後\n").unwrap();
        std::fs::write(root.join("转换词典/emoji.txt"), "后\t后 👑\n").unwrap();
        engine.config.opencc.enabled = true;
        engine.config.opencc.to_traditional = true;
        engine.config.opencc.emoji = true;
        engine.config.reverse.scheme.clear();
        engine.config.candidates.split_scheme.clear();
        let prepared = Engine::prepare_global_assets(&root, &engine.config);
        std::fs::remove_dir_all(root.join("转换词典")).unwrap();
        assert!(engine.apply_prepared_global_assets(prepared));
        assert!(engine.schema.reverse.is_none());
        assert!(engine.schema.reverse_path.is_none());
        assert!(engine.schema.split.is_none());
        let out = engine.process_key(&mut Session::new(true), KeyInput::char_key('h'));
        let candidates = out.state.unwrap().candidates;
        assert_eq!(candidates[0].text, "後");
        assert!(candidates.iter().any(|c| c.text == "👑"));
        drop(engine);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn background_switch_reprepares_after_config_change_and_cancels_on_drop() {
        let (mut engine, root) = fixture("background");
        engine.begin_schema_switch("two");
        // begin cloned the old setting. The old prepared local split must not
        // overwrite this explicit disable when its worker result arrives.
        engine.config.candidates.split_scheme.clear();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        let mut session = Session::new(true);
        while engine.config.schema.current != "two" {
            assert!(
                std::time::Instant::now() < deadline,
                "background switch did not settle"
            );
            engine.apply_pending_schema_switch(&mut session);
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert!(engine.schema.split.is_none());
        assert!(!engine.schema_switch_pending());
        engine.begin_schema_switch("one");
        // Drop must disconnect the retirement channel before joining the worker,
        // including when preparation finishes before cancellation is observed.
        drop(engine);
        std::fs::remove_dir_all(root).unwrap();
    }
}
