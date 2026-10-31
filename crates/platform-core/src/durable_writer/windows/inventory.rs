//! Missing/empty is a physical observation. Any marker, including preparation
//! or a well-formed former live marker, remains unknown after a crash.
use super::{
    io::{self, blocked, Directory, Result},
    lexical_path, Kind, WriterOwner,
};
use std::{
    ffi::OsString,
    fs,
    os::windows::fs::MetadataExt,
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Observation {
    Missing,
    Empty,
}

#[derive(Default)]
struct Inventory {
    bound: Vec<Directory>,
    absent: Vec<PathBuf>,
    empty: Vec<(usize, u64)>,
    private: Vec<usize>,
    listings: Vec<(usize, Vec<OsString>, u64)>,
}

impl Inventory {
    fn walk(
        &mut self,
        path: &Path,
        observe: &mut impl FnMut(Observation, &Path) -> Result<()>,
    ) -> Result<Option<usize>> {
        // Resolve actual native components, including genuine absence of the
        // data root itself. Never turn an arbitrary I/O error into Missing.
        let mut last = None;
        for component in platform_native::filesystem::path_components(&lexical_path(path)) {
            match fs::symlink_metadata(&component) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    if last.is_none() {
                        return Err(blocked(&component, error));
                    }
                    observe(Observation::Missing, &component)?;
                    io::missing(&component)?;
                    self.absent.push(component);
                    return Ok(None);
                }
                Err(error) => return Err(blocked(&component, error)),
                Ok(_) => (),
            }
            let selected = Directory::open(&component)?;
            self.bound.push(selected);
            last = Some(self.bound.len() - 1);
        }
        last.ok_or_else(super::invalid_owner).map(Some)
    }

    fn require_private(&mut self, index: usize) -> Result<()> {
        self.bound[index].verify(true)?;
        self.private.push(index);
        Ok(())
    }

    fn owner(
        &mut self,
        owner: &WriterOwner,
        observe: &mut impl FnMut(Observation, &Path) -> Result<()>,
    ) -> Result<()> {
        let path = io::owner_path(owner).join("writers");
        let Some(index) = self.walk(&path, observe)? else {
            return Ok(());
        };
        // Mac checks only the existing selected writers directory here. A
        // genuinely absent leaf adds no owner-record or parent-ACL admission.
        self.require_private(index)?;
        let directory = &self.bound[index];
        let before = directory
            .pin
            .file
            .metadata()
            .map_err(|error| blocked(&directory.path, error))?
            .last_write_time();
        require_empty(&directory.path)?;
        observe(Observation::Empty, &directory.path)?;
        self.empty.push((index, before));
        Ok(())
    }

    fn finish(&self) -> Result<()> {
        for path in &self.absent {
            io::missing(path)?;
        }
        for (index, before) in &self.empty {
            let directory = &self.bound[*index];
            require_empty(&directory.path)?;
            if directory
                .pin
                .file
                .metadata()
                .map_err(|error| blocked(&directory.path, error))?
                .last_write_time()
                != *before
            {
                return Err(blocked(
                    &directory.path,
                    "Writer directory changed during inventory",
                ));
            }
        }
        for (index, before, modified) in &self.listings {
            let directory = &self.bound[*index];
            if names(&directory.path)? != *before
                || directory
                    .pin
                    .file
                    .metadata()
                    .map_err(|error| blocked(&directory.path, error))?
                    .last_write_time()
                    != *modified
            {
                return Err(blocked(
                    &directory.path,
                    "Owner directory list changed during inventory",
                ));
            }
        }
        for directory in &self.bound {
            directory.verify(false)?;
        }
        for index in &self.private {
            self.bound[*index].verify(true)?;
        }
        Ok(())
    }
}

fn require_empty(path: &Path) -> Result<()> {
    match fs::read_dir(path)
        .map_err(|error| blocked(path, error))?
        .next()
    {
        None => Ok(()),
        Some(Ok(_)) => Err(blocked(
            path,
            "Retained writer material has no recovered closure proof",
        )),
        Some(Err(error)) => Err(blocked(path, error)),
    }
}

fn names(path: &Path) -> Result<Vec<OsString>> {
    let mut names = fs::read_dir(path)
        .map_err(|error| blocked(path, error))?
        .map(|entry| {
            entry
                .map(|entry| entry.file_name())
                .map_err(|error| blocked(path, error))
        })
        .collect::<Result<Vec<_>>>()?;
    names.sort();
    Ok(names)
}

fn owner_with(
    owner: &WriterOwner,
    mut observe: impl FnMut(Observation, &Path) -> Result<()>,
) -> Result<()> {
    let mut inventory = Inventory::default();
    inventory.owner(owner, &mut observe)?;
    inventory.finish()
}

pub(super) fn require_owner_clear(owner: &WriterOwner) -> Result<()> {
    owner_with(owner, |_, _| Ok(()))
}

fn all_with(data: &Path, mut observe: impl FnMut(Observation, &Path) -> Result<()>) -> Result<()> {
    let mut inventory = Inventory::default();
    inventory.owner(&WriterOwner::platform(data)?, &mut observe)?;
    for (name, kind) in [
        ("node-project-ownership", Kind::Node),
        ("web-project-ownership", Kind::Web),
    ] {
        let path = lexical_path(data).join(name);
        let Some(index) = inventory.walk(&path, &mut observe)? else {
            continue;
        };
        inventory.require_private(index)?;
        let directory = &inventory.bound[index];
        let path = directory.path.clone();
        let before = names(&path)?;
        let modified = directory
            .pin
            .file
            .metadata()
            .map_err(|error| blocked(&path, error))?
            .last_write_time();
        for name in &before {
            let id = name
                .to_str()
                .ok_or_else(|| blocked(&path, "Owner ID is not UTF-8"))?;
            if id == ".pending-deletions" {
                continue;
            }
            let owner = WriterOwner::new(data, kind, id)?;
            inventory.owner(&owner, &mut observe)?;
        }
        inventory.listings.push((index, before, modified));
    }
    inventory.finish()
}

pub(super) fn require_all_clear(data: &Path) -> Result<()> {
    all_with(data, |_, _| Ok(()))
}

#[cfg(test)]
#[path = "inventory_tests.rs"]
mod tests;
