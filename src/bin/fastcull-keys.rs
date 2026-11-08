#![cfg_attr(windows, windows_subsystem = "windows")]
#[cfg(windows)]
mod native {
    use std::{
        path::{Path, PathBuf},
        time::{Duration, Instant, SystemTime, UNIX_EPOCH},
    };
    use windows_sys::Win32::{
        Foundation::*,
        System::Threading::*,
        UI::{Input::KeyboardAndMouse::*, WindowsAndMessaging::*},
    };
    const ID: i32 = 0x4643;
    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain([0]).collect()
    }
    fn register() -> bool {
        unsafe {
            RegisterHotKey(
                std::ptr::null_mut(),
                ID,
                MOD_CONTROL | MOD_ALT | MOD_NOREPEAT,
                0x4b,
            ) != 0
        }
    }
    pub fn matches(host: &str, name: &str) -> bool {
        match host {
            "bridge" => {
                name.eq_ignore_ascii_case("adobe bridge.exe")
                    || name.eq_ignore_ascii_case("bridge.exe")
            }
            "lightroom" => name.eq_ignore_ascii_case("lightroom.exe"),
            _ => false,
        }
    }
    fn foreground(host: &str) -> bool {
        unsafe {
            let window = GetForegroundWindow();
            if window.is_null() || !GetWindow(window, GW_OWNER).is_null() {
                return false;
            }
            let mut pid = 0;
            GetWindowThreadProcessId(window, &mut pid);
            let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if process.is_null() {
                return false;
            }
            let mut buffer = [0u16; 32768];
            let mut length = buffer.len() as u32;
            let ok = QueryFullProcessImageNameW(process, 0, buffer.as_mut_ptr(), &mut length);
            CloseHandle(process);
            if ok == 0 {
                return false;
            }
            let name = String::from_utf16_lossy(&buffer[..length as usize]);
            Path::new(&name)
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|name| matches(host, name))
        }
    }
    fn live(dir: &Path) -> bool {
        std::fs::metadata(dir.join("alive"))
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| SystemTime::now().duration_since(t).ok())
            .is_some_and(|age| age < Duration::from_secs(30))
    }
    fn fresh_job(modified: SystemTime, now: SystemTime) -> bool {
        now.duration_since(modified)
            .is_ok_and(|age| age < Duration::from_secs(10))
    }
    // Bridge publishes closed UTF-8 lists by atomic rename. Claim before spawn
    // so a later scan cannot launch the same request twice. Never execute a
    // command or program supplied by the queue: only our sibling FastCull.
    fn drain_launch_queue(
        dir: &Path,
        mut spawn: impl FnMut(&Path) -> anyhow::Result<()>,
    ) -> anyhow::Result<()> {
        let queue = dir.parent().unwrap_or(dir).join("bridge-requests");
        if !queue.is_dir() {
            return Ok(());
        }
        for entry in std::fs::read_dir(queue)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().is_none_or(|ext| ext != "queued") || !entry.file_type()?.is_file() {
                continue;
            }
            if !fresh_job(entry.metadata()?.modified()?, SystemTime::now()) {
                let _ = std::fs::rename(&path, path.with_extension("expired"));
                continue;
            }
            let claimed = path.with_extension("starting");
            if std::fs::rename(&path, &claimed).is_err() {
                continue;
            }
            if let Err(error) = spawn(&claimed) {
                let failed = path.with_extension("failed");
                let _ = std::fs::rename(&claimed, &failed);
                let _ = std::fs::write(
                    dir.join("launch-error"),
                    format!("{}\n{error:#}", failed.display()),
                );
            }
        }
        Ok(())
    }
    fn launch_bridge_job(request: &Path) -> anyhow::Result<()> {
        use std::os::windows::process::CommandExt;
        let executable = std::env::current_exe()?.with_file_name("fastcull.exe");
        let child = std::process::Command::new(executable)
            .arg("--bridge-list-owned")
            .arg(request)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP)
            .spawn()?;
        unsafe {
            AllowSetForegroundWindow(child.id());
        }
        Ok(())
    }
    pub fn run() -> anyhow::Result<()> {
        let mut args: Vec<_> = std::env::args_os().skip(1).collect();
        // Bridge File.execute resolves Windows shortcuts without preserving
        // their arguments. Launch this executable directly with a default host.
        if args.is_empty() {
            let directory = PathBuf::from(
                std::env::var_os("APPDATA").ok_or_else(|| anyhow::anyhow!("APPDATA missing"))?,
            )
            .join("FastCull/hotkeys-bridge");
            args = vec![
                "--watch".into(),
                "bridge".into(),
                directory.into_os_string(),
            ];
        }
        if args.first().is_some_and(|a| a == "--check") {
            anyhow::ensure!(register(), "Ctrl+Alt+K is already registered");
            unsafe {
                UnregisterHotKey(std::ptr::null_mut(), ID);
            }
            return Ok(());
        }
        anyhow::ensure!(args.len() == 3, "expected --start/--watch host directory");
        let host = args[1].to_str().unwrap_or("");
        anyhow::ensure!(host == "bridge" || host == "lightroom", "invalid host");
        let dir = PathBuf::from(&args[2]);
        anyhow::ensure!(dir.is_absolute(), "absolute request directory required");
        if args[0] == "--start" {
            use std::os::windows::process::CommandExt;
            std::process::Command::new(std::env::current_exe()?)
                .arg("--watch")
                .arg(host)
                .arg(&dir)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP)
                .spawn()?;
            return Ok(());
        }
        anyhow::ensure!(args[0] == "--watch", "invalid mode");
        let mutex = unsafe {
            CreateMutexW(
                std::ptr::null(),
                0,
                wide(&format!("Local\\FastCull-Keys-{host}")).as_ptr(),
            )
        };
        anyhow::ensure!(!mutex.is_null(), "cannot create hotkey mutex");
        if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
            unsafe {
                CloseHandle(mutex);
            }
            return Ok(());
        }
        let mut registered = false;
        let mut last = Instant::now() - Duration::from_secs(2);
        let mut retry = Instant::now() - Duration::from_secs(2);
        let mut message: MSG = unsafe { std::mem::zeroed() };
        let mut heartbeat = Instant::now() - Duration::from_secs(2);
        while live(&dir) {
            if heartbeat.elapsed() >= Duration::from_secs(1) {
                let stamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis();
                let _ = std::fs::write(dir.join("helper-alive"), stamp.to_string());
                if host == "bridge" {
                    let _ = std::fs::write(dir.join("launcher-ready"), stamp.to_string());
                }
                heartbeat = Instant::now();
            }
            if host == "bridge" {
                let _ = drain_launch_queue(&dir, launch_bridge_job);
            }
            let active = foreground(host);
            if registered && !active {
                unsafe {
                    UnregisterHotKey(std::ptr::null_mut(), ID);
                }
                registered = false;
                let _ = std::fs::remove_file(dir.join("request"));
            }
            if active && !registered && retry.elapsed() > Duration::from_secs(1) {
                registered = register();
                retry = Instant::now();
                let _ = std::fs::write(
                    dir.join("status"),
                    if registered {
                        "ready: Ctrl+Alt+K"
                    } else {
                        "conflict: Ctrl+Alt+K is registered by another application"
                    },
                );
            }
            unsafe {
                while PeekMessageW(
                    &mut message,
                    std::ptr::null_mut(),
                    WM_HOTKEY,
                    WM_HOTKEY,
                    PM_REMOVE,
                ) != 0
                {
                    if message.wParam == ID as usize
                        && registered
                        && foreground(host)
                        && last.elapsed() > Duration::from_millis(800)
                    {
                        let stamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis();
                        // Replace a pending request, never accumulate launches while busy.
                        std::fs::write(dir.join("request"), stamp.to_string())?;
                        last = Instant::now();
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(40));
        }
        if registered {
            unsafe {
                UnregisterHotKey(std::ptr::null_mut(), ID);
            }
        }
        let _ = std::fs::write(dir.join("status"), "stopped: host heartbeat expired");
        unsafe {
            CloseHandle(mutex);
        }
        Ok(())
    }
    #[cfg(test)]
    mod tests {
        use std::{
            fs,
            time::{Duration, SystemTime},
        };
        #[test]
        fn queue_claims_closed_lists_once_and_retains_launch_failures() {
            let root = tempfile::tempdir().unwrap();
            let host = root.path().join("hotkeys-bridge");
            let queue = root.path().join("bridge-requests");
            fs::create_dir_all(&host).unwrap();
            fs::create_dir_all(&queue).unwrap();
            fs::write(queue.join("one.txt"), "unfinished").unwrap();
            fs::write(
                queue.join("two.queued"),
                "FASTCULL_BRIDGE_V1\n照片 & %.ARW\n",
            )
            .unwrap();
            let mut calls = 0;
            super::drain_launch_queue(&host, |request| {
                calls += 1;
                assert_eq!(request.extension().unwrap(), "starting");
                assert!(fs::read_to_string(request)?.contains("照片 & %.ARW"));
                Ok(())
            })
            .unwrap();
            super::drain_launch_queue(&host, |_| panic!("duplicate launch")).unwrap();
            assert_eq!(calls, 1);
            assert!(queue.join("one.txt").exists());
            fs::write(queue.join("bad.queued"), "invalid").unwrap();
            super::drain_launch_queue(&host, |_| anyhow::bail!("spawn denied")).unwrap();
            assert!(queue.join("bad.failed").exists());
            assert!(
                fs::read_to_string(host.join("launch-error"))
                    .unwrap()
                    .contains("spawn denied")
            );
            super::drain_launch_queue(&host, |_| panic!("retry failed job")).unwrap();
        }
        #[test]
        fn stale_or_future_queue_jobs_must_not_launch_on_helper_restart() {
            let now = SystemTime::now();
            assert!(super::fresh_job(now, now));
            assert!(!super::fresh_job(now - Duration::from_secs(10), now));
            assert!(!super::fresh_job(now + Duration::from_secs(1), now));
        }
        #[test]
        fn only_exact_adobe_host_executables_match() {
            assert!(super::matches("bridge", "Adobe Bridge.exe"));
            assert!(super::matches("lightroom", "Lightroom.exe"));
            for name in [
                "fastcull.exe",
                "bridge-helper.exe",
                "Lightroom.exe.bak",
                "notepad.exe",
            ] {
                assert!(!super::matches("bridge", name));
            }
            assert!(!super::matches("lightroom", "Adobe Bridge.exe"));
        }
    }
}
fn main() {
    #[cfg(windows)]
    if native::run().is_err() {
        std::process::exit(1);
    }
}
