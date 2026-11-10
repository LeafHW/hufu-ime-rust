//! Canonical on-disk trainer dictionaries, independent of IME composition/learning.
//!
//! A single loader owns parsing, but never the cache or input Host mutex. The
//! selected source generation guards publication, including an A -> B -> A switch.
use hufu_dict::Schema;
use std::{
    collections::{HashSet, VecDeque},
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Condvar, Mutex, OnceLock},
    time::{Duration, Instant, SystemTime},
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct SourceKey {
    pub data_dir: PathBuf,
    pub dir: PathBuf,
    pub name: String,
}

#[derive(Clone)]
pub(super) struct SourceSnapshot {
    pub schema: Arc<Schema>,
    pub revision: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
struct FileStamp {
    path: PathBuf,
    bytes: u64,
    // Keep the filesystem's full timestamp precision (NTFS: 100 ns), not seconds.
    modified: SystemTime,
    missing: bool,
}

type Fingerprint = Vec<FileStamp>;
type Observation = Result<Fingerprint, String>;
type Loaded = Result<Arc<Schema>, String>;
type Snapshot = Result<SourceSnapshot, String>;

#[derive(Default)]
struct State {
    key: Option<SourceKey>,
    generation: u64,
    revision: u64,
    value: Option<Snapshot>,
    baseline: Option<Observation>,
    pending: Option<Observation>,
    dependencies: Vec<PathBuf>,
    loading: bool,
    failures: u32,
    retry_at: Option<Instant>,
}

#[derive(Default)]
struct SourceCache {
    state: Mutex<State>,
    finished: Condvar,
}

static SOURCES: OnceLock<Result<Arc<SourceCache>, String>> = OnceLock::new();

fn shared() -> Result<&'static Arc<SourceCache>, String> {
    SOURCES
        .get_or_init(|| {
            let cache = Arc::new(SourceCache::default());
            let worker = cache.clone();
            std::thread::Builder::new()
                .name("hufu-trainer-sources".into())
                .spawn(move || loop {
                    std::thread::sleep(Duration::from_millis(350));
                    worker.refresh();
                })
                .map_err(|e| format!("无法启动词提码表监视: {e}"))?;
            Ok(cache)
        })
        .as_ref()
        .map_err(Clone::clone)
}

/// Caller must release the input Host lock before entering this function.
/// Only the first request for a selected source waits for its initial disk load.
pub(super) fn get(key: SourceKey) -> Snapshot {
    shared()?.get(key)
}

/// Deterministic API-test hook: use the exact worker observation/publication path.
#[cfg(test)]
pub(super) fn refresh_now(key: &SourceKey) -> Result<(), String> {
    let cache = shared()?;
    // Initial errors still select the source; a later repair must be refreshable.
    let _ = cache.get(key.clone());
    cache.refresh_now(key)
}

fn switched() -> String {
    "词提码表来源已切换，请重试".into()
}

impl SourceCache {
    fn select(&self, key: &SourceKey) -> u64 {
        let (generation, retired) = {
            let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
            let retired = if state.key.as_ref() != Some(key) {
                state.generation = state.generation.wrapping_add(1);
                state.revision = state.revision.wrapping_add(1);
                state.key = Some(key.clone());
                state.baseline = None;
                state.pending = None;
                state.dependencies.clear();
                state.failures = 0;
                state.retry_at = None;
                state.value.take()
            } else {
                None
            };
            (state.generation, retired)
        };
        // A large old dictionary must never be destroyed under the cache mutex.
        drop(retired);
        self.finished.notify_all();
        generation
    }

    fn matches(state: &State, key: &SourceKey, generation: u64) -> bool {
        state.generation == generation && state.key.as_ref() == Some(key)
    }

    fn get(&self, key: SourceKey) -> Snapshot {
        self.get_with(key, load_snapshot)
    }

    fn get_with(
        &self,
        key: SourceKey,
        load: impl FnOnce(&SourceKey, &Fingerprint) -> Loaded,
    ) -> Snapshot {
        let generation = self.select(&key);
        let dependencies = {
            let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
            loop {
                if !Self::matches(&state, &key, generation) {
                    return Err(switched());
                }
                if let Some(value) = &state.value {
                    return value.clone();
                }
                if !state.loading {
                    state.loading = true;
                    break state.dependencies.clone();
                }
                state = self.finished.wait(state).unwrap_or_else(|p| p.into_inner());
            }
        };
        let observed = fingerprint_with(&key, &dependencies);
        self.load_and_publish(
            &key,
            generation,
            dependencies,
            observed,
            load,
            Instant::now(),
        );
        self.current(&key, generation)
    }

    fn current(&self, key: &SourceKey, generation: u64) -> Snapshot {
        let state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        if !Self::matches(&state, key, generation) {
            return Err(switched());
        }
        state
            .value
            .as_ref()
            .cloned()
            .unwrap_or_else(|| Err("词提码表尚未加载，请稍后重试".into()))
    }

    fn publish(
        &self,
        key: &SourceKey,
        generation: u64,
        observed: Observation,
        dependencies: Vec<PathBuf>,
        loaded: Loaded,
        now: Instant,
    ) {
        let mut uncommitted = Some(loaded);
        let retired = {
            let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
            let retired = if Self::matches(&state, key, generation) {
                let loaded = uncommitted.take().expect("one load result per publication");
                state.revision = state.revision.wrapping_add(1);
                let revision = state.revision;
                if loaded.is_err() {
                    state.failures = state.failures.saturating_add(1);
                    // A sharing violation can disappear without changing metadata.
                    // Retry failed sources at 1/2/4 seconds, capped at four seconds.
                    let seconds = 1u64 << (state.failures.saturating_sub(1).min(2));
                    state.retry_at = Some(now + Duration::from_secs(seconds));
                } else {
                    state.failures = 0;
                    state.retry_at = None;
                }
                state.baseline = Some(observed);
                state.dependencies = dependencies;
                state.pending = None;
                state
                    .value
                    .replace(loaded.map(|schema| SourceSnapshot { schema, revision }))
            } else {
                None
            };
            state.loading = false;
            retired
        };
        self.finished.notify_all();
        drop(retired);
        drop(uncommitted);
    }

    fn refresh(&self) {
        self.refresh_with(Instant::now(), load_snapshot);
    }

    fn refresh_with(&self, now: Instant, load: impl FnOnce(&SourceKey, &Fingerprint) -> Loaded) {
        let (key, generation, dependencies) = {
            let state = self.state.lock().unwrap_or_else(|p| p.into_inner());
            if state.loading {
                return;
            }
            let Some(key) = &state.key else { return };
            (key.clone(), state.generation, state.dependencies.clone())
        };
        // Healthy unchanged sources only enumerate/stat files. Failed sources
        // additionally retry with a bounded delay, even when metadata is unchanged.
        let observed = fingerprint_with(&key, &dependencies);
        {
            let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
            if !Self::matches(&state, &key, generation) || state.loading {
                return;
            }
            if state.baseline.as_ref() == Some(&observed) {
                state.pending = None;
                if !state.retry_at.is_some_and(|retry_at| now >= retry_at) {
                    return;
                }
            } else if state.pending.as_ref() != Some(&observed) {
                state.pending = Some(observed);
                return;
            }
            // Two consecutive identical changed observations: claim the sole loader.
            state.loading = true;
        }
        self.load_and_publish(&key, generation, dependencies, observed, load, now);
    }

    fn load_and_publish(
        &self,
        key: &SourceKey,
        generation: u64,
        previous_dependencies: Vec<PathBuf>,
        observed: Observation,
        load: impl FnOnce(&SourceKey, &Fingerprint) -> Loaded,
        now: Instant,
    ) {
        let (dependencies, baseline, discovery_error) =
            prepare_sources(key, previous_dependencies, observed);
        let loaded = if let Some(error) = discovery_error {
            Err(error)
        } else {
            match &baseline {
                Ok(files) => load(key, files),
                Err(e) => Err(e.clone()),
            }
        };
        self.publish(key, generation, baseline, dependencies, loaded, now);
    }

    #[cfg(test)]
    fn get_schema(&self, key: SourceKey) -> Loaded {
        self.get(key).map(|source| source.schema)
    }

    #[cfg(test)]
    fn refresh_now(&self, key: &SourceKey) -> Result<(), String> {
        let generation = {
            let state = self.state.lock().unwrap_or_else(|p| p.into_inner());
            if state.key.as_ref() != Some(key) {
                return Err(switched());
            }
            state.generation
        };
        for _ in 0..2 {
            self.wait_for_loader(key, generation)?;
            self.refresh();
        }
        self.wait_for_loader(key, generation)?;
        self.current(key, generation).map(|_| ())
    }

    #[cfg(test)]
    fn wait_for_loader(&self, key: &SourceKey, generation: u64) -> Result<(), String> {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        while Self::matches(&state, key, generation) && state.loading {
            state = self.finished.wait(state).unwrap_or_else(|p| p.into_inner());
        }
        if Self::matches(&state, key, generation) {
            Ok(())
        } else {
            Err(switched())
        }
    }
}

fn consumed(path: &Path) -> bool {
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    // Schema::load dispatches these roles before checking the extension.
    if matches!(
        stem.as_str(),
        "快符"
            | "常用符号"
            | "一简符号"
            | "补充语料"
            | "用户调整"
            | "用户词"
            | "个人存储表"
            | "个人存储"
    ) {
        return true;
    }
    let ext = path
        .extension()
        .map(|s| s.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    matches!(ext.as_str(), "txt" | "yaml" | "yml" | "注释" | "拆分")
}

fn stamp(path: PathBuf, metadata: fs::Metadata) -> Result<FileStamp, String> {
    let modified = metadata
        .modified()
        .map_err(|e| format!("无法读取词提码表时间 {}: {e}", path.display()))?;
    Ok(FileStamp {
        path,
        bytes: metadata.len(),
        modified,
        missing: false,
    })
}

fn missing_stamp(path: PathBuf) -> FileStamp {
    FileStamp {
        path,
        bytes: 0,
        modified: SystemTime::UNIX_EPOCH,
        missing: true,
    }
}

#[cfg(test)]
fn fingerprint(key: &SourceKey) -> Observation {
    fingerprint_with(key, &[])
}

fn fingerprint_with(key: &SourceKey, dependencies: &[PathBuf]) -> Observation {
    let entries = fs::read_dir(&key.dir)
        .map_err(|e| format!("无法读取词提方案目录 {}: {e}", key.dir.display()))?;
    let mut files = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| format!("无法枚举词提码表: {e}"))?;
        let path = entry.path();
        if !consumed(&path) {
            continue;
        }
        let metadata =
            fs::metadata(&path).map_err(|e| format!("无法读取词提码表 {}: {e}", path.display()))?;
        if metadata.is_file() {
            files.push(stamp(path, metadata)?);
        }
    }
    for stem in ["个人存储表", "个人存储"] {
        for ext in ["txt", "yaml"] {
            let path = key.data_dir.join(format!("{stem}.{ext}"));
            match fs::metadata(&path) {
                Ok(metadata) if metadata.is_file() => files.push(stamp(path, metadata)?),
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(format!("无法读取全局个人存储表 {}: {e}", path.display())),
            }
        }
    }
    for path in dependencies {
        match fs::metadata(path) {
            Ok(metadata) => files.push(stamp(path.clone(), metadata)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                files.push(missing_stamp(path.clone()));
            }
            Err(e) => return Err(format!("无法读取词提导入码表 {}: {e}", path.display())),
        }
    }
    files.sort();
    files.dedup();
    Ok(files)
}

fn rime_source(key: &SourceKey, path: &Path) -> bool {
    if path.parent() != Some(key.dir.as_path()) {
        return false;
    }
    let ext = path
        .extension()
        .map(|s| s.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    matches!(ext.as_str(), "yaml" | "yml")
        && !matches!(
            stem.as_str(),
            "快符"
                | "常用符号"
                | "一简符号"
                | "补充语料"
                | "用户调整"
                | "用户词"
                | "个人存储表"
                | "个人存储"
        )
}

/// Parse metadata only, never dictionary rows. Use the same full-file decoding
/// decision as parse_file, including legacy GBK files and UTF-8 BOM handling.
fn import_header(path: &Path) -> Result<(FileStamp, Vec<String>), String> {
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok((missing_stamp(path.to_path_buf()), Vec::new()));
        }
        Err(e) => return Err(format!("无法读取词提导入码表 {}: {e}", path.display())),
    };
    let before = stamp(path.to_path_buf(), metadata)?;
    let mut lines = hufu_dict::parse::read_lines(path)
        .map_err(|e| format!("无法读取词提导入头 {}: {e}", path.display()))?;
    let imports =
        if hufu_dict::parse::sniff_format(&lines) == hufu_dict::parse::TableFormat::RimeYaml {
            let end = lines
                .iter()
                .position(|line| {
                    let t = line.trim();
                    t == "..." || (t.contains('\t') && !t.contains(": ") && !t.ends_with(':'))
                })
                .unwrap_or(lines.len());
            lines.truncate(end);
            hufu_dict::parse::rime::parse(&lines).meta.imports
        } else {
            Vec::new()
        };
    let after = fs::metadata(path)
        .map_err(|e| format!("词提导入头读取期间文件变化 {}: {e}", path.display()))?;
    if stamp(path.to_path_buf(), after)? != before {
        return Err("词提导入码表正在写入，请稍后重试".into());
    }
    Ok((before, imports))
}

fn discover_imports(
    key: &SourceKey,
    files: &Fingerprint,
    dependencies: &mut Vec<PathBuf>,
    read_stamps: &mut Fingerprint,
) -> Result<(), String> {
    for source in files
        .iter()
        .filter(|file| !file.missing && rime_source(key, &file.path))
    {
        let (sample, imports) = import_header(&source.path)?;
        read_stamps.push(sample);
        let mut queue: VecDeque<String> = imports.into();
        let mut visited = HashSet::new();
        while let Some(name) = queue.pop_front() {
            if !visited.insert(name.clone()) {
                continue;
            }
            // Mirror merge_import_closure: every descendant resolves against
            // the original top-level base, not its immediate importing file.
            let path = source.path.with_file_name(format!("{name}.dict.yaml"));
            dependencies.push(path.clone());
            let (sample, imports) = import_header(&path)?;
            read_stamps.push(sample);
            queue.extend(imports);
        }
    }
    Ok(())
}

fn prepare_sources(
    key: &SourceKey,
    previous_dependencies: Vec<PathBuf>,
    observed: Observation,
) -> (Vec<PathBuf>, Observation, Option<String>) {
    let files = match observed {
        Ok(files) => files,
        Err(_) => match fingerprint_with(key, &[]) {
            // An obsolete imported file can become inaccessible. Still read the
            // current roots: the user may already have removed that import.
            Ok(files) => files,
            Err(e) => return (previous_dependencies, Err(e), None),
        },
    };
    let mut dependencies = Vec::new();
    let mut read_stamps = Vec::new();
    let discovered = discover_imports(key, &files, &mut dependencies, &mut read_stamps);
    if discovered.is_err() {
        // Retain known paths if an unavailable header prevented complete discovery.
        dependencies.extend(previous_dependencies);
    }
    dependencies.sort();
    dependencies.dedup();
    let baseline = fingerprint_with(key, &dependencies);
    let discovery_error = match discovered {
        Err(e) => Some(e),
        Ok(()) => match &baseline {
            Ok(current) if read_stamps.iter().all(|sample| current.contains(sample)) => None,
            Ok(_) => Some("词提导入码表正在写入，请稍后重试".into()),
            Err(_) => None,
        },
    };
    (dependencies, baseline, discovery_error)
}

fn load_snapshot(key: &SourceKey, before: &Fingerprint) -> Loaded {
    // Some legacy Schema::load branches tolerate unreadable optional files. Do
    // not report a successful canonical snapshot when an existing source is locked.
    for file in before.iter().filter(|file| !file.missing) {
        fs::File::open(&file.path)
            .map_err(|e| format!("无法读取词提码表 {}: {e}", file.path.display()))?;
    }
    let mut schema = Schema::load(&key.dir)
        .map_err(|e| format!("词提码表加载失败 {}: {e}", key.dir.display()))?;
    // Match Engine::load_schema_for, without global annotations/conversion assets.
    for stem in ["个人存储表", "个人存储"] {
        for ext in ["txt", "yaml"] {
            schema.merge_personal_file(&key.data_dir.join(format!("{stem}.{ext}")));
        }
    }
    let watched: Vec<PathBuf> = before.iter().map(|file| file.path.clone()).collect();
    if fingerprint_with(key, &watched)?.as_slice() != before.as_slice() {
        return Err("词提码表正在写入，请稍后重试".into());
    }
    if schema.dict.entries.is_empty()
        && schema.super_dict.entries.is_empty()
        && schema.personal.entries.is_empty()
        && schema.user_dict.entries.is_empty()
    {
        return Err(format!(
            "词提码表没有可用词条，请检查文件内容: {}",
            key.dir.display()
        ));
    }
    schema.symbols = Default::default();
    schema.supplement = Default::default();
    schema.pinyin = None;
    schema.unicode_block = None;
    schema.split = None;
    schema.reverse = None;
    schema.reverse_path = None;
    schema.reverse_rule = None;
    schema.encoder_rules.clear();
    Ok(Arc::new(schema))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicU64, Ordering},
        mpsc,
    };

    static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

    struct Fixture {
        key: SourceKey,
    }

    impl Fixture {
        fn new(tag: &str) -> Self {
            let serial = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
            let data_dir = std::env::temp_dir().join(format!(
                "hufu-trainer-sources-{}-{tag}-{serial}",
                std::process::id()
            ));
            let dir = data_dir.join("码表").join("Test");
            fs::create_dir_all(&dir).unwrap();
            fs::write(
                dir.join("main.txt"),
                "#hufu-dict v1 name=Test\na\t甲\na\t乙\n",
            )
            .unwrap();
            Self {
                key: SourceKey {
                    data_dir,
                    dir,
                    name: "Test".into(),
                },
            }
        }

        fn write(&self, name: &str, content: &str) {
            fs::write(self.key.dir.join(name), content).unwrap();
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.key.data_dir);
        }
    }

    fn texts(schema: &Schema, code: &str) -> Vec<String> {
        schema
            .candidates(code)
            .iter()
            .map(|e| e.text.clone())
            .collect()
    }

    #[test]
    fn same_second_same_length_rewrite_is_seen_and_waits_for_two_stable_ticks() {
        let fixture = Fixture::new("nanos");
        let path = fixture.key.dir.join("main.txt");
        let second = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(second + Duration::from_nanos(100_000_000))
            .unwrap();
        let cache = SourceCache::default();
        let first = cache.get_schema(fixture.key.clone()).unwrap();
        let before = fingerprint(&fixture.key).unwrap();
        fixture.write("main.txt", "#hufu-dict v1 name=Test\na\t丙\na\t丁\n");
        fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(second + Duration::from_nanos(800_000_000))
            .unwrap();
        let after = fingerprint(&fixture.key).unwrap();
        assert_eq!(before[0].bytes, after[0].bytes);
        assert_eq!(
            before[0].modified.duration_since(second).unwrap().as_secs(),
            0
        );
        assert_eq!(
            after[0].modified.duration_since(second).unwrap().as_secs(),
            0
        );
        assert_ne!(before, after);
        cache.refresh();
        assert!(Arc::ptr_eq(
            &first,
            &cache.get_schema(fixture.key.clone()).unwrap()
        ));
        cache.refresh();
        let next = cache.get_schema(fixture.key.clone()).unwrap();
        assert!(!Arc::ptr_eq(&first, &next));
        assert_eq!(texts(&next, "a"), ["丙", "丁"]);
        cache.refresh();
        cache.refresh();
        assert!(Arc::ptr_eq(
            &next,
            &cache.get_schema(fixture.key.clone()).unwrap()
        ));
    }

    #[test]
    fn plain_yaml_yml_add_remove_and_rename_refresh_the_canonical_dictionary() {
        let fixture = Fixture::new("extensions");
        let cache = SourceCache::default();
        let first = cache.get_schema(fixture.key.clone()).unwrap();
        fixture.write("五笔86.yaml", "---\nname: plain\n...\n普通词\tab\n");
        fixture.write("随意名字.yml", "---\nname: other\n...\n短词\tcd\n");
        cache.refresh_now(&fixture.key).unwrap();
        let added = cache.get_schema(fixture.key.clone()).unwrap();
        assert_eq!(texts(&added, "ab"), ["普通词"]);
        assert_eq!(texts(&added, "cd"), ["短词"]);
        assert!(!Arc::ptr_eq(&first, &added));
        fs::rename(
            fixture.key.dir.join("五笔86.yaml"),
            fixture.key.dir.join("改名.yml"),
        )
        .unwrap();
        cache.refresh_now(&fixture.key).unwrap();
        let renamed = cache.get_schema(fixture.key.clone()).unwrap();
        assert!(!Arc::ptr_eq(&added, &renamed));
        assert_eq!(texts(&renamed, "ab"), ["普通词"]);
        fs::remove_file(fixture.key.dir.join("随意名字.yml")).unwrap();
        cache.refresh_now(&fixture.key).unwrap();
        assert!(texts(&cache.get_schema(fixture.key.clone()).unwrap(), "cd").is_empty());
        fs::rename(
            fixture.key.dir.join("改名.yml"),
            fixture.key.dir.join("改名.bak"),
        )
        .unwrap();
        cache.refresh_now(&fixture.key).unwrap();
        assert!(texts(&cache.get_schema(fixture.key.clone()).unwrap(), "ab").is_empty());
    }

    #[test]
    fn user_file_reloads_keep_embedded_adjustments_and_small_user_tables() {
        let fixture = Fixture::new("adjustments");
        fixture.write(
            "main.txt",
            "#hufu-dict v1 name=Test\na\t甲\na\t乙\n{删除}a\t甲\n",
        );
        fixture.write("导出用户码表.txt", "#hufu-dict v1 name=user\nz\t自造词\n");
        let cache = SourceCache::default();
        assert_eq!(
            texts(&cache.get_schema(fixture.key.clone()).unwrap(), "a"),
            ["乙"]
        );
        fixture.write("用户调整.txt", "{添加}a\t新词\tp2\n{置顶}a\t新词\n");
        cache.refresh_now(&fixture.key).unwrap();
        let adjusted = cache.get_schema(fixture.key.clone()).unwrap();
        assert_eq!(texts(&adjusted, "a"), ["新词", "乙"]);
        assert_eq!(texts(&adjusted, "z"), ["自造词"]);
        fixture.write("用户调整.txt", "{删除}a\t乙\n");
        cache.refresh_now(&fixture.key).unwrap();
        assert!(texts(&cache.get_schema(fixture.key.clone()).unwrap(), "a").is_empty());
        fs::remove_file(fixture.key.dir.join("用户调整.txt")).unwrap();
        cache.refresh_now(&fixture.key).unwrap();
        let removed = cache.get_schema(fixture.key.clone()).unwrap();
        assert_eq!(texts(&removed, "a"), ["乙"]);
        assert_eq!(texts(&removed, "z"), ["自造词"]);
        let mut learned = (*removed).clone();
        learned.user_dict.add_word("a", "仅会话学习");
        assert!(Arc::ptr_eq(
            &removed,
            &cache.get_schema(fixture.key.clone()).unwrap()
        ));
        assert_eq!(
            texts(&cache.get_schema(fixture.key.clone()).unwrap(), "a"),
            ["乙"]
        );
    }

    #[test]
    fn global_personal_files_and_special_stems_are_monitored() {
        let fixture = Fixture::new("personal");
        let cache = SourceCache::default();
        cache.get_schema(fixture.key.clone()).unwrap();
        fs::write(
            fixture.key.data_dir.join("个人存储表.yaml"),
            "#hufu-dict v1 name=p\np\t全局词\n",
        )
        .unwrap();
        cache.refresh_now(&fixture.key).unwrap();
        assert_eq!(
            texts(&cache.get_schema(fixture.key.clone()).unwrap(), "p"),
            ["全局词"]
        );
        fixture.write("个人存储.自定义", "#hufu-dict v1 name=p\nx\t方案词\n");
        cache.refresh_now(&fixture.key).unwrap();
        assert_eq!(
            texts(&cache.get_schema(fixture.key.clone()).unwrap(), "x"),
            ["方案词"]
        );
        fs::remove_file(fixture.key.data_dir.join("个人存储表.yaml")).unwrap();
        cache.refresh_now(&fixture.key).unwrap();
        assert!(texts(&cache.get_schema(fixture.key.clone()).unwrap(), "p").is_empty());
    }

    #[test]
    fn missing_directory_and_invalid_dictionary_report_errors_then_recover() {
        let fixture = Fixture::new("recover");
        let cache = SourceCache::default();
        cache.get_schema(fixture.key.clone()).unwrap();
        fixture.write("main.txt", "这个文件已经没有合法的码表行");
        assert!(cache
            .refresh_now(&fixture.key)
            .unwrap_err()
            .contains("没有可用词条"));
        assert!(cache.get_schema(fixture.key.clone()).is_err());
        fixture.write("main.txt", "#hufu-dict v1 name=Test\nb\t恢复词\n");
        cache.refresh_now(&fixture.key).unwrap();
        assert_eq!(
            texts(&cache.get_schema(fixture.key.clone()).unwrap(), "b"),
            ["恢复词"]
        );
        fs::remove_dir_all(&fixture.key.dir).unwrap();
        assert!(cache
            .refresh_now(&fixture.key)
            .unwrap_err()
            .contains("无法读取词提方案目录"));
        assert!(cache.get_schema(fixture.key.clone()).is_err());
        fs::create_dir_all(&fixture.key.dir).unwrap();
        fixture.write("main.txt", "#hufu-dict v1 name=Test\nc\t重建词\n");
        cache.refresh_now(&fixture.key).unwrap();
        assert_eq!(
            texts(&cache.get_schema(fixture.key.clone()).unwrap(), "c"),
            ["重建词"]
        );
    }

    #[test]
    fn a_b_a_switch_rejects_an_old_inflight_load_and_serializes_parsing() {
        let a = Fixture::new("generation-a");
        let b = Fixture::new("generation-b");
        let cache = Arc::new(SourceCache::default());
        let (started_tx, started_rx) = mpsc::channel();
        let (resume_tx, resume_rx) = mpsc::channel();
        let thread_cache = cache.clone();
        let key = a.key.clone();
        let worker = std::thread::spawn(move || {
            thread_cache.get_with(key, |key, files| {
                started_tx.send(()).unwrap();
                resume_rx.recv().unwrap();
                load_snapshot(key, files)
            })
        });
        started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        cache.select(&b.key);
        cache.select(&a.key);
        {
            let state = cache.state.lock().unwrap();
            assert!(
                state.loading,
                "source switches cannot start a parallel loader"
            );
            assert!(state.value.is_none());
        }
        resume_tx.send(()).unwrap();
        assert!(worker.join().unwrap().err().unwrap().contains("来源已切换"));
        assert!(
            cache.state.lock().unwrap().value.is_none(),
            "old A cannot publish into new A"
        );
        assert_eq!(
            texts(&cache.get_schema(a.key.clone()).unwrap(), "a"),
            ["甲", "乙"]
        );
    }

    #[test]
    fn revision_is_stable_for_reads_but_advances_for_reload_errors_and_a_b_a() {
        let a = Fixture::new("revision-a");
        let b = Fixture::new("revision-b");
        let cache = SourceCache::default();
        let first = cache.get(a.key.clone()).unwrap();
        assert_eq!(first.revision, cache.get(a.key.clone()).unwrap().revision);
        // A successful unchanged tick cannot parse or advance the revision.
        cache.refresh_with(Instant::now(), |_, _| panic!("unchanged source reparsed"));
        assert_eq!(first.revision, cache.get(a.key.clone()).unwrap().revision);
        a.write("main.txt", "#hufu-dict v1 name=Test\na\t更新后的词\n");
        cache.refresh_now(&a.key).unwrap();
        let changed = cache.get(a.key.clone()).unwrap().revision;
        assert!(changed > first.revision);
        a.write("main.txt", "无效内容");
        assert!(cache.refresh_now(&a.key).is_err());
        let failed = cache.state.lock().unwrap().revision;
        assert!(failed > changed);
        a.write("main.txt", "#hufu-dict v1 name=Test\na\t甲\na\t乙\n");
        cache.refresh_now(&a.key).unwrap();
        let restored = cache.get(a.key.clone()).unwrap().revision;
        assert!(restored > failed);
        let b_revision = cache.get(b.key.clone()).unwrap().revision;
        let a_revision = cache.get(a.key.clone()).unwrap().revision;
        assert!(b_revision > restored);
        assert!(a_revision > b_revision);
        assert_eq!(
            texts(&cache.get_schema(a.key.clone()).unwrap(), "a"),
            ["甲", "乙"]
        );
    }

    #[test]
    fn transient_read_error_recovers_without_metadata_changes_and_retry_is_bounded() {
        let fixture = Fixture::new("retry");
        let cache = SourceCache::default();
        assert!(cache
            .get_with(fixture.key.clone(), |_, _| Err("文件暂时被占用".into()))
            .is_err());
        let baseline = fingerprint(&fixture.key).unwrap();
        let (first_retry, first_revision) = {
            let state = cache.state.lock().unwrap();
            (state.retry_at.unwrap(), state.revision)
        };
        cache.refresh_with(first_retry - Duration::from_millis(1), |_, _| {
            panic!("retry ran early")
        });
        assert_eq!(cache.state.lock().unwrap().revision, first_revision);
        cache.refresh_with(first_retry, |_, _| Err("仍被占用".into()));
        let second_retry = cache.state.lock().unwrap().retry_at.unwrap();
        assert_eq!(
            second_retry.duration_since(first_retry),
            Duration::from_secs(2)
        );
        cache.refresh_with(second_retry, |_, _| Err("仍被占用".into()));
        let third_retry = cache.state.lock().unwrap().retry_at.unwrap();
        assert_eq!(
            third_retry.duration_since(second_retry),
            Duration::from_secs(4)
        );
        cache.refresh_with(third_retry, load_snapshot);
        let recovered = cache.get(fixture.key.clone()).unwrap();
        assert_eq!(baseline, fingerprint(&fixture.key).unwrap());
        assert_eq!(texts(&recovered.schema, "a"), ["甲", "乙"]);
        assert!(cache.state.lock().unwrap().retry_at.is_none());
        cache.refresh_with(third_retry + Duration::from_secs(60), |_, _| {
            panic!("healthy source reparsed")
        });
    }

    #[test]
    fn import_dependencies_follow_nested_external_cycles_missing_creation_and_deletion() {
        let fixture = Fixture::new("imports");
        let nested = fixture.key.dir.join("nested");
        let shared = fixture.key.dir.parent().unwrap().join("Shared");
        fs::create_dir_all(&nested).unwrap();
        fs::create_dir_all(&shared).unwrap();
        fixture.write("bundle.yaml", "---\nname: bundle\nimport_tables: [nested/child, ../Shared/shared, missing/new]\n...\n");
        fs::write(
            nested.join("child.dict.yaml"),
            "---\nname: child\nimport_tables: [nested/grand, nested/child]\n...\n内词\tb\n",
        )
        .unwrap();
        fs::write(
            nested.join("grand.dict.yaml"),
            "---\nname: grand\nimport_tables: [../Shared/shared]\n...\n深词\tg\n",
        )
        .unwrap();
        fs::write(
            shared.join("shared.dict.yaml"),
            "---\nname: shared\nimport_tables: [nested/child]\n...\n外词\th\n",
        )
        .unwrap();
        let cache = SourceCache::default();
        let first = cache.get_schema(fixture.key.clone()).unwrap();
        assert_eq!(texts(&first, "b"), ["内词"]);
        assert_eq!(texts(&first, "g"), ["深词"]);
        assert_eq!(texts(&first, "h"), ["外词"]);
        assert!(texts(&first, "m").is_empty());
        assert_eq!(
            cache.state.lock().unwrap().dependencies.len(),
            4,
            "cycles add no unbounded paths"
        );
        fs::write(
            nested.join("grand.dict.yaml"),
            "---\nname: grand\nimport_tables: [../Shared/shared]\n...\n深词已更新\tg\n",
        )
        .unwrap();
        cache.refresh_now(&fixture.key).unwrap();
        assert_eq!(
            texts(&cache.get_schema(fixture.key.clone()).unwrap(), "g"),
            ["深词已更新"]
        );
        fs::remove_file(shared.join("shared.dict.yaml")).unwrap();
        cache.refresh_now(&fixture.key).unwrap();
        assert!(texts(&cache.get_schema(fixture.key.clone()).unwrap(), "h").is_empty());
        fs::create_dir_all(fixture.key.dir.join("missing")).unwrap();
        fixture.write(
            "missing/new.dict.yaml",
            "---\nname: new\n...\n后来创建\tm\n",
        );
        cache.refresh_now(&fixture.key).unwrap();
        assert_eq!(
            texts(&cache.get_schema(fixture.key.clone()).unwrap(), "m"),
            ["后来创建"]
        );
        fs::remove_file(nested.join("child.dict.yaml")).unwrap();
        cache.refresh_now(&fixture.key).unwrap();
        let removed = cache.get_schema(fixture.key.clone()).unwrap();
        assert!(texts(&removed, "b").is_empty());
        assert!(
            texts(&removed, "g").is_empty(),
            "grandchild no longer imported through deleted child"
        );
        fs::write(
            nested.join("child.dict.yaml"),
            "---\nname: child\nimport_tables: [nested/grand]\n...\n重建内词\tb\n",
        )
        .unwrap();
        cache.refresh_now(&fixture.key).unwrap();
        let recreated = cache.get_schema(fixture.key.clone()).unwrap();
        assert_eq!(texts(&recreated, "b"), ["重建内词"]);
        assert_eq!(texts(&recreated, "g"), ["深词已更新"]);
    }

    #[test]
    fn changing_an_import_header_rebuilds_the_watch_graph_and_renames_are_detected() {
        let fixture = Fixture::new("import-rename");
        fs::create_dir_all(fixture.key.dir.join("nested")).unwrap();
        fixture.write(
            "bundle.yml",
            "---\nname: bundle\nimport_tables:\n  - nested/old\n...\n",
        );
        fixture.write("nested/old.dict.yaml", "---\nname: old\n...\n旧词\tb\n");
        let cache = SourceCache::default();
        assert_eq!(
            texts(&cache.get_schema(fixture.key.clone()).unwrap(), "b"),
            ["旧词"]
        );
        fs::rename(
            fixture.key.dir.join("nested/old.dict.yaml"),
            fixture.key.dir.join("nested/renamed.dict.yaml"),
        )
        .unwrap();
        cache.refresh_now(&fixture.key).unwrap();
        assert!(texts(&cache.get_schema(fixture.key.clone()).unwrap(), "b").is_empty());
        fixture.write(
            "bundle.yml",
            "---\nname: bundle\nimport_tables: ['nested/renamed', 'nested/later']\n...\n",
        );
        cache.refresh_now(&fixture.key).unwrap();
        assert_eq!(
            texts(&cache.get_schema(fixture.key.clone()).unwrap(), "b"),
            ["旧词"]
        );
        fixture.write(
            "nested/later.dict.yaml",
            "---\nname: later\n...\n新导入词\tc\n",
        );
        cache.refresh_now(&fixture.key).unwrap();
        assert_eq!(
            texts(&cache.get_schema(fixture.key.clone()).unwrap(), "c"),
            ["新导入词"]
        );
        let state = cache.state.lock().unwrap();
        assert!(!state
            .dependencies
            .contains(&fixture.key.dir.join("nested/old.dict.yaml")));
        assert!(state
            .dependencies
            .contains(&fixture.key.dir.join("nested/later.dict.yaml")));
    }

    #[test]
    fn removing_an_import_recovers_even_if_its_old_metadata_is_unreadable() {
        let fixture = Fixture::new("revoked-import");
        fs::create_dir_all(fixture.key.dir.join("nested")).unwrap();
        fixture.write(
            "bundle.yaml",
            "---\nname: bundle\nimport_tables: [nested/old]\n...\n",
        );
        fixture.write("nested/old.dict.yaml", "---\nname: old\n...\n旧导入词\tb\n");
        let cache = SourceCache::default();
        cache.get_schema(fixture.key.clone()).unwrap();
        fixture.write("bundle.yaml", "---\nname: bundle\n...\n新词\tc\n");
        let (generation, dependencies) = {
            let mut state = cache.state.lock().unwrap();
            state.loading = true;
            (state.generation, state.dependencies.clone())
        };
        // Model the exact observation produced by AccessDenied on an old import;
        // filesystem ACL edits would make this regression platform-dependent.
        cache.load_and_publish(
            &fixture.key,
            generation,
            dependencies,
            Err("旧依赖 metadata: AccessDenied".into()),
            load_snapshot,
            Instant::now(),
        );
        let restored = cache.get_schema(fixture.key.clone()).unwrap();
        assert_eq!(texts(&restored, "c"), ["新词"]);
        assert!(texts(&restored, "b").is_empty());
        assert!(cache.state.lock().unwrap().dependencies.is_empty());
    }
}
