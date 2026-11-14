// Compiled as a finite fake encoder by process tests, not a native codec acceptance test.
use std::{
    fs,
    fs::OpenOptions,
    io::{self, Write},
    path::PathBuf,
};

fn main() -> io::Result<()> {
    let args: Vec<_> = std::env::args().collect();
    if args.get(1).is_some_and(|arg| arg == "--encoder-fixture-watchdog") {
        watchdog(&args)?;
        unreachable!("watchdog exits after reaping the fixture tree");
    }
    if args.get(1).is_some_and(|arg| arg == "--encoder-fixture-stall") {
        // Bound orphan lifetime even if the watchdog itself is interrupted.
        std::thread::sleep(std::time::Duration::from_secs(30));
        return Ok(());
    }
    if args.get(1).is_some_and(|arg| arg == "--encoder-force-force-idle") {
        use std::{io::BufRead, time::{Duration, Instant}};
        let root = PathBuf::from(&args[2]);
        let mut header = String::new();
        io::stdin().lock().read_line(&mut header)?;
        assert!(header.starts_with("YUV4MPEG2 ") && header.ends_with('\n'), "actual producer header: {:?}", header);
        let mut leader = fs::File::create(root.join("leader"))?;
        if std::env::var("ENCODER_FIXTURE_CASE").is_ok_and(|case| case.contains("-progress-force")) {
            fs::write(root.join("ready-opened"), "PID file open, not initialized")?;
            let deadline = Instant::now() + Duration::from_secs(3);
            while !root.join("publish-ready").exists() {
                assert!(Instant::now() < deadline, "readiness observer acknowledgement");
                std::thread::yield_now();
            }
        }
        write!(leader, "{}", std::process::id())?;
        drop(leader);
        fs::write(root.join("ready"), "producer header read; complete PIDs")?;
        // Like --encoder-fixture-stall, bound emergency lifetime. Never wait/reap: the test
        // subreaper must receive the grandchild even if it dies before this leader.
        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline {
            std::thread::park_timeout(deadline.saturating_duration_since(Instant::now()));
        }
        return Err(io::Error::other("force idle emergency lifetime expired"));
    }
    let value = |key: &str| args.windows(2).find(|a| a[0] == key).map(|a| a[1].clone());
    let early = value("--encoder-fixture-exit");
    let marker = value("--encoder-fixture-marker");
    let stats = value("--stats").map(PathBuf::from);
    if early.is_none() {
        io::copy(&mut io::stdin(), &mut io::sink())?;
    }
    if args.iter().any(|arg| arg == "--vvenc-fixture-progress") {
        // VVenC's stdout stats are distinct from stderr diagnostics and AVM POC records.
        io::stdout().write_all(b"noise frame= 700\r\nPOC: 0\nvvenc [info]: stats:  frame= 60 fps= 2.9 avg_fps= 2.9 bitrate= 29173.77 kbps avg_bitrate= 29173.77 kbps elapsed= 0\rvvenc [info]: stats: frame=bad fps= 999\nbad\xff\r\nvvenc [info]: stats: 75.0%% frame= 90/120 fps= 2.9 avg_fps= 2.9 bitrate= 29173.77 kbps avg_bitrate= 29173.77 kbps elapsed= 1\nvvenc [info]: stats: frame=90\rvvenc [info]: stats: frame=30\n")?;
        io::stderr().write_all(b"diagnostic\xfe\r\nvvenc [info]: stats: frame=900 fps= 999\n")?;
        for _ in 0..512 {
            io::stdout().write_all(&[b'o'; 1024])?;
            io::stderr().write_all(&[b'e'; 1024])?;
        }
        io::stdout().write_all(b"\nvvenc [info]: stats: frame=120 fps= 2.9\rvvenc [info]: stats: frame=120 fps= 2.9")?;
        io::stderr().write_all(b"\r\nstderr tail")?;
        return Ok(());
    }
    if let Some(records) = value("--encoder-fixture-progress") {
        for frame in 1..=records.parse::<usize>().unwrap() {
            writeln!(io::stderr(), "{frame}/4 frames\r")?;
        }
    } else {
        io::stdout().write_all(b"POC: 0\r\n")?;
        io::stderr().write_all(b"1/4 frames\r\n")?;
    }
    if let Some(marker) = value("--encoder-fixture-drop-marker") {
        while !PathBuf::from(&marker).exists() {
            std::thread::yield_now();
        }
        io::stdout().write_all(b"POC: 1\r\n")?;
        io::stderr().write_all(b"2/4 frames\r\n")?;
    }
    // Both streams exceed pipe capacity, including when the other stream blocks.
    for _ in 0..512 {
        io::stdout().write_all(&[b'o'; 1024])?;
        io::stderr().write_all(&[b'e'; 1024])?;
    }
    io::stdout().write_all(b"\r\nbad\xff\rstdout tail")?;
    io::stderr().write_all(b"\r\nbad\xfe\rstderr tail")?;
    if let Some(marker) = marker {
        let alive = stats.as_ref().is_some_and(|s| s.parent().unwrap().exists());
        fs::write(marker, format!("stats_alive={alive}"))?;
    }
    if let Some(order) = value("--encoder-fixture-order") {
        writeln!(
            OpenOptions::new().append(true).create(true).open(order)?,
            "{}",
            value("-o").unwrap()
        )?;
        let output = PathBuf::from(value("-o").unwrap());
        if output != PathBuf::from("/dev/null") {
            fs::write(output, "finite serial ownership marker")?;
        }
    }
    if let Some(code) = early {
        std::process::exit(code.parse().unwrap());
    }
    Ok(())
}

// Linux-only process fixture: adopt/reap grandchildren too, not just the test leader.
fn watchdog(args: &[String]) -> io::Result<()> {
    use std::{
        os::unix::process::CommandExt,
        process::Command,
        time::{Duration, Instant},
    };
    unsafe extern "C" {
        fn prctl(option: i32, ...) -> i32;
        fn kill(pid: i32, signal: i32) -> i32;
        fn waitpid(pid: i32, status: *mut i32, options: i32) -> i32;
    }
    // SAFETY: Linux PR_SET_CHILD_SUBREAPER takes integer arguments, no pointers.
    if unsafe { prctl(36, 1_usize, 0_usize, 0_usize, 0_usize) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let case = &args[4];
    let mut child = Command::new(&args[2])
        .args([
            "--exact",
            args.get(5).map_or("core::encoder::tests::diagnostics::diagnostics_child", String::as_str),
            "--nocapture",
        ])
        .env("ENCODER_FIXTURE_ROOT", &args[3])
        .env("ENCODER_FIXTURE_CASE", case)
        .process_group(0)
        .spawn()?;
    let mut start = Instant::now();
    let mut timeout = Duration::from_secs(20);
    let mut armed = false;
    let mut timed_out = false;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if case == "watchdog" && !armed && PathBuf::from(&args[3]).join("watchdog-started").exists()
        {
            armed = true;
            start = Instant::now();
            timeout = Duration::from_millis(250);
        }
        if start.elapsed() > timeout {
            timed_out = true;
            // SAFETY: The child leads a dedicated process group; negative pid selects only it.
            unsafe {
                kill(-(child.id() as i32), 9);
            }
            break child.wait()?;
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    // Even a failed assertion must not leave a detached encoder behind.
    // SAFETY: Same private group; ESRCH is harmless when it is already empty.
    unsafe {
        kill(-(child.id() as i32), 9);
    }
    let mut reaped = 0;
    loop {
        // SAFETY: Null status is allowed; wait only for this isolated supervisor's children.
        let pid = unsafe { waitpid(-1, std::ptr::null_mut(), 0) };
        if pid > 0 {
            reaped += 1;
            continue;
        }
        let error = io::Error::last_os_error();
        match error.raw_os_error() {
            Some(4) => continue, // EINTR
            Some(10) => break,   // ECHILD: the whole tree was reaped.
            _ => return Err(error),
        }
    }
    println!("encoder fixture watchdog reaped {reaped} descendants; timeout={timed_out}");
    std::process::exit(if timed_out {
        124
    } else {
        status.code().unwrap_or(1)
    });
}
