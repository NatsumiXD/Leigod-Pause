//! Bounded, read-only LinkInfo parsing. Never launches shortcuts or arguments.
//! Format: https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-shllink/6813269d-0cc8-4be2-933f-e96e8e3412dc
use std::{env, path::PathBuf};
fn target(bytes: &[u8]) -> Option<PathBuf> {
    fn dword(bytes: &[u8], offset: usize) -> Option<usize> {
        Some(
            u32::from_le_bytes(bytes.get(offset..offset.checked_add(4)?)?.try_into().ok()?)
                as usize,
        )
    }
    fn string(bytes: &[u8], offset: usize, unicode: bool) -> Option<String> {
        if offset == 0 {
            return None;
        }
        let tail = bytes.get(offset..)?;
        if unicode {
            let mut units = Vec::new();
            for pair in tail.as_chunks::<2>().0 {
                let unit = u16::from_le_bytes([pair[0], pair[1]]);
                if unit == 0 {
                    return String::from_utf16(&units).ok();
                }
                units.push(unit);
            }
            None
        } else {
            let end = tail.iter().position(|v| *v == 0)?;
            let text = &tail[..end];
            text.is_ascii()
                .then(|| String::from_utf8_lossy(text).into_owned())
        }
    }
    if bytes.len() > 1024 * 1024
        || dword(bytes, 0)? != 0x4c
        || bytes.get(4..20)? != [1, 0x14, 2, 0, 0, 0, 0, 0, 0xc0, 0, 0, 0, 0, 0, 0, 0x46]
    {
        return None;
    }
    let flags = dword(bytes, 20)?;
    if flags & 2 == 0 || flags & 0x100 != 0 {
        return None;
    }
    let mut position = 0x4cusize;
    if flags & 1 != 0 {
        let size = u16::from_le_bytes(bytes.get(position..position + 2)?.try_into().ok()?) as usize;
        position = position.checked_add(size + 2)?;
    }
    let size = dword(bytes, position)?;
    let info = bytes.get(position..position.checked_add(size)?)?;
    let header = dword(info, 4)?;
    if header < 0x1c || header > info.len() || dword(info, 8)? & 1 == 0 {
        return None;
    }
    let unicode = header >= 0x24 && dword(info, 28)? != 0;
    let (base, suffix) = if unicode {
        (dword(info, 28)?, dword(info, 32)?)
    } else {
        (dword(info, 16)?, dword(info, 24)?)
    };
    let base = string(info, base, unicode)?;
    let suffix = if suffix == 0 {
        String::new()
    } else {
        string(info, suffix, unicode)?
    };
    if base.to_ascii_lowercase().ends_with(".exe") || suffix.is_empty() {
        Some(PathBuf::from(base))
    } else {
        Some(PathBuf::from(base).join(suffix.trim_start_matches(['\\', '/'])))
    }
}

pub fn candidates() -> Vec<PathBuf> {
    let mut pending = Vec::new();
    for (variable, suffix) in [
        ("APPDATA", r"Microsoft\Windows\Start Menu\Programs"),
        ("PROGRAMDATA", r"Microsoft\Windows\Start Menu\Programs"),
        ("USERPROFILE", "Desktop"),
        ("PUBLIC", "Desktop"),
    ] {
        if let Some(root) = env::var_os(variable) {
            pending.push((PathBuf::from(root).join(suffix), 0));
        }
    }
    let mut targets = Vec::new();
    let mut visited = 0;
    while let Some((folder, depth)) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(folder) else {
            continue;
        };
        for entry in entries.flatten() {
            visited += 1;
            if visited > 8192 {
                return targets;
            }
            let path = entry.path();
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() && depth < 3 {
                pending.push((path, depth + 1));
                continue;
            }
            let name = path.to_string_lossy();
            if !kind.is_file()
                || path
                    .extension()
                    .is_none_or(|ext| !ext.eq_ignore_ascii_case("lnk"))
                || (!name.contains("雷神") && !name.to_ascii_lowercase().contains("leigod"))
            {
                continue;
            }
            if entry.metadata().is_ok_and(|meta| meta.len() <= 1024 * 1024)
                && let Ok(bytes) = std::fs::read(path)
                && let Some(target) = target(&bytes)
            {
                targets.push(target);
            }
        }
    }
    targets
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unicode_install_paths_are_preserved_and_invalid_offsets_rejected() {
        let path = r"D:\游戏工具\雷神\leigod_launcher.exe";
        let mut info = vec![0u8; 0x24];
        info[4..8].copy_from_slice(&0x24u32.to_le_bytes());
        info[8..12].copy_from_slice(&1u32.to_le_bytes());
        info[28..32].copy_from_slice(&0x24u32.to_le_bytes());
        for unit in path.encode_utf16().chain(Some(0)) {
            info.extend_from_slice(&unit.to_le_bytes());
        }
        let size = info.len() as u32;
        info[..4].copy_from_slice(&size.to_le_bytes());
        let mut bytes = vec![0u8; 0x4c];
        bytes[..4].copy_from_slice(&0x4cu32.to_le_bytes());
        bytes[4..20].copy_from_slice(&[1, 0x14, 2, 0, 0, 0, 0, 0, 0xc0, 0, 0, 0, 0, 0, 0, 0x46]);
        bytes[20..24].copy_from_slice(&2u32.to_le_bytes());
        bytes.extend(info);
        assert_eq!(target(&bytes), Some(PathBuf::from(path)));
        bytes[0x4c..0x50].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(target(&bytes).is_none());
        assert!(target(&[0; 8]).is_none());
    }
}
