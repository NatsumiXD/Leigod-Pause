use serde::{Deserialize, Serialize};
use std::{fs, io, path::Path};
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Settings {
    pub exit_delay_minutes: u64,
    pub auto_stop_enabled: bool,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            exit_delay_minutes: 10,
            auto_stop_enabled: true,
        }
    }
}
impl Settings {
    pub fn load(path: &Path) -> io::Result<Self> {
        match fs::read(path) {
            Ok(bytes) => {
                let value: Self = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
                value.validate()?;
                Ok(value)
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e),
        }
    }
    pub fn validate(&self) -> io::Result<()> {
        if self.exit_delay_minutes > 1440 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "等待分钟必须在 0..1440 之间",
            ));
        }
        Ok(())
    }
    pub fn save(&self, path: &Path) -> io::Result<()> {
        self.validate()?;
        let pending = path.with_extension("json.tmp");
        fs::write(
            &pending,
            serde_json::to_vec_pretty(self).map_err(io::Error::other)?,
        )?;
        fs::rename(pending, path)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn validation_rejects_excessive_timeout() {
        assert_eq!(Settings::default().exit_delay_minutes, 10);
        assert!(
            Settings {
                exit_delay_minutes: 1441,
                ..Default::default()
            }
            .validate()
            .is_err()
        );
        assert!(
            serde_json::from_str::<Settings>(r#"{"exit_delay_minutes":2}"#)
                .unwrap()
                .auto_stop_enabled
        );
    }
}
