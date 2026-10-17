use serde_json::Value;
use std::{
    fs::{self, File},
    io::{self, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
pub struct History {
    json: File,
    text: File,
    pub path: PathBuf,
}
pub fn epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
impl History {
    pub fn create(root: &Path) -> io::Result<Self> {
        let dir = root.join("logs");
        fs::create_dir_all(&dir)?;
        let stem = format!("session-{}-{}", epoch_ms(), std::process::id());
        let path = dir.join(format!("{stem}.log"));
        Ok(Self {
            json: File::create(dir.join(format!("{stem}.ndjson")))?,
            text: File::create(&path)?,
            path,
        })
    }
    pub fn push(&mut self, value: &Value) -> io::Result<String> {
        let block = format!("[{}]\n{}\n", epoch_ms(), crate::report::text(value));
        writeln!(self.json, "{value}")?;
        self.json.flush()?;
        self.text.write_all(block.as_bytes())?;
        self.text.flush()?;
        Ok(block)
    }
    pub fn read_all(&self) -> io::Result<Vec<String>> {
        let mut files = fs::read_dir(
            self.path
                .parent()
                .ok_or_else(|| io::Error::other("Missing log directory"))?,
        )?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| {
            p.extension().is_some_and(|e| e == "log")
                && p.file_name()
                    .is_some_and(|n| n.to_string_lossy().starts_with("session-"))
        })
        .collect::<Vec<_>>();
        files.sort();
        files.into_iter().map(fs::read_to_string).collect()
    }
    pub fn export(&self) -> io::Result<PathBuf> {
        let destination =
            self.path
                .with_file_name(format!("export-{}-{}.log", epoch_ms(), std::process::id()));
        let mut file = File::create(&destination)?;
        for block in self.read_all()? {
            file.write_all(block.as_bytes())?;
        }
        file.flush()?;
        Ok(destination)
    }
}
