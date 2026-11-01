//! Loopback listeners with platform-specific address ownership policy.
use socket2::{Domain, Protocol, Socket, Type};
use std::{
    io,
    net::{Ipv4Addr, SocketAddrV4, TcpListener},
};

pub fn bind_loopback(port: u16) -> io::Result<TcpListener> {
    let socket = Socket::new(Domain::IPV4, Type::STREAM, Some(Protocol::TCP))?;
    #[cfg(unix)]
    socket.set_reuse_address(true)?;
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawSocket;
        use windows_sys::Win32::Networking::WinSock::{
            setsockopt, WSAGetLastError, SOL_SOCKET, SO_EXCLUSIVEADDRUSE,
        };
        // Set before bind; Windows SO_REUSEADDR could take over an active peer.
        // https://learn.microsoft.com/windows/win32/winsock/using-so-reuseaddr-and-so-exclusiveaddruse
        socket.set_reuse_address(false)?;
        let enabled: i32 = 1;
        if unsafe {
            setsockopt(
                socket.as_raw_socket() as _,
                SOL_SOCKET,
                SO_EXCLUSIVEADDRUSE,
                (&enabled as *const i32).cast(),
                std::mem::size_of_val(&enabled) as i32,
            )
        } != 0
        {
            return Err(io::Error::from_raw_os_error(unsafe { WSAGetLastError() }));
        }
    }
    socket.bind(&SocketAddrV4::new(Ipv4Addr::LOCALHOST, port).into())?;
    socket.listen(128)?;
    let listener: TcpListener = socket.into();
    listener.set_nonblocking(true)?;
    Ok(listener)
}
