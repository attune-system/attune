use anyhow::{bail, ensure, Result};
use icu_casemap::CaseMapper;
use icu_normalizer_v1::ComposingNormalizer;
use std::collections::{BTreeMap, BTreeSet};

pub(super) fn validate(path: &str) -> Result<()> {
    ensure!(
        !path.is_empty() && path.len() <= 255,
        "invalid path length: {path:?}"
    );
    ensure!(
        ComposingNormalizer::new_nfc().is_normalized(path),
        "path is not Unicode 15.1 NFC: {path:?}"
    );
    ensure!(
        !path
            .chars()
            .any(|c| c.is_control() || "\\:;*?[]".contains(c)),
        "forbidden path character: {path:?}"
    );
    ensure!(
        path.split('/').all(|c| !matches!(c, "" | "." | "..")),
        "unsafe path: {path:?}"
    );
    split_ustar(path)?;
    Ok(())
}

pub(super) fn split_ustar(path: &str) -> Result<(&str, &str)> {
    if path.len() <= 100 {
        return Ok(("", path));
    }
    for (index, _) in path.rmatch_indices('/') {
        if index <= 155 && path.len() - index - 1 <= 100 {
            return Ok((&path[..index], &path[index + 1..]));
        }
    }
    bail!("path does not fit ustar: {path:?}")
}

#[derive(Default)]
pub(super) struct Paths {
    files: BTreeSet<String>,
    directories: BTreeMap<String, String>,
}

impl Paths {
    pub fn insert(&mut self, path: &str) -> Result<()> {
        validate(path)?;
        let fold =
            |s: &str| ComposingNormalizer::new_nfc().normalize(&CaseMapper::new().fold_string(s));
        let key = fold(path);
        ensure!(
            !self.directories.contains_key(&key) && self.files.insert(key),
            "duplicate or colliding path: {path:?}"
        );
        for (index, _) in path.match_indices('/') {
            let parent = &path[..index];
            let key = fold(parent);
            ensure!(
                !self.files.contains(&key),
                "file/directory collision: {path:?}"
            );
            if let Some(previous) = self.directories.insert(key, parent.to_owned()) {
                ensure!(
                    previous == parent,
                    "case-folding directory collision: {path:?}"
                );
            }
        }
        Ok(())
    }
}

pub(super) fn id(value: &str) -> Result<()> {
    ensure!(
        (1..=128).contains(&value.len())
            && value.as_bytes()[0].is_ascii_lowercase()
            && value
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._-".contains(&b)),
        "invalid artifact/variant ID: {value:?}"
    );
    Ok(())
}
