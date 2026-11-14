use andean_condor::{
    core::encoder::parse::*,
    models::encoder::{Encoder, EncoderBase},
};

fn assert_parser_parity(input: &[u8], expected: Option<u64>) {
    if let Ok(line) = std::str::from_utf8(input) {
        assert_eq!(parse_aom_vpx_frames(line), expected, "scalar: {input:?}");
        for encoder in [EncoderBase::AOM, EncoderBase::VPX, EncoderBase::AVM] {
            assert_eq!(
                Encoder::parse_encoded_frames_base(&encoder, line),
                expected,
                "public {encoder:?}: {input:?}"
            );
        }
    }
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    if is_x86_feature_detected!("sse4.1") && is_x86_feature_detected!("ssse3") {
        assert_eq!(
            // SAFETY: Both required CPU features are checked. Every load must be bounded.
            unsafe { parse_aom_vpx_frames_sse41(input) },
            expected,
            "SIMD: {input:?}"
        );
    }
}

// Literal expectations are independent of the parser, including every truncation.
fn guard_page_inputs(mut check: impl FnMut(&[u8], Option<u64>)) -> usize {
    let mut cases = 0;
    let mut check = |input: &[u8], expected| {
        cases += 1;
        check(input, expected);
    };
    for count in [
        0,
        1,
        9,
        12,
        123,
        1234,
        12345,
        123_456,
        1_234_567,
        12_345_678,
        123_456_789,
        1_234_567_890,
        12_345_678_901,
        123_456_789_012,
        1_234_567_890_123,
        12_345_678_901_234,
        123_456_789_012_345,
        999_999_999_999_999,
        1_234_567_890_123_456,
        9_999_999_999_999_999,
        12_345_678_901_234_567,
        123_456_789_012_345_678,
        1_234_567_890_123_456_789,
        12_345_678_901_234_567_890,
        u64::MAX,
    ] {
        for slash in 19..=53 {
            let mut line = vec![b'x'; slash];
            line.extend_from_slice(format!("/{count} ").as_bytes());
            let terminated = line.len();
            line.extend_from_slice(b"123456789012345678901234567890123B");
            for end in 0..=line.len() {
                check(&line[..end], (end >= terminated).then_some(count));
            }
        }
    }
    for count in [
        "",
        "+1",
        "-1",
        "1x",
        "x1",
        "1.0",
        "1/2",
        "1\t",
        "１",
        "\u{a0}1",
        "18446744073709551616",
        "99999999999999999999",
        "99999999999999999999999999999999",
    ] {
        for slash in 19..=53 {
            let mut line = vec![b'x'; slash];
            line.extend_from_slice(format!("/{count} ").as_bytes());
            check(&line, None);
            line.extend_from_slice(b"123456789012345678901234567890123B");
            check(&line, None);
        }
    }
    for count in ["00000000000000000000000000000000", "00000000000000000000000000000001"] {
        let line = format!("Pass 1/1 frame    1/{count} ");
        check(line.as_bytes(), Some(u64::from(count.ends_with('1'))));
    }
    for len in 0..=96 {
        let mut line = vec![b'7'; len];
        check(&line, None); // Neither required separator.
        if let Some(last) = line.last_mut() {
            *last = b' ';
            check(&line, None);
        } // No slash.
        for slash in 19..len {
            let mut line = vec![b'7'; len];
            line[slash] = b'/';
            check(&line, None); // No space, including all 16-byte boundaries.
        }
    }
    for byte in 0..=u8::MAX {
        let mut line = b"Pass 1/1 frame    1/0 12345678901234567890B".to_vec();
        line[20] = byte;
        let expected = byte.is_ascii_digit().then(|| u64::from(byte - b'0'));
        check(&line, expected);
    }
    for place in 0..15 {
        for byte in 0..=u8::MAX {
            let mut line = b"Pass 1/1 frame    1/000000000000000 12345678901234567890B".to_vec();
            line[20 + place] = byte;
            // A space after a zero terminates a valid count before trailing fields.
            let expected = if byte == b' ' && place > 0 {
                Some(0)
            } else {
                byte.is_ascii_digit()
                    .then(|| u64::from(byte - b'0') * 10_u64.pow(14 - place as u32))
            };
            check(&line, expected);
        }
    }
    cases
}

#[test]
fn aom_scalar_and_simd_parsers_agree_on_truncated_records() {
    isolate_guard_page_case(
        "core::encoder::parse::aom_scalar_and_simd_parsers_agree_on_truncated_records",
        || {
            let cases = guard_page_inputs(assert_parser_parity);
            #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
            println!(
                "parser-bounds matrix: {cases} cases; SSSE3+SSE4.1={}",
                is_x86_feature_detected!("sse4.1") && is_x86_feature_detected!("ssse3")
            );
            #[cfg(not(any(target_arch = "x86", target_arch = "x86_64")))]
            println!("parser-bounds matrix: {cases} cases; SIMD unavailable on this architecture");
        },
    );
}

#[test]
#[cfg(target_os = "linux")]
fn aom_simd_parser_never_reads_past_a_guarded_record() {
    isolate_guard_page_case(
        "core::encoder::parse::aom_simd_parser_never_reads_past_a_guarded_record",
        guard_page_cases,
    );
}

fn isolate_guard_page_case(test_name: &str, run: impl FnOnce()) {
    use std::{
        process::Command,
        thread,
        time::{Duration, Instant},
    };
    const CHILD: &str = "AV1AN_PARSER_BOUNDS_CHILD";
    if std::env::var_os(CHILD).is_some() {
        #[cfg(target_os = "linux")]
        assert_eq!(
            // SAFETY: This isolated child disables core dumps with a valid limit structure.
            unsafe {
                libc::setrlimit(
                    libc::RLIMIT_CORE,
                    &libc::rlimit {
                        rlim_cur: 0,
                        rlim_max: 0,
                    },
                )
            },
            0
        );
        run();
        println!("parser-bounds child cases completed");
        return;
    }
    // Re-exec isolates overreads from the multithreaded parent test runner.
    let mut child = Command::new(std::env::current_exe().expect("test executable"))
        .args(["--exact", test_name, "--nocapture"])
        .env(CHILD, "1")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("start parser child");
    let deadline = Instant::now() + Duration::from_secs(30);
    while child.try_wait().expect("poll parser child").is_none() {
        if Instant::now() >= deadline {
            child.kill().expect("kill timed-out parser child");
            child.wait().expect("reap timed-out parser child");
            panic!("parser child exceeded 30 seconds");
        }
        thread::sleep(Duration::from_millis(10));
    }
    let output = child.wait_with_output().expect("parser child output");
    assert!(
        output.status.success(),
        "parser child {test_name}: {}\n{}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("parser-bounds child cases completed")
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("1 passed; 0 failed"),
        "exact parser contract executed"
    );
    print!("{}", String::from_utf8_lossy(&output.stdout));
    eprint!("{}", String::from_utf8_lossy(&output.stderr));
}

#[cfg(target_os = "linux")]
fn guard_page_cases() {
    // SAFETY: sysconf has no pointer arguments or additional preconditions.
    let page_size =
        usize::try_from(unsafe { libc::sysconf(libc::_SC_PAGESIZE) }).expect("positive page size");
    // SAFETY: Request a fresh anonymous two-page mapping with no fixed address.
    let mapping = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            page_size * 2,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
            -1,
            0,
        )
    };
    assert_ne!(mapping, libc::MAP_FAILED);
    let bytes = mapping.cast::<u8>();
    assert_eq!(
        // SAFETY: The second page is page-aligned and entirely inside this mapping.
        unsafe { libc::mprotect(bytes.add(page_size).cast(), page_size, libc::PROT_NONE) },
        0
    );
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    if is_x86_feature_detected!("sse4.1") && is_x86_feature_detected!("ssse3") {
        eprintln!("parser-bounds SSSE3+SSE4.1 active in isolated child");
    } else {
        eprintln!(
            "parser-bounds SIMD skipped: SSSE3+SSE4.1 unavailable; scalar/public still checked"
        );
    }
    // Every slice ends at PROT_NONE; varying lengths also varies starting alignment.
    let cases = guard_page_inputs(|source, expected| {
        assert!(source.len() <= page_size);
        // SAFETY: The nonoverlapping copy fits the first page; the slice lasts until the next copy.
        let input = unsafe {
            let start = bytes.add(page_size - source.len());
            std::ptr::copy_nonoverlapping(source.as_ptr(), start, source.len());
            std::slice::from_raw_parts(start, source.len())
        };
        assert_parser_parity(input, expected);
    });
    // SAFETY: This is the complete still-live mapping returned by mmap above.
    assert_eq!(unsafe { libc::munmap(mapping, page_size * 2) }, 0);
    println!("parser-bounds guard-page cases completed: {cases}");
}
