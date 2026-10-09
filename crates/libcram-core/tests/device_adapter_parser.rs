use libcram_core::device_adapter::parse_report;

#[test]
fn rejects_invalid_bounds_and_freshness() {
    for bad in [
        "CRAM-PHYSICAL-V1 0 990 1 2 0",
        "CRAM-PHYSICAL-V1 1 990 0 0 0",
        "CRAM-PHYSICAL-V1 1 990 3 2 0",
        "CRAM-PHYSICAL-V1 1 1001 1 2 0",
        "CRAM-PHYSICAL-V1 1 0 1 2 0",
        "CRAM-PHYSICAL-V1 1 990 1 2 0 extra",
        "CRAM-PHYSICAL-V1 +1 990 1 2 0",
        "CRAM-PHYSICAL-V1 1 990 1 2 257",
        "CRAM-PHYSICAL-V1 1 990 1 2 1",
        "CRAM-PHYSICAL-V1 1 990 1 2 0 5",
        "CRAM-PHYSICAL-V1 1 990 18446744073709551616 2 0",
        "OTHER 1 990 1 2 0",
    ] {
        assert!(
            parse_report(bad.as_bytes(), 1000).is_err(),
            "accepted {bad}"
        );
    }
    assert!(parse_report(b"CRAM-PHYSICAL-V1 1 1 1 2 0", 1002).is_err());
    assert!(parse_report(b"CRAM-PHYSICAL-V1 1 1 1 2 0", 1001).is_ok());
    assert!(parse_report(&[b' '; 16385], 1000).is_err());
    assert!(parse_report(&[255], 1000).is_err());
    let mut exact = b"CRAM-PHYSICAL-V1 1 990 1 2 0".to_vec();
    exact.resize(16384, b' ');
    assert_eq!(
        parse_report(&exact, 1000)
            .expect("exact byte budget")
            .reading
            .used_bytes,
        1
    );
    exact.push(b' ');
    assert!(parse_report(&exact, 1000).is_err(), "one byte over budget");
    let maximum = format!(
        "CRAM-PHYSICAL-V1 1 990 0 18446744073709551615 256 {}",
        vec!["18446744073709551615"; 256].join(" ")
    );
    assert_eq!(
        parse_report(maximum.as_bytes(), 1000)
            .expect("bounded maximum")
            .reading
            .poison_pages
            .len(),
        256
    );
}

#[test]
fn preserves_distinct_physical_snapshots() {
    let a = parse_report(b"CRAM-PHYSICAL-V1 7 990 123 4096 2 12 98", 1000).expect("valid a");
    let b = parse_report(b"CRAM-PHYSICAL-V1 19 995 456 8192 1 55", 1000).expect("valid b");
    assert_eq!(
        (
            a.generation,
            a.observed_unix_ms,
            a.reading.used_bytes,
            a.reading.capacity_bytes
        ),
        (7, 990, 123, 4096)
    );
    assert_eq!(a.reading.poison_pages, vec![12, 98]);
    assert_eq!(
        (
            b.generation,
            b.observed_unix_ms,
            b.reading.used_bytes,
            b.reading.capacity_bytes
        ),
        (19, 995, 456, 8192)
    );
    assert_eq!(b.reading.poison_pages, vec![55]);
}
