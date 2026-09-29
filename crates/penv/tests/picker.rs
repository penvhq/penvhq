//! The choice prompt in a real pseudo-terminal: the bytes a terminal sends for
//! the arrow keys go in, and the item chosen comes out. The test binary runs
//! itself as the child, so the prompt is penv's own and no agent check stands
//! in front of it.
#![cfg(unix)]

use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::FromRawFd;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const CHILD: &str = "PENV_PICKER_TEST_CHILD";
const ITEMS: [&str; 4] = ["alpha", "bravo", "charlie", "delta"];

/// Not a test of its own: the child half, run only when the parent asks.
#[test]
fn child() {
    let Ok(mode) = std::env::var(CHILD) else {
        return;
    };
    penv::ui::init(true, false);
    let items: Vec<String> = ITEMS.iter().map(|s| s.to_string()).collect();
    let said = match mode.as_str() {
        "many" => match penv::ui::multiselect("Which ones?", &items, &[]) {
            None => "NO PICKER".to_string(),
            Some(Ok(chosen)) => format!(
                "PICKED {}",
                chosen
                    .iter()
                    .map(|i| ITEMS[*i])
                    .collect::<Vec<_>>()
                    .join(",")
            ),
            Some(Err(_)) => "CANCELLED".to_string(),
        },
        _ => match penv::ui::select("Which?", &items, None) {
            None => "NO PICKER".to_string(),
            Some(Ok(index)) => format!("PICKED {}", ITEMS[index]),
            Some(Err(_)) => "CANCELLED".to_string(),
        },
    };
    eprintln!("\n{said}.");
}

unsafe extern "C" {
    fn posix_openpt(flags: i32) -> i32;
    fn grantpt(fd: i32) -> i32;
    fn unlockpt(fd: i32) -> i32;
    fn ptsname(fd: i32) -> *const std::ffi::c_char;
}

const O_RDWR: i32 = 2;
#[cfg(any(target_os = "linux", target_os = "android"))]
const O_NOCTTY: i32 = 0o400;
#[cfg(not(any(target_os = "linux", target_os = "android")))]
const O_NOCTTY: i32 = 0x20000;

/// A pseudo-terminal: the master end this test types into and reads, and the
/// path of the end the child is given.
fn open_pty() -> (File, String) {
    unsafe {
        let master = posix_openpt(O_RDWR | O_NOCTTY);
        assert!(master >= 0, "posix_openpt");
        assert_eq!(grantpt(master), 0, "grantpt");
        assert_eq!(unlockpt(master), 0, "unlockpt");
        let name = std::ffi::CStr::from_ptr(ptsname(master))
            .to_string_lossy()
            .into_owned();
        (File::from_raw_fd(master), name)
    }
}

/// What a session looks like from the terminal's side.
struct Session {
    master: File,
    /// The terminal's other end, held so the child exiting is no hang-up that
    /// could drop what it wrote before the reader got to it.
    _held: File,
    seen: Arc<Mutex<Vec<u8>>>,
    child: std::process::Child,
}

impl Session {
    /// The child with stderr (where the prompt draws) on the terminal, and stdin
    /// on it too unless `stdin` says otherwise.
    fn start(mode: &str, stdin: Option<Stdio>) -> Session {
        let (master, slave) = open_pty();
        let end = || {
            std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(&slave)
                .expect("the terminal's other end")
        };
        let held = end();
        let child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "child", "--nocapture", "--test-threads=1"])
            .env(CHILD, mode)
            .env("TERM", "xterm-256color")
            .stdin(stdin.unwrap_or_else(|| end().into()))
            .stdout(Stdio::null())
            .stderr(end())
            .spawn()
            .expect("the child runs");
        let seen = Arc::new(Mutex::new(Vec::new()));
        let mut reader = master.try_clone().unwrap();
        let log = Arc::clone(&seen);
        std::thread::spawn(move || {
            let mut buffer = [0u8; 1024];
            while let Ok(n) = reader.read(&mut buffer) {
                if n == 0 {
                    break;
                }
                log.lock().unwrap().extend_from_slice(&buffer[..n]);
            }
        });
        Session {
            master,
            _held: held,
            seen,
            child,
        }
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.seen.lock().unwrap()).into_owned()
    }

    fn wait_for(&self, needle: &str) -> String {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let text = self.text();
            if text.contains(needle) {
                return text;
            }
            assert!(
                Instant::now() < deadline,
                "never saw {needle:?} in {text:?}"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn send(&mut self, bytes: &[u8]) {
        self.master.write_all(bytes).unwrap();
        self.master.flush().unwrap();
        std::thread::sleep(Duration::from_millis(30));
    }

    /// Type `keys`, one write each, once the prompt is on screen, and return
    /// the line the child ends on.
    fn answer(mut self, keys: &[&[u8]]) -> String {
        self.wait_for("enter to");
        for key in keys {
            self.send(key);
        }
        let text = self.wait_for(".\r\n");
        let _ = self.child.wait();
        text.lines()
            .rev()
            .find(|l| l.contains("PICKED") || l.contains("CANCELLED") || l.contains("NO PICKER"))
            .unwrap_or_default()
            .trim()
            .to_string()
    }
}

const CSI_UP: &[u8] = b"\x1b[A";
const CSI_DOWN: &[u8] = b"\x1b[B";
const SS3_UP: &[u8] = b"\x1bOA";
const SS3_DOWN: &[u8] = b"\x1bOB";
const ENTER: &[u8] = b"\r";

#[test]
fn down_down_up_enter_picks_the_second_item_with_csi_arrows() {
    let said = Session::start("one", None).answer(&[CSI_DOWN, CSI_DOWN, CSI_UP, ENTER]);
    assert_eq!(said, "PICKED bravo.");
}

#[test]
fn application_cursor_mode_arrows_move_the_same_way() {
    let said = Session::start("one", None).answer(&[SS3_DOWN, SS3_DOWN, SS3_UP, ENTER]);
    assert_eq!(said, "PICKED bravo.");
}

#[test]
fn an_arrow_split_across_writes_is_an_arrow_and_not_a_cancel() {
    let said = Session::start("one", None).answer(&[b"\x1b", b"[B", b"\x1b", b"O", b"B", ENTER]);
    assert_eq!(said, "PICKED charlie.");
}

#[test]
fn j_k_and_ctrl_n_ctrl_p_move_too() {
    let said = Session::start("one", None).answer(&[b"j", b"\x0e", b"k", b"\x10", b"j", ENTER]);
    assert_eq!(said, "PICKED bravo.");
}

#[test]
fn esc_alone_and_ctrl_c_cancel_and_the_terminal_is_put_back() {
    for key in [b"\x1b".as_slice(), b"\x03"] {
        let session = Session::start("one", None);
        let said = session.answer(&[key]);
        assert_eq!(said, "CANCELLED.", "{key:?}");
    }
    let session = Session::start("one", None);
    session.wait_for("enter to");
    let mut session = session;
    session.send(b"\x03");
    let text = session.wait_for(".\r\n");
    assert!(
        text.contains("\x1b[?25h"),
        "the cursor is shown again: {text:?}"
    );
}

#[test]
fn space_ticks_items_in_a_multiselect() {
    let said =
        Session::start("many", None).answer(&[CSI_DOWN, b" ", CSI_DOWN, CSI_DOWN, b" ", ENTER]);
    assert_eq!(said, "PICKED bravo,delta.");
}

#[test]
fn a_stdin_that_is_not_a_terminal_never_shows_the_prompt() {
    let session = Session::start("one", Some(Stdio::null()));
    let text = session.wait_for(".\r\n");
    assert!(text.contains("NO PICKER."), "{text:?}");
    assert!(!text.contains("Which?"), "the prompt was drawn: {text:?}");
}
