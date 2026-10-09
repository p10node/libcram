//! Pressure and bounded sampled access telemetry.
#![deny(clippy::unwrap_used)]
use std::io;
pub mod access;
#[cfg(all(target_os = "linux", feature = "ebpf"))]
pub mod pebs;
#[cfg(any(test, all(target_os = "linux", feature = "ebpf")))]
pub mod process_pebs;
pub mod sampling;
#[derive(Default, Debug, Clone, Copy, PartialEq)]
pub struct PsiLine {
    pub avg10: f64,
    pub avg60: f64,
    pub avg300: f64,
    pub total: u64,
}
#[derive(Default, Debug, Clone, Copy, PartialEq)]
pub struct PsiSample {
    pub some: PsiLine,
    pub full: PsiLine,
}
fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "malformed memory PSI")
}
pub fn parse_memory_pressure(text: &str) -> io::Result<PsiSample> {
    let mut some = None;
    let mut full = None;
    for line in text.lines() {
        let mut words = line.split_whitespace();
        let dest = match words.next() {
            Some("some") => &mut some,
            Some("full") => &mut full,
            _ => return Err(invalid()),
        };
        if dest.is_some() {
            return Err(invalid());
        }
        let mut value = PsiLine::default();
        let mut seen = 0u8;
        for word in words {
            let (key, v) = word.split_once('=').ok_or_else(invalid)?;
            let bit = match key {
                "avg10" => 1,
                "avg60" => 2,
                "avg300" => 4,
                "total" => 8,
                _ => return Err(invalid()),
            };
            if seen & bit != 0 {
                return Err(invalid());
            }
            seen |= bit;
            if bit == 8 {
                if v.is_empty() || !v.bytes().all(|b| b.is_ascii_digit()) {
                    return Err(invalid());
                }
                value.total = v.parse().map_err(|_| invalid())?;
            } else {
                if v.is_empty() || !v.bytes().all(|b| b.is_ascii_digit() || b == b'.') {
                    return Err(invalid());
                }
                let n: f64 = v.parse().map_err(|_| invalid())?;
                if !n.is_finite() || !(0.0..=100.0).contains(&n) {
                    return Err(invalid());
                }
                match bit {
                    1 => value.avg10 = n,
                    2 => value.avg60 = n,
                    _ => value.avg300 = n,
                }
            }
        }
        if seen != 15 {
            return Err(invalid());
        }
        *dest = Some(value);
    }
    Ok(PsiSample {
        some: some.ok_or_else(invalid)?,
        full: full.ok_or_else(invalid)?,
    })
}
/// Read Linux system memory PSI; unsupported platforms return an explicit error.
pub fn read_memory_pressure() -> io::Result<PsiSample> {
    #[cfg(target_os = "linux")]
    {
        parse_memory_pressure(&std::fs::read_to_string("/proc/pressure/memory")?)
    }
    #[cfg(not(target_os = "linux"))]
    {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "memory PSI requires Linux",
        ))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn psi() {
        let s = parse_memory_pressure("some avg10=1.25 avg60=2.50 avg300=3.75 total=42\nfull avg10=0.00 avg60=0.01 avg300=0.02 total=2\n").expect("valid PSI");
        assert_eq!(s.some.avg10, 1.25);
        assert_eq!(s.full.total, 2);
        for bad in [
            "",
            "some avg10=NaN avg60=0 avg300=0 total=0",
            "some avg10=101 avg60=0 avg300=0 total=0",
            "some avg10=0 avg60=0 avg300=0 total=-1",
            "some avg10=0 avg10=1 avg300=0 total=0",
            "some avg10=0 avg60=0 avg300=0 total=0\nsome avg10=0 avg60=0 avg300=0 total=0",
        ] {
            assert!(parse_memory_pressure(bad).is_err(), "{bad}");
        }
    }
}
