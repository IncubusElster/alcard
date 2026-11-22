use super::*;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    mpsc,
};

static UNIQUE: AtomicU64 = AtomicU64::new(0);

fn test_address() -> SocketAddr {
    let id = UNIQUE.fetch_add(1, Ordering::Relaxed);
    SocketAddr::from_abstract_name(format!("alcard-shell-test-{}-{id}", std::process::id()))
        .unwrap()
}

#[test]
fn home_address_is_per_user_and_per_display() {
    let base = Path::new("/run/user/1000");
    let a = name(1000, base, "wayland-0");
    assert_eq!(a, name(1000, base, "wayland-0"));
    assert_ne!(a, name(1000, base, "wayland-1"));
    assert_ne!(a, name(1001, base, "wayland-0"));
    assert_ne!(
        a,
        name(1000, Path::new("/run/user/1000-other"), "wayland-0")
    );
    assert!(a.len() < 107);
}

#[test]
fn abstract_socket_is_single_owner_and_has_no_stale_file() {
    let addr = test_address();
    let owner = UnixListener::bind_addr(&addr).unwrap();
    assert_eq!(
        UnixListener::bind_addr(&addr).err().unwrap().kind(),
        io::ErrorKind::AddrInUse
    );
    drop(owner);
    let next = UnixListener::bind_addr(&addr).unwrap();
    drop(next);
}

#[test]
fn local_home_request_is_delivered_exactly_once() {
    let address = test_address();
    let listener = UnixListener::bind_addr(&address).unwrap();
    let uid = Uid::current().as_raw();
    let (tx, rx) = mpsc::channel();
    let worker = thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        handle_request(stream, uid, || tx.send(()).map_err(io::Error::other)).unwrap();
    });
    exchange(UnixStream::connect_addr(&address).unwrap()).unwrap();
    rx.recv_timeout(IO_TIMEOUT).unwrap();
    worker.join().unwrap();
    assert!(rx.try_recv().is_err());
}

#[test]
fn repeated_home_requests_can_reach_the_same_running_shell() {
    let addr = test_address();
    let listener = UnixListener::bind_addr(&addr).unwrap();
    let uid = Uid::current().as_raw();
    let (tx, rx) = mpsc::channel();
    let worker = thread::spawn(move || {
        for _ in 0..3 {
            let (stream, _) = listener.accept().unwrap();
            handle_request(stream, uid, || tx.send(()).map_err(io::Error::other)).unwrap();
        }
    });
    for _ in 0..3 {
        exchange(UnixStream::connect_addr(&addr).unwrap()).unwrap();
    }
    worker.join().unwrap();
    assert_eq!(rx.try_iter().count(), 3);
}

#[test]
fn unknown_command_is_not_dispatched() {
    let address = test_address();
    let listener = UnixListener::bind_addr(&address).unwrap();
    let uid = Uid::current().as_raw();
    let (tx, rx) = mpsc::channel();
    let worker = thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        assert!(handle_request(stream, uid, || { tx.send(()).map_err(io::Error::other) }).is_err());
    });
    let mut stream = UnixStream::connect_addr(&address).unwrap();
    stream.write_all(b"STOP\n").unwrap();
    worker.join().unwrap();
    assert!(rx.try_recv().is_err());
}

#[test]
fn no_running_shell_does_not_start_another_instance() {
    let addr = test_address();
    assert!(UnixStream::connect_addr(&addr).is_err());
}
