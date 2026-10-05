use crate::config::atomic_write;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Usage {
    pub count: u64,
    pub last_opened: u64,
}

#[derive(Default)]
pub struct History {
    pub entries: BTreeMap<String, Usage>,
    path: PathBuf,
}

impl History {
    pub fn empty(path: PathBuf) -> Self {
        Self {
            entries: BTreeMap::new(),
            path,
        }
    }

    pub fn load(path: PathBuf) -> Result<Self> {
        let entries = match fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
            Err(error) => return Err(error.into()),
        };
        Ok(Self { entries, path })
    }

    pub fn record(&mut self, id: &str) -> Result<()> {
        let usage = self.entries.entry(id.into()).or_default();
        usage.count = usage.count.saturating_add(1);
        usage.last_opened = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
        atomic_write(&self.path, &serde_json::to_vec_pretty(&self.entries)?)
    }
}
