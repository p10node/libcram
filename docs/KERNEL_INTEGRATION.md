# Kernel and hardware integration notes

The SDK's NUMA path uses ordinary Linux userspace memory-policy interfaces.
Those interfaces cannot make private kernel-only device memory available to
userspace. CRAM availability must be established for the actual driver/kernel.

A relevant upstream design discussion is the January 8, 2026 RFC v3,
"mm,numa: N_PRIVATE node isolation for device-managed memory":
https://lwn.net/Articles/1053508/
(original series: https://lwn.net/ml/all/20260108203755.1163107-1-gourry@gourry.net/).
It explicitly distinguishes compressed RAM from ordinary CXL, describes private
nodes excluded from general allocation, and says its compression-capacity
validation needs device-specific counters/interrupts. It is an RFC, not proof
that a stable userspace CRAM sysfs ABI or accessible CRAM node exists everywhere.

Consequently:

- A CXL attribute is transport/topology metadata, not proof of compression.
- Any compression flag understood by this SDK must match the deployment's
  documented adapter convention; do not assume it is a universal Linux ABI.
- A private node unavailable through mbind/move_pages is unsupported by this
  userspace backend, even when the kernel can use it internally.
- Capacity and poison observations need a real device-specific integration.
- Tests with a mock sysfs tree prove parser behavior, not compatibility with an
  unspecified device or experimental kernel patch series.

For PMU sampling, consult perf_event_open(2):
https://man7.org/linux/man-pages/man2/perf_event_open.2.html
`PERF_SAMPLE_DATA_SRC` and precise data addresses require hardware support;
raw event encodings must be selected for the actual CPU. Kernel permission or
virtualization limitations must remain explicit errors, not synthetic samples.
