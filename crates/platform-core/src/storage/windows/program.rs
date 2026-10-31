//! Counts the selected members, never an executable's Cargo/installation parent.
use super::*;
use crate::installation_scope::{InstallationScope, Stamp};

pub(super) fn scan(scope: &InstallationScope, other_roots: &[PathBuf]) -> Value {
    scan_with(scope, other_roots, || {})
}

fn scan_with(
    scope: &InstallationScope,
    other_roots: &[PathBuf],
    between_passes: impl FnOnce(),
) -> Value {
    let started = now();
    let members = scope.members();
    let mut total = Counter::default();
    let mut counts = BTreeMap::new();
    let mut issues = Vec::new();
    let mut before = Vec::new();
    let mut known = false;
    let mut selected_files = Vec::<Stamp>::new();
    // The root's identity is fixed at declaration; contents may change and are
    // freshly measured. Executables must retain their actual startup stamps.
    for member in members {
        match member.observe() {
            Ok(stamp) => {
                known = true;
                if !member.directory {
                    let mut count = Counter::default();
                    if !selected_files.iter().any(|old| old.path == stamp.path) {
                        count.add(stamp.bytes);
                        total.add(stamp.bytes);
                        selected_files.push(stamp.clone());
                    }
                    counts.insert(member.kind.to_owned(), count.value());
                }
                before.push(Some(stamp));
            }
            Err(error) => {
                issues.push(problem(
                    &member.path,
                    "installation_member_unavailable_or_changed",
                    error,
                ));
                before.push(None);
            }
        }
    }
    let resources = &members[2];
    let exclusions = members[..2]
        .iter()
        .map(|member| &member.path)
        .chain(other_roots.iter().filter(|path| *path != &resources.path))
        .filter_map(|path| {
            path.strip_prefix(&resources.path)
                .ok()
                .map(Path::to_path_buf)
        })
        .collect::<Vec<_>>();
    let mut first = if before[2].is_some() {
        let mut count = Counter::default();
        let scan = walk(&resources.path, &exclusions, |entry| {
            count.add(entry.bytes);
            total.add(entry.bytes);
        });
        counts.insert(resources.kind.to_owned(), count.value());
        scan
    } else {
        Walk::new()
    };
    between_passes();
    let second = if before[2].is_some() {
        walk(&resources.path, &exclusions, |_| {})
    } else {
        Walk::new()
    };
    if first.signature() != second.signature() {
        first.issues.push(problem(
            &resources.path,
            "changed_during_scan",
            "Installation resources changed between metadata passes",
        ));
    }
    let mut after = Vec::new();
    for (member, old) in members.iter().zip(&before) {
        let current = member.observe().ok();
        if &current != old {
            issues.push(problem(
                &member.path,
                "installation_member_changed_during_scan",
                "Original member identity or metadata changed between passes",
            ));
        }
        after.push(current);
    }
    let signature = first.signature();
    issues.append(&mut first.issues);
    issues.extend(second.issues);
    for issue in &mut issues {
        if let Some(path) = issue["path"].as_str() {
            if !Path::new(path).is_absolute() {
                issue["path"] = json!(resources.path.join(path));
            }
        }
    }
    let declared = members
        .iter()
        .map(|member| {
            json!({
                "kind":member.kind,"path":member.path,"directory":member.directory,
                "original":member.original,"bindingIssue":member.binding_error,
            })
        })
        .collect::<Vec<_>>();
    json!({
        "scope":{"kind":"platformProgram"},"root":null,"roots":members.iter().map(|member| &member.path).collect::<Vec<_>>(),
        "scanId":Uuid::new_v4().to_string(),"startedAt":started,"completedAt":now(),
        "complete":issues.is_empty(),"stale":false,"totalBytes":if known {int(total.bytes)} else {Value::Null},
        "fileCount":if known {int(total.files)} else {Value::Null},"categories":counts,"issues":issues,
        "excludedRoots":exclusions,"measurement":"logicalFileBytes","nonAtomic":true,
        "installationBinding":{"kind":"selectedWindowsMembers","members":declared,"observedBefore":before,"observedAfter":after,"resourcesSignature":signature,"version":crate::APP_VERSION,"target":crate::build_profile::BUILD_TARGET},
    })
}

#[cfg(test)]
#[path = "program/tests.rs"]
mod tests;
