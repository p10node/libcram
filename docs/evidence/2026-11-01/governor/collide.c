// PEBS fmt2 two-event collision probe. Owned process only.
// Opens <load_cfg> (period P1) and ALL_STORES 0x82d0 (period P2) exactly like
// libcram (user-only, precise 2, PERF_SAMPLE_ADDR, wakeup_events=1), enables
// both immediately before a 1-load + 1-store lockstep loop, then counts the
// kernel's own SAMPLE / LOST_SAMPLES / THROTTLE records per event.
// usage: collide <cpu> <load_cfg_hex> <P1> <P2> <iters>
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
#include <unistd.h>
#define PAGES 256
static volatile uint64_t rd[2048], wr[2048];
struct ring { int fd; struct perf_event_mmap_page *mp; char *data; size_t dsz; uint64_t samples, lost, throttle, inrange; };
static int open_ev(uint64_t cfg, uint64_t period) {
  struct perf_event_attr a; memset(&a, 0, sizeof a);
  a.type = PERF_TYPE_RAW; a.size = sizeof a; a.config = cfg; a.sample_period = period;
  a.sample_type = PERF_SAMPLE_ADDR; a.disabled = 1; a.exclude_kernel = 1; a.exclude_hv = 1;
  a.precise_ip = 2; a.wakeup_events = 1;
  return syscall(SYS_perf_event_open, &a, 0, -1, -1, 0);
}
static void drain(struct ring *r) {
  uint64_t head = __atomic_load_n(&r->mp->data_head, __ATOMIC_ACQUIRE), tail = r->mp->data_tail;
  while (tail < head) {
    struct perf_event_header h; char buf[64]; size_t off = tail % r->dsz;
    for (size_t i = 0; i < sizeof h; i++) ((char *)&h)[i] = r->data[(off + i) % r->dsz];
    for (size_t i = 0; i < h.size && i < sizeof buf; i++) buf[i] = r->data[(off + i) % r->dsz];
    uint64_t v = *(uint64_t *)(buf + sizeof h);
    if (h.type == PERF_RECORD_SAMPLE) { r->samples++; if ((v >= (uintptr_t)rd && v < (uintptr_t)(rd + 2048)) || (v >= (uintptr_t)wr && v < (uintptr_t)(wr + 2048))) r->inrange++; }
    else if (h.type == PERF_RECORD_LOST_SAMPLES) r->lost += v;
    else if (h.type == PERF_RECORD_THROTTLE) r->throttle++;
    tail += h.size;
  }
  __atomic_store_n(&r->mp->data_tail, tail, __ATOMIC_RELEASE);
}
int main(int argc, char **argv) {
  if (argc < 6) return 2;
  int drain_each = getenv("DRAIN") != 0;
  int cpu = atoi(argv[1]); uint64_t cfg = strtoull(argv[2], 0, 16), p1 = strtoull(argv[3], 0, 0), p2 = strtoull(argv[4], 0, 0), n = strtoull(argv[5], 0, 0);
  cpu_set_t s; CPU_ZERO(&s); CPU_SET(cpu, &s); sched_setaffinity(0, sizeof s, &s);
  struct ring r[2] = {{0}}; uint64_t cfgs[2] = {cfg, 0x82d0}, per[2] = {p1, p2};
  long pg = sysconf(_SC_PAGESIZE);
  for (int i = 0; i < 2; i++) {
    r[i].fd = open_ev(cfgs[i], per[i]); if (r[i].fd < 0) { perror("open"); return 3; }
    void *m = mmap(0, (PAGES + 1) * pg, PROT_READ | PROT_WRITE, MAP_SHARED, r[i].fd, 0);
    if (m == MAP_FAILED) { perror("mmap"); return 3; }
    r[i].mp = m; r[i].data = (char *)m + pg; r[i].dsz = PAGES * pg;
  }
  uint64_t sum = 0, chunk = 1 << 24;
  ioctl(r[0].fd, PERF_EVENT_IOC_ENABLE, 0); ioctl(r[1].fd, PERF_EVENT_IOC_ENABLE, 0);
  for (uint64_t done = 0; done < n; done += chunk) {
    for (uint64_t i = 0; i < chunk; i++) { sum += rd[i & 2047]; wr[i & 2047] = sum; }
    if (drain_each) { drain(&r[0]); drain(&r[1]); }
  }
  ioctl(r[0].fd, PERF_EVENT_IOC_DISABLE, 0); ioctl(r[1].fd, PERF_EVENT_IOC_DISABLE, 0);
  uint64_t c[2]; for (int i = 0; i < 2; i++) { if (read(r[i].fd, &c[i], 8) != 8) c[i] = 0; drain(&r[i]); }
  printf("RESULT load=0x%" PRIx64 " P1=%" PRIu64 " P2=%" PRIu64 " counts=%" PRIu64 ",%" PRIu64 " expected=%" PRIu64 ",%" PRIu64
         " samples=%" PRIu64 ",%" PRIu64 " lost_samples=%" PRIu64 ",%" PRIu64 " throttle=%" PRIu64 ",%" PRIu64 " inrange=%" PRIu64 ",%" PRIu64 " sum=%" PRIu64 "\n",
         cfg, p1, p2, c[0], c[1], c[0] / p1, c[1] / p2, r[0].samples, r[1].samples, r[0].lost, r[1].lost, r[0].throttle, r[1].throttle, r[0].inrange, r[1].inrange, sum);
  return 0;
}
