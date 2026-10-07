//! Discovery and native uninstall operations. The UI must confirm a Target
//! before calling execute; execute revalidates that exact target.
use crate::{
    catalog::{Application, Catalog},
    config::Config,
    search::{self, Candidate, Kind},
};
use anyhow::Result;
use std::path::PathBuf;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "macos")]
use macos as native;
#[cfg(target_os = "windows")]
use windows as native;

#[derive(Clone, Debug)]
pub struct Target {
    pub id: String,
    pub name: String,
    pub path: PathBuf,
    pub aliases: Vec<String>,
    action: native::Action,
}

impl Target {
    pub fn candidate(&self, config: &Config) -> Candidate {
        Candidate {
            id: self.id.clone(),
            title: self.name.clone(),
            detail: self.path.display().to_string(),
            path: self.path.clone(),
            kind: Kind::Uninstall,
            aliases: search::app_aliases(
                &Application {
                    id: self.id.clone(),
                    name: self.name.clone(),
                    path: self.path.clone(),
                    aliases: self.aliases.clone(),
                },
                config,
            ),
        }
    }

    pub fn description(&self) -> String {
        native::description(self)
    }

    pub fn execute(&self) -> Result<String> {
        native::execute(self)
    }
}

pub fn discover(catalog: &Catalog) -> Vec<Target> {
    native::discover(catalog)
}
