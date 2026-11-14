// Finite Linux lifecycle fixture, not codec or licensed-plugin acceptance.
use std::{fs, io::{self, Write}, path::PathBuf, process::Command, time::{Duration, Instant}};
unsafe extern "C" { fn prctl(option: i32, ...) -> i32; fn kill(pid: i32, sig: i32) -> i32; fn waitpid(pid: i32, status: *mut i32, flags: i32) -> i32; fn signal(sig: i32, handler: usize) -> usize; fn getppid() -> i32; fn getpgid(pid: i32) -> i32; fn setpgid(pid: i32, pgid: i32) -> i32; }
fn publish_pid(path: PathBuf, pid: u32) -> io::Result<()> {
    let pending = path.with_extension("pending");
    fs::write(&pending, pid.to_string())?;
    fs::rename(pending, path)
}
fn main() -> io::Result<()> {
    let args: Vec<_> = std::env::args().collect();
    if args.get(1).is_some_and(|a| a == "--complete") { return Ok(()); }
    if args.get(1).is_some_and(|a| a == "supervise" || a == "supervise-index-publication") {
        use std::os::unix::process::CommandExt;
        // SAFETY: This isolated Linux subreaper takes integer arguments only.
        assert_eq!(unsafe { prctl(36, 1_usize, 0_usize, 0_usize, 0_usize) }, 0);
        let root = PathBuf::from(&args[3]);
        let case = &args[4];
        let index_child = args[1] == "supervise-index-publication";
        let test = args.get(5).expect("explicit exact lifecycle/index selector");
        let mut child = Command::new(&args[2]).args(["--exact", test, "--nocapture"]).env(if index_child { "VS_INDEX_ROOT" } else { "PROCESS_LIFECYCLE_ROOT" }, &root).env(if index_child { "VS_INDEX_CASE" } else { "PROCESS_LIFECYCLE_CASE" }, case).process_group(0).spawn()?;
        let start = Instant::now();
        let mut rescued = 0;
        let status = loop {
            if let Some(s) = child.try_wait()? { break s; }
            if start.elapsed() > Duration::from_secs(4) {
                rescued += 1;
                // SAFETY: Only the fixture test's dedicated process group is selected.
                unsafe { kill(-(child.id() as i32), 9); }
                for name in ["leader", "grandchild"] {
                    if let Ok(pid) = fs::read_to_string(root.join(name)).and_then(|s| s.parse::<i32>().map_err(io::Error::other)) {
                        // SAFETY: PIDs came only from this private finite fixture.
                        unsafe { kill(pid, 9); }
                    }
                }
                if index_child && case == "concurrent" {
                    for entry in fs::read_dir(&root)? {
                        let path = entry?.path();
                        if path.file_name().is_some_and(|name| name.to_string_lossy().starts_with("leader-")) {
                            let pid = fs::read_to_string(path)?.parse::<i32>().map_err(io::Error::other)?;
                            // SAFETY: Each PID is a child handshake in this private fixture directory.
                            unsafe { kill(pid, 9); }
                        }
                    }
                }
                break child.wait()?;
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        loop {
            // SAFETY: Only this isolated supervisor's adopted fixture children are reaped.
            let pid = unsafe { waitpid(-1, std::ptr::null_mut(), 0) };
            if pid > 0 { rescued += 1; } else if io::Error::last_os_error().raw_os_error() == Some(4) { continue; } else { break; }
        }
        println!("process lifecycle case={case} selected={} rescued={rescued}", root.join("selected").exists());
        std::process::exit(if status.success() && rescued == 0 { 0 } else { 1 });
    }
    if args.get(1).is_some_and(|a| a == "grandchild") {
        std::thread::sleep(Duration::from_secs(8));
        return Ok(());
    }
    let value = |key: &str| args.windows(2).find(|a| a[0] == key).map(|a| a[1].clone());
    if let Some(root) = std::env::var_os("VS_INDEX_ROOT") {
        let root = PathBuf::from(root);
        let case = std::env::var("VS_INDEX_CASE").expect("case");
        writeln!(fs::OpenOptions::new().create(true).append(true).open(root.join("invocations"))?, "run")?;
        let output = PathBuf::from(value("-o").expect("output"));
        fs::write(root.join(format!("argv-{}", std::process::id())), args[1..].join("\n"))?;
        let leader = if case == "concurrent" { format!("leader-{}", std::process::id()) } else { "leader".into() };
        publish_pid(root.join(leader), std::process::id())?;
        if case != "missing" { fs::write(&output, "partial")?; }
        io::stdout().write_all(b"index stdout\xff\n")?;
        if case == "observer-panic" { io::stdout().write_all(b"100\n")?; }
        io::stderr().write_all(b"index stderr\xfe\n")?;
        if case == "failure-retry" && !root.join("succeed").exists() { std::process::exit(42); }
        if case == "force-retry" && !root.join("succeed").exists() {
            let mut grandchild = Command::new(std::env::current_exe()?).arg("grandchild").spawn()?;
            publish_pid(root.join("grandchild"), grandchild.id())?;
            // Do not race the subreaper by reaping the grandchild during group SIGKILL.
            std::thread::sleep(Duration::from_secs(8));
            grandchild.wait()?;
            return Ok(());
        }
        if case == "graceful" {
            fs::write(root.join("active"), "yes")?;
            let deadline = Instant::now() + Duration::from_secs(8);
            while !root.join("release").exists() && Instant::now() < deadline { std::thread::yield_now(); }
        }
        if case == "concurrent" {
            fs::write(root.join(format!("active-{}", std::process::id())), "yes")?;
            let deadline = Instant::now() + Duration::from_secs(8);
            while !root.join("release").exists() && Instant::now() < deadline { std::thread::yield_now(); }
        }
        match case.as_str() {
            "missing" => {},
            "zero" => fs::write(&output, "")?,
            "garbage" => fs::write(&output, "garbage")?,
            "truncated" => fs::write(&output, "DGHEVCIndexFileNV18 DGIndexNV 2053\nCODED 50\nORDER ")?,
            "directory" => { fs::remove_file(&output)?; fs::create_dir(&output)?; fs::write(output.join("private-sidecar"), "owned")?; },
            "symlink" => { fs::remove_file(&output)?; std::os::unix::fs::symlink(root.join("sentinel"), &output)?; },
            _ => { fs::copy(root.join("sample.dgi"), &output)?; fs::write(output.with_extension("log"), "owned sidecar")?; },
        }
        return Ok(());
    }
    // The public zone eligibility probe is finite and receives a real Y4M frame.
    if value("--zones").as_deref() == Some("0,0,crf=30") && std::env::var("PROCESS_LIFECYCLE_CASE").as_deref() != Ok("zone-probe") {
        io::copy(&mut io::stdin(), &mut io::sink())?;
        return Ok(());
    }
    let root = PathBuf::from(std::env::var_os("PROCESS_LIFECYCLE_ROOT").expect("root"));
    let case = std::env::var("PROCESS_LIFECYCLE_CASE").expect("case");
    // SAFETY: Reset only this fixture process's SIGINT disposition, so group shielding is real.
    unsafe { signal(2, 0); }
    publish_pid(root.join("leader"), std::process::id())?;
    let output = if case.starts_with("mkv") || case.starts_with("commit-mkvmerge") {
        let options = fs::read_to_string(args.iter().find_map(|s| s.strip_prefix('@')).expect("options"))?;
        let mut lines = options.lines().map(str::trim);
        lines.find(|s| *s == "\"-o\",");
        PathBuf::from(lines.next().expect("output").trim_end_matches(',').trim_matches('"'))
    } else {
        PathBuf::from(value("-o").unwrap_or_else(|| args.last().expect("output").clone()))
    };
    if case == "stream-idle-return" {
        io::copy(&mut io::stdin(), &mut io::sink())?;
        fs::write(output, "complete")?;
        return Ok(());
    }
    if case.starts_with("commit-") {
        io::copy(&mut io::stdin(), &mut io::sink())?;
        if let Some(pass) = value("--pass") { writeln!(fs::OpenOptions::new().create(true).append(true).open(root.join("passes"))?, "{pass}")?; }
        if output != PathBuf::from("/dev/null") { fs::write(output, if case.starts_with("commit-dg") { "DGHEVCIndexFileNV18 DGIndexNV 2053\n\nCODED 50\nORDER 0\n" } else { "complete" })?; }
        if case.ends_with("-between") { fs::write(root.join("committed"), "pass1partial")?; }
        return Ok(());
    }
    if case.starts_with("graceful") {
        io::copy(&mut io::stdin(), &mut io::sink())?;
        writeln!(fs::OpenOptions::new().create(true).append(true).open(root.join("passes"))?, "{}", value("--pass").expect("pass"))?;
        if value("--pass").as_deref() == Some("1") {
            fs::write(root.join("active"), "yes")?;
            let deadline = Instant::now() + Duration::from_secs(8);
            while !root.join("release").exists() && Instant::now() < deadline { std::thread::yield_now(); }
        }
        if output != PathBuf::from("/dev/null") { fs::write(output, [0, 0, 1, 0x65, 0x88])?; }
        return Ok(());
    }
    if !case.starts_with("filter") && output != PathBuf::from("/dev/null") && !output.exists() { fs::write(output, "interrupted")?; }
    let mut grandchild = Command::new(std::env::current_exe()?).arg("grandchild").spawn()?;
    if ["mid-production", "stdin-full", "serial", "serial-native"].contains(&case.as_str()) {
        io::stdout().write_all(b"process lifecycle mid stdout\xff\n")?;
        io::stdout().flush()?;
        io::stderr().write_all(b"process lifecycle mid stderr\xfe\n")?;
        io::stderr().flush()?;
    }
    publish_pid(root.join("grandchild"), grandchild.id())?;
    if case == "leader-changes-group" {
        // SAFETY: Join the test parent's existing group in this session. A group
        // leader cannot setsid(), but can setpgid() when it is not a session leader.
        assert_eq!(unsafe { setpgid(0, getpgid(getppid())) }, 0);
        fs::write(root.join("group-changed"), "ready")?;
        std::thread::sleep(Duration::from_secs(8));
    }
    if case == "leader-exits" { std::process::exit(0); }
    if case == "filter-full" {
        for _ in 0..32 { io::stdout().write_all(&[b'x'; 64 * 1024])?; }
    }
    // Keep the leader out of waitpid during simultaneous group SIGKILL: otherwise
    // it can reap the grandchild just before dying, defeating the adoption oracle.
    // The four-second supervisor fails/rescues if force never arrives.
    std::thread::sleep(Duration::from_secs(8));
    grandchild.wait()?;
    Ok(())
}
