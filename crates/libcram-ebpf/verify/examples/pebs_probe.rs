#[cfg(target_os="linux")]
fn main() {
 let bytes=std::fs::read("crates/libcram-ebpf/target/bpfel-unknown-none/release/libcram-pebs").expect("BPF object");
 // Invalid config must fail before loading BPF or opening perf events.
 let bad=libcram_profiling_check::telemetry::sampling::ExplicitPebsConfig{pmu_type:1,config:1,config1:0,period:1000,kind:libcram_profiling_check::telemetry::access::AccessKind::Read};
 let error=libcram_profiling_check::telemetry::pebs::PebsSampler::attach(&bytes,std::process::id(),0,&[bad]).err().expect("reject software PMU");
 println!("software PMU rejected: {error}");
 // Loading only: this does not attach any PMU event or assert PEBS availability.
 match aya::Ebpf::load(&bytes) {
  Ok(mut bpf)=> {
   for name in ["sample_load","sample_store"] {
    let p:&mut aya::programs::PerfEvent=bpf.program_mut(name).expect("program").try_into().expect("perf event");
    println!("{name} verifier load: {:?}",p.load());
   }
  },
  Err(e)=>println!("BPF load capability result: {e:?}"),
 }
}

#[cfg(not(target_os="linux"))]
fn main() { eprintln!("PEBS requires Linux Intel x86_64"); }
