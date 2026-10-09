// Verifier-load acceptance only: this probe does not attach a PMU or claim PEBS accuracy.
#[cfg(target_os = "linux")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args().nth(1).unwrap_or_else(|| {
        "crates/libcram-ebpf/target/bpfel-unknown-none/release/libcram-pebs".into()
    });
    let bytes = std::fs::read(path)?;
    let mut bpf = aya::Ebpf::load(&bytes)?;
    for name in ["sample_load", "sample_store"] {
        let program: &mut aya::programs::PerfEvent = bpf
            .program_mut(name)
            .ok_or("missing perf-event program")?
            .try_into()?;
        program.load()?;
        println!("{name}: verifier load PASS (PMU attachment untested)");
    }
    Ok(())
}
#[cfg(not(target_os = "linux"))]
fn main() -> std::process::ExitCode {
    eprintln!("UNSUPPORTED: BPF verifier requires Linux; no hardware acceptance performed");
    std::process::ExitCode::from(2)
}
