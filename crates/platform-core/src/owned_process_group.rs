//! macOS child observation without releasing its PID/PGID reservation.
use crate::ApiError;

#[cfg(unix)]
pub(super) const TERMINATE: i32 = libc::SIGTERM;
#[cfg(unix)]
pub(super) const KILL: i32 = libc::SIGKILL;
#[cfg(not(unix))]
pub(super) const TERMINATE: i32 = 15;
#[cfg(not(unix))]
pub(super) const KILL: i32 = 9;

#[derive(Clone, Copy, Debug)]
pub(super) struct ExitObservation {
    pub code: Option<i32>,
}

pub(super) struct ProcessGroup {
    pid: u32,
    armed: bool,
}

impl ProcessGroup {
    pub fn new(pid: u32) -> Self {
        Self { pid, armed: true }
    }

    pub fn observe(&mut self) -> Result<Option<ExitObservation>, ApiError> {
        if !self.armed {
            return Err(problem(
                "PROCESS_OWNERSHIP_RELEASED",
                "Child ownership has been released",
            ));
        }
        match observe(self.pid) {
            Ok(value) => Ok(value),
            Err(error) => {
                // ECHILD and other unknown states never authorize a numeric signal.
                self.armed = false;
                Err(error)
            }
        }
    }

    pub fn signal(&mut self, signal: i32) -> Result<(), ApiError> {
        self.observe()?;
        #[cfg(target_os = "macos")]
        {
            if unsafe { libc::kill(-(self.pid as i32), signal) } != 0 {
                let error = std::io::Error::last_os_error().raw_os_error();
                if error != Some(libc::ESRCH)
                    // macOS can reject a signal to an already zombie-only
                    // group. Accept only a fresh, pinned empty-group proof.
                    && !(self.observe()?.is_some() && !self.has_descendants()?)
                {
                    return Err(problem(
                        "PROCESS_SIGNAL_FAILED",
                        "Could not signal the owned process group",
                    )
                    .detail("errno", serde_json::json!(error)));
                }
            }
        }
        let _ = signal;
        Ok(())
    }

    pub fn has_descendants(&mut self) -> Result<bool, ApiError> {
        self.observe()?;
        members(self.pid)
    }

    pub fn disarm_before_reap(&mut self) {
        self.armed = false;
    }
}

fn problem(code: &str, message: &str) -> ApiError {
    ApiError::new(code, message).outcome("unknown")
}

fn observe(pid: u32) -> Result<Option<ExitObservation>, ApiError> {
    #[cfg(target_os = "macos")]
    loop {
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        let result = unsafe {
            libc::waitid(
                libc::P_PID,
                pid as libc::id_t,
                &mut info,
                libc::WEXITED | libc::WNOWAIT | libc::WNOHANG,
            )
        };
        if result == 0 {
            return if unsafe { info.si_pid() } == 0 {
                Ok(None)
            } else {
                Ok(Some(ExitObservation {
                    code: if info.si_code == libc::CLD_EXITED {
                        Some(unsafe { info.si_status() })
                    } else {
                        None
                    },
                }))
            };
        }
        if std::io::Error::last_os_error().raw_os_error() != Some(libc::EINTR) {
            return Err(problem(
                "PROCESS_OWNERSHIP_LOST",
                "Could not prove unreaped child ownership",
            ));
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = pid;
        Err(problem(
            "PROCESS_PLATFORM_UNSUPPORTED",
            "Owned group verification is only implemented for macOS",
        ))
    }
}

pub(super) fn members(pid: u32) -> Result<bool, ApiError> {
    #[cfg(target_os = "macos")]
    {
        const PROC_PGRP_ONLY: u32 = 2;
        let mut previous: Option<Vec<(i32, u64, u64)>> = None;
        for _ in 0..2 {
            let mut pids = [0i32; 4096];
            let bytes = std::mem::size_of_val(&pids);
            let count = unsafe {
                libc::proc_listpids(PROC_PGRP_ONLY, pid, pids.as_mut_ptr().cast(), bytes as i32)
            };
            validate_count(count, bytes)?;
            let mut identities = vec![];
            if scan_members(
                pid,
                &pids[..count as usize / std::mem::size_of::<i32>()],
                |member| {
                    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
                    let size = std::mem::size_of_val(&info);
                    let received = unsafe {
                        libc::proc_pidinfo(
                            member,
                            libc::PROC_PIDTBSDINFO,
                            // Nonzero arg asks libproc to include zombies.
                            // With arg=0 a retained zombie returns ESRCH, which
                            // cannot serve as either live or closed evidence.
                            1,
                            (&mut info as *mut libc::proc_bsdinfo).cast(),
                            size as i32,
                        )
                    };
                    if received == size as i32 {
                        // Reused/disappearing members never prove an empty group.
                        Ok(if info.pbi_pid != member as u32 || info.pbi_pgid != pid {
                            Member::Changed
                        } else if info.pbi_status == libc::SZOMB {
                            identities.push((member, info.pbi_start_tvsec, info.pbi_start_tvusec));
                            Member::Zombie
                        } else {
                            Member::Live
                        })
                    } else if received == 0
                        || std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
                    {
                        Ok(Member::Changed)
                    } else {
                        Err(problem(
                            "PROCESS_GROUP_UNCONFIRMED",
                            "Could not verify an owned group member",
                        ))
                    }
                },
            )? {
                return Ok(true);
            }
            identities.sort_unstable();
            if let Some(previous) = previous {
                // Even a newly observed zombie could just have forked a live
                // grandchild after enumeration. Require two complete stable
                // identity sets; a changed scan continues the owned drain.
                return Ok(!stable_zombies(&previous, &identities));
            }
            previous = Some(identities);
        }
        unreachable!("two scans always return a proof or continue")
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = pid;
        Err(problem(
            "PROCESS_PLATFORM_UNSUPPORTED",
            "Owned group verification is only implemented for macOS",
        ))
    }
}

pub(super) enum Member {
    Zombie,
    Live,
    Changed,
}
pub(super) fn stable_zombies(first: &[(i32, u64, u64)], second: &[(i32, u64, u64)]) -> bool {
    first == second
}
pub(super) fn validate_count(count: i32, bytes: usize) -> Result<(), ApiError> {
    // libproc maps syscall errors to zero. The pinned leader itself must still
    // be listed, so zero can never be an empty-group receipt.
    if count <= 0
        || count as usize >= bytes
        || !(count as usize).is_multiple_of(std::mem::size_of::<i32>())
    {
        return Err(problem(
            "PROCESS_GROUP_UNCONFIRMED",
            "Could not completely inspect the owned group",
        ));
    }
    Ok(())
}
pub(super) fn scan_members(
    pid: u32,
    pids: &[i32],
    mut inspect: impl FnMut(i32) -> Result<Member, ApiError>,
) -> Result<bool, ApiError> {
    let unique = pids
        .iter()
        .copied()
        .collect::<std::collections::BTreeSet<_>>();
    if pids.iter().any(|member| *member <= 0)
        || unique.len() != pids.len()
        || !unique.contains(&(pid as i32))
    {
        return Err(problem(
            "PROCESS_GROUP_UNCONFIRMED",
            "Group list does not contain a unique pinned leader and valid members",
        ));
    }
    for member in pids {
        if *member != pid as i32 && !matches!(inspect(*member)?, Member::Zombie) {
            return Ok(true);
        }
    }
    Ok(false)
}
