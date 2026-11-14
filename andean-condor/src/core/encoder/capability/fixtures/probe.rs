// Finite native-process stand-in: all scratch creation/ownership stays in the real probes.
use std::{fs, io::{self, Read}, path::PathBuf, time::{Duration, Instant}};

fn main() -> io::Result<()> {
    let args: Vec<_> = std::env::args().collect();
    if args.get(1).is_some_and(|arg| arg == "--full-help") {
        println!("--webm");
        return Ok(());
    }
    let root = PathBuf::from(std::env::var_os("CAPABILITY_PROBE_ROOT").expect("root"));
    let mode = fs::read_to_string(root.join("mode"))?;
    let value = |key| args.windows(2).find(|a| a[0] == key).map(|a| PathBuf::from(&a[1]));
    let zone = value("--zonefile").or_else(|| value("-c"));
    let path = zone.clone().or_else(|| value("-b")).unwrap_or_else(|| PathBuf::from("/dev/null"));
    let record = root.join(format!("record-{}", std::process::id()));
    fs::create_dir(&record)?;
    fs::write(record.join("path"), path.as_os_str().as_encoded_bytes())?;
    fs::write(record.join("argv"), args[1..].join("\n"))?;
    if mode == "feed-failure" {
        std::process::exit(42);
    }
    let mut input = Vec::new();
    io::stdin().read_to_end(&mut input)?;
    let ten_bit = value("-i").is_some();
    let header = if ten_bit { b"YUV4MPEG2 W320 H240 F30:1 Ip A0:0 C420p10\nFRAME\n".as_slice() } else { b"YUV4MPEG2 W320 H240 F30:1 Ip A0:0 C420\nFRAME\n".as_slice() };
    assert!(input.starts_with(header));
    assert_eq!(input.len(), header.len() + 320 * 240 * 3 / 2 * if ten_bit { 2 } else { 1 });
    let expected = if let Some(zone) = zone {
        let bytes = fs::read(zone)?;
        assert_eq!(bytes, if ten_bit { b"Zones : 0,0,30\nForceKeyFrames : 0f\n".as_slice() } else { b"0 --crf 30\n".as_slice() });
        Some(bytes)
    } else if value("-b").is_some() && mode != "missing" {
        if mode == "directory" {
            fs::create_dir(&path)?;
            fs::write(path.join("sidecar"), b"owned")?;
            None
        } else {
            let bytes = format!("DKIF{}", std::process::id()).into_bytes();
            fs::write(&path, &bytes)?;
            Some(bytes)
        }
    } else {
        None
    };
    fs::write(record.join("ready"), b"ready")?;
    let deadline = Instant::now() + Duration::from_secs(8);
    while !root.join("release").exists() {
        assert!(Instant::now() < deadline, "probe handshake was never released");
        std::thread::yield_now();
    }
    // The real owner must retain initialized scratch until the native process exits.
    if let Some(expected) = expected {
        assert_eq!(fs::read(&path)?, expected);
    }
    fs::write(record.join("alive-until-exit"), b"yes")?;
    if mode == "failure" { std::process::exit(42); }
    Ok(())
}
