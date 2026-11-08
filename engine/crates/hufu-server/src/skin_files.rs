//! Skin edit serialization is independent of the input engine mutex. Take this
//! lock before Host so a long file edit never waits for it while blocking keys.
use std::{io::{self, Write}, path::Path, sync::{Mutex, MutexGuard}};

static EDITS: Mutex<()> = Mutex::new(());

pub fn edit_lock() -> MutexGuard<'static, ()> {
    EDITS.lock().unwrap_or_else(|p| p.into_inner())
}

pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let id = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let name = path.file_name().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "missing skin filename"))?;
    let mut temporary = name.to_os_string();
    temporary.push(format!(".hufu-edit-{}-{id}.tmp", std::process::id()));
    let temporary = path.with_file_name(temporary);
    let result = (|| {
        let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(&temporary)?;
        file.write_all(bytes)?;
        file.flush()?;
        drop(file);
        // Same-directory rename replaces the completed file atomically. Readers
        // can observe old or new JSON, never the middle of a truncate/write.
        std::fs::rename(&temporary, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

#[cfg(test)]
mod tests {
    #[test]
    fn atomic_edit_replaces_existing_file_without_leaving_temporaries() {
        let dir = std::env::temp_dir().join(format!("hufu-skin-atomic-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("皮肤.json");
        std::fs::write(&path, b"old").unwrap();
        super::write_atomic(&path, b"new complete JSON").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"new complete JSON");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
