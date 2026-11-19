//! Real parent/helper process fixture. Its receipt stands in for the private
//! desktop receipt, whose construction and coordinator join are tested separately.
use platform_native::windows::files;
use platform_update_handoff::{
    observe_parent_exit, DrainProof, ParentEndpoint, SessionBinding, SessionDirectory,
};
use std::{
    io::{self, Read, Write},
    path::PathBuf,
    time::Duration,
};

struct FixtureReceipt(SessionBinding);
impl DrainProof for FixtureReceipt {
    fn binding(&self) -> &SessionBinding {
        &self.0
    }
}

pub fn run() -> io::Result<()> {
    let mut args = std::env::args_os().skip(1);
    let mode = args.next().ok_or(io::ErrorKind::InvalidInput)?;
    if mode == "--platform-update-helper" {
        // Cancellation closes the pipe. Only an actual parent exit produces this receipt.
        let observed = observe_parent_exit()?;
        if observed.plan() != b"confirmed by fixture" {
            return Err(io::ErrorKind::InvalidData.into());
        }
        let (dir, _pin) = files::directory_scope(observed.directory())?;
        files::create_private_file(&dir.join("observed"))?.write_all(b"exited")?;
        return Ok(());
    }
    let root = PathBuf::from(args.next().ok_or(io::ErrorKind::InvalidInput)?);
    let session = SessionDirectory::create_in(&files::directory_file(&root)?)?;
    let name = session.path().file_name().unwrap().to_owned();
    let receipt = FixtureReceipt(session.binding().clone());
    let mut endpoint =
        ParentEndpoint::<FixtureReceipt>::launch_with_plan(session, b"confirmed by fixture")?;
    endpoint.wait_registered(Duration::from_secs(10))?;
    if mode == "commit" {
        endpoint.commit(receipt, Duration::from_secs(5))?;
    } else if mode == "cancel" {
        drop(endpoint);
    } else {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    println!("{}", name.to_str().unwrap());
    io::stdout().flush()?;
    io::stdin().read_exact(&mut [0])?;
    Ok(())
}
