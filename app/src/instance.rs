//! One window for every file: a second `csvfab file.csv` hands its paths to
//! the running window (a new tab each) and exits. If anything about this
//! fails, the second launch simply opens its own window — nothing here can
//! leave a window without the files it was asked to open.

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver};

#[cfg(unix)]
fn socket_path() -> PathBuf {
    let dir = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    // SAFETY: getuid cannot fail.
    let uid = unsafe { libc_getuid() };
    dir.join(format!("csvfab-{uid}.sock"))
}

#[cfg(unix)]
extern "C" {
    #[link_name = "getuid"]
    fn libc_getuid() -> u32;
}

/// Sends the paths to a running window. True if one took them.
pub fn hand_over(paths: &[PathBuf]) -> bool {
    // Made once a window answers (alone, the paths are made absolute by `App::open`).
    let msg = || -> String { paths.iter().map(|p| format!("{}\n", std::fs::canonicalize(p).unwrap_or_else(|_| p.clone()).display())).collect() };
    #[cfg(unix)]
    {
        let Ok(mut s) = std::os::unix::net::UnixStream::connect(socket_path()) else { return false };
        let msg = msg();
        let _ = s.set_write_timeout(Some(ACK_WAIT));
        let _ = s.set_read_timeout(Some(ACK_WAIT));
        // An empty message still raises the window.
        s.write_all(msg.as_bytes()).is_ok() && s.write_all(b"\n").is_ok() && acked(&mut s)
    }
    #[cfg(windows)]
    {
        let Some(port) = std::fs::read_to_string(port_file()).ok().and_then(|s| s.trim().parse::<u16>().ok()) else { return false };
        let Ok(mut s) = std::net::TcpStream::connect_timeout(&([127, 0, 0, 1], port).into(), std::time::Duration::from_millis(300)) else { return false };
        let msg = msg();
        let _ = s.set_read_timeout(Some(ACK_WAIT));
        s.write_all(format!("csvfab\n{msg}\n").as_bytes()).is_ok() && acked(&mut s)
    }
}

/// How long a launch waits for the window to confirm it took the files. A window that
/// accepts the connection but never answers (stopped, wedged) does not swallow them: the
/// launch opens its own window instead.
const ACK_WAIT: std::time::Duration = std::time::Duration::from_secs(1);

/// The window's answer: `ok`, once it has the paths.
fn acked(s: &mut impl std::io::Read) -> bool {
    let mut b = [0u8; 3];
    s.read_exact(&mut b).is_ok() && &b == b"ok\n"
}

#[cfg(windows)]
fn port_file() -> PathBuf {
    crate::config::dir().join("instance.port")
}

/// Listens for paths from later launches; each batch arrives on the channel,
/// and `wake` is called so the UI picks it up.
pub fn listen(wake: impl Fn() + Send + 'static) -> Option<Receiver<Vec<PathBuf>>> {
    let (tx, rx) = channel();
    #[cfg(unix)]
    let listener = {
        let p = socket_path();
        let _ = std::fs::remove_file(&p);       // a stale socket from a crash: nobody answered above
        std::os::unix::net::UnixListener::bind(&p).ok()?
    };
    #[cfg(windows)]
    let listener = {
        let l = std::net::TcpListener::bind(("127.0.0.1", 0)).ok()?;
        let _ = std::fs::create_dir_all(crate::config::dir());
        std::fs::write(port_file(), l.local_addr().ok()?.port().to_string()).ok()?;
        l
    };
    std::thread::spawn(move || {
        for mut conn in listener.incoming().flatten() {
            // A client that connects and goes quiet must not block the next launches.
            let _ = conn.set_read_timeout(Some(ACK_WAIT));
            let mut paths = Vec::new();
            #[cfg(windows)]
            let mut first = true;
            let mut complete = false;
            for line in BufReader::new(&mut conn).lines().map_while(Result::ok) {
                #[cfg(windows)]
                if std::mem::take(&mut first) { if line != "csvfab" { break } continue; }
                if line.is_empty() { complete = true; break; }
                paths.push(PathBuf::from(line));
            }
            // Half a message (the launch died, timed out): it opens its own window, or nothing.
            if !complete { continue; }
            if tx.send(paths).is_err() { break; }
            let _ = conn.write_all(b"ok\n");
            wake();
        }
    });
    Some(rx)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn hand_over_needs_the_windows_answer() {
        let dir = std::env::temp_dir().join(format!("csvfab-instance-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // The only test touching this variable (socket_path reads it).
        std::env::set_var("XDG_RUNTIME_DIR", &dir);
        // No window: the launch opens its own.
        assert!(!hand_over(&[PathBuf::from("/nonexistent/a.csv")]));
        // A window: it takes the paths and answers.
        let rx = listen(|| {}).unwrap();
        assert!(hand_over(&[PathBuf::from("/nonexistent/a.csv"), PathBuf::from("/nonexistent/b.csv")]));
        assert_eq!(rx.recv().unwrap(), vec![PathBuf::from("/nonexistent/a.csv"), PathBuf::from("/nonexistent/b.csv")]);
        assert!(hand_over(&[]));
        assert_eq!(rx.recv().unwrap(), Vec::<PathBuf>::new());
        // A window that accepts but never answers (stopped): the launch does not lose the files.
        drop(rx);
        let _ = std::fs::remove_file(socket_path());
        let _mute = std::os::unix::net::UnixListener::bind(socket_path()).unwrap();
        let t = std::time::Instant::now();
        assert!(!hand_over(&[PathBuf::from("/nonexistent/a.csv")]));
        assert!(t.elapsed() < ACK_WAIT * 2);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// After a hand-over: brings the window that took the files to the front. Wayland lets a
/// window raise itself only with an activation token it does not have here, so on
/// Hyprland the compositor is asked (as the web launcher did): the Lua dispatcher of
/// Hyprland 0.56+, the old `focuswindow` form if that one is refused.
pub fn raise() {
    if std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_none() { return; }
    let sel = r"class:^(csvfab)$";
    let lua = format!("hl.dsp.focus({{ window = \"{}\" }})", sel.replace('\\', "\\\\"));
    let ok = std::process::Command::new("hyprctl").args(["dispatch", &lua]).output().is_ok_and(|o| o.status.success() && String::from_utf8_lossy(&o.stdout).trim() == "ok");
    if !ok {
        let _ = std::process::Command::new("hyprctl").args(["dispatch", "focuswindow", sel]).output();
    }
}
