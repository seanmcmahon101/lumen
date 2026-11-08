//! Small user preferences are written serially off the UI thread.
use serde::{Serialize, de::DeserializeOwned};
use std::path::PathBuf;
use std::sync::{OnceLock, mpsc};

enum Write {
    Data(&'static str, Vec<u8>),
    Flush(mpsc::Sender<()>),
}
static WRITER: OnceLock<mpsc::Sender<Write>> = OnceLock::new();

pub fn directory() -> Option<PathBuf> {
    std::env::var_os("LUMEN_DATA_DIR")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("APPDATA").map(|p| PathBuf::from(p).join("Lumen")))
}

pub fn load<T: DeserializeOwned + Default>(name: &str) -> T {
    directory()
        .and_then(|p| std::fs::read(p.join(name)).ok())
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

pub fn save(name: &'static str, value: &impl Serialize) {
    let writer = WRITER.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<Write>();
        std::thread::spawn(move || {
            while let Ok(request) = rx.recv() {
                match request {
                    Write::Data(name, bytes) => {
                        if let Some(dir) = directory() {
                            let _ = atomic_write(&dir.join(name), &bytes);
                        }
                    }
                    Write::Flush(done) => {
                        let _ = done.send(());
                    }
                }
            }
        });
        tx
    });
    if let Ok(bytes) = serde_json::to_vec(value) {
        let _ = writer.send(Write::Data(name, bytes));
    }
}

pub fn flush() {
    if let Some(writer) = WRITER.get() {
        let (tx, rx) = mpsc::channel();
        let _ = writer.send(Write::Flush(tx));
        let _ = rx.recv_timeout(std::time::Duration::from_secs(2));
    }
}

pub fn atomic_write(file: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = file.with_extension(format!("tmp-{}", std::process::id()));
    let mut output = std::fs::File::create(&temporary)?;
    output.write_all(bytes)?;
    output.sync_all()?;
    drop(output);
    let result = std::fs::rename(&temporary, file);
    if result.is_err() {
        let _ = std::fs::remove_file(temporary);
    }
    result
}
