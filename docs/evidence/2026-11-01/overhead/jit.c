// Per-sample stolen-time of PEBS sampling WITHOUT BPF (kernel-only baseline).
// Same loop shape as pebs_overhead jitter mode: 1 load + 1 store + RDTSC/iter.
// usage: jit <cpu> <period> <pairs> <iters> <none|nomap|mmap>
// kinds per pair (ABBA): base vs sampled(kind). Events: L1_HIT:u + ALL_STORES:u, precise 2.
#define _GNU_SOURCE
#include <inttypes.h>
#include <linux/perf_event.h>
#include <sched.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/mman.h>
#include <sys/syscall.h>
#include <time.h>
#include <unistd.h>
#include <x86intrin.h>

static uint64_t clk(clockid_t c) { struct timespec t; clock_gettime(c, &t); return t.tv_sec * 1000000000ULL + t.tv_nsec; }
static volatile uint64_t *rd, *wr;
static uint64_t thr;
struct res { uint64_t gaps, gap_tsc, cpu, sum; };
static struct res run(uint64_t iters) {
  struct res r = {0}; uint64_t prev = __rdtsc(), c0 = clk(CLOCK_THREAD_CPUTIME_ID);
  for (uint64_t n = 0; n < iters; n++) {
    size_t i = n & 2047; r.sum += rd[i]; wr[i] = r.sum;
    uint64_t now = __rdtsc(), d = now - prev; prev = now;
    if (d >= thr && d < 64000 * 2.095) { r.gaps++; r.gap_tsc += d; }
  }
  r.cpu = clk(CLOCK_THREAD_CPUTIME_ID) - c0; return r;
}
static int open_ev(uint64_t cfg, uint64_t period) {
  struct perf_event_attr a; memset(&a, 0, sizeof a);
  a.type = PERF_TYPE_RAW; a.size = sizeof a; a.config = cfg; a.sample_period = period;
  a.sample_type = PERF_SAMPLE_ADDR; a.disabled = 1; a.exclude_kernel = 1; a.exclude_hv = 1;
  a.precise_ip = 2; a.wakeup_events = 1;
  return syscall(SYS_perf_event_open, &a, 0, -1, -1, 0);
}
int main(int argc, char **argv) {
  int cpu = atoi(argv[1]); uint64_t period = strtoull(argv[2], 0, 0); int pairs = atoi(argv[3]);
  uint64_t iters = strtoull(argv[4], 0, 0); const char *kind = argv[5];
  cpu_set_t s; CPU_ZERO(&s); CPU_SET(cpu, &s); sched_setaffinity(0, sizeof s, &s);
  rd = mmap(0, 16384, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
  wr = mmap(0, 16384, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
  memset((void *)rd, 0x5a, 16384); memset((void *)wr, 0x5a, 16384);
  uint64_t c0 = __rdtsc(), t0 = clk(CLOCK_MONOTONIC); usleep(200000);
  double tpn = (double)(__rdtsc() - c0) / (clk(CLOCK_MONOTONIC) - t0); thr = 64 * tpn;
  printf("CONFIG jit cpu=%d period=%" PRIu64 " kind=%s tsc_per_ns=%.6f\n", cpu, period, kind, tpn);
  run(iters);
  for (int p = 0; p < pairs; p++) {
    for (int k = 0; k < 2; k++) {
      int sampled = (p & 1) ? !k : k;
      int fds[2] = {-1, -1}; void *maps[2] = {0, 0};
      if (sampled && strcmp(kind, "none")) {
        fds[0] = open_ev(0x01d1, period); fds[1] = open_ev(0x82d0, period);
        if (fds[0] < 0 || fds[1] < 0) { perror("perf_event_open"); return 3; }
        if (!strcmp(kind, "mmap")) for (int j = 0; j < 2; j++) maps[j] = mmap(0, 65 * 4096, PROT_READ | PROT_WRITE, MAP_SHARED, fds[j], 0);
        for (int j = 0; j < 2; j++) ioctl(fds[j], PERF_EVENT_IOC_ENABLE, 0);
      }
      struct res r = run(iters);
      uint64_t cnt[2] = {0, 0};
      for (int j = 0; j < 2; j++) if (fds[j] >= 0) {
        ioctl(fds[j], PERF_EVENT_IOC_DISABLE, 0);
        if (read(fds[j], &cnt[j], 8) != 8) cnt[j] = 0;
        if (maps[j]) munmap(maps[j], 65 * 4096);
        close(fds[j]);
      }
      uint64_t samples = cnt[0] / period + cnt[1] / period;
      printf("JBLOCK pair=%d kind=%s cpu_ns=%" PRIu64 " gaps=%" PRIu64 " gap_ns=%.0f samples=%" PRIu64 " counts=%" PRIu64 ",%" PRIu64 " sum=%" PRIu64 "\n",
             p, sampled ? "sampled" : "base", r.cpu, r.gaps, r.gap_tsc / tpn, samples, cnt[0], cnt[1], r.sum);
    }
  }
  return 0;
}
