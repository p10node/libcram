// Kernel-enforced PEBS sample budget probe (PERF_EVENT_IOC_REFRESH). Owned
// process only; user-only, precise 2, PERF_SAMPLE_ADDR, like libcram.
// The event is armed with IOC_REFRESH(budget) instead of IOC_ENABLE: the
// kernel disables it after `budget` overflows. Each interval runs an
// L1-streaming burst far above the budget, counts the samples delivered, checks
// that the event stopped (time_enabled frozen), and re-arms only after it
// stopped, so the remaining budget can never exceed one grant.
// usage: budget <cpu> <cfg_hex> <period> <budget> <intervals> <interval_ms>
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

#define PAGES 64
static volatile uint64_t rd[2048], wr[2048];

static uint64_t now_ns(void) {
  struct timespec t;
  clock_gettime(CLOCK_MONOTONIC, &t);
  return t.tv_sec * 1000000000ull + t.tv_nsec;
}

int main(int argc, char **argv) {
  if (argc != 7) return 2;
  int cpu = atoi(argv[1]);
  uint64_t cfg = strtoull(argv[2], 0, 16), period = strtoull(argv[3], 0, 0);
  int budget = atoi(argv[4]), intervals = atoi(argv[5]);
  uint64_t interval_ns = strtoull(argv[6], 0, 0) * 1000000ull;
  cpu_set_t s;
  CPU_ZERO(&s);
  CPU_SET(cpu, &s);
  if (sched_setaffinity(0, sizeof s, &s)) return 3;
  struct perf_event_attr a;
  memset(&a, 0, sizeof a);
  a.type = PERF_TYPE_RAW;
  a.size = sizeof a;
  a.config = cfg;
  a.sample_period = period;
  a.sample_type = PERF_SAMPLE_ADDR;
  a.read_format = PERF_FORMAT_TOTAL_TIME_ENABLED | PERF_FORMAT_TOTAL_TIME_RUNNING;
  a.disabled = 1;
  a.exclude_kernel = 1;
  a.exclude_hv = 1;
  a.precise_ip = 2;
  a.wakeup_events = 1;
  int fd = syscall(SYS_perf_event_open, &a, 0, -1, -1, 0);
  if (fd < 0) { perror("perf_event_open"); return 3; }
  long pg = sysconf(_SC_PAGESIZE);
  struct perf_event_mmap_page *mp = mmap(0, (PAGES + 1) * pg, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
  if (mp == MAP_FAILED) { perror("mmap"); return 3; }
  char *data = (char *)mp + pg;
  size_t dsz = PAGES * pg;
  uint64_t tail = 0, sum = 0, worst = 0, rearms = 0;
  if (ioctl(fd, PERF_EVENT_IOC_REFRESH, budget)) { perror("IOC_REFRESH"); return 3; }
  for (int k = 0; k < intervals; k++) {
    uint64_t start = now_ns(), loads = 0;
    while (now_ns() - start < interval_ns) {  // burst: L1 streaming loop
      for (int i = 0; i < 4096; i++) { sum += rd[i & 2047]; wr[i & 2047] = sum; }
      loads += 4096;
    }
    uint64_t head = __atomic_load_n(&mp->data_head, __ATOMIC_ACQUIRE), samples = 0, lost = 0;
    while (tail < head) {
      struct perf_event_header h;
      for (size_t i = 0; i < sizeof h; i++) ((char *)&h)[i] = data[(tail % dsz + i) % dsz];
      if (h.type == PERF_RECORD_SAMPLE) samples++;
      if (h.type == PERF_RECORD_LOST_SAMPLES) lost++;
      tail += h.size;
    }
    __atomic_store_n(&mp->data_tail, tail, __ATOMIC_RELEASE);
    uint64_t v1[3], v2[3];
    if (read(fd, v1, sizeof v1) != sizeof v1) return 3;
    usleep(2000);  // time_enabled advances only while the event is enabled
    if (read(fd, v2, sizeof v2) != sizeof v2) return 3;
    int stopped = v2[1] == v1[1];
    if (samples > worst) worst = samples;
    printf("interval=%d loads=%" PRIu64 " unbudgeted_samples=%" PRIu64 " samples=%" PRIu64 " lost=%" PRIu64
           " stopped=%d\n", k, loads, loads / period, samples, lost, stopped);
    if (stopped) {
      if (ioctl(fd, PERF_EVENT_IOC_REFRESH, budget)) { perror("IOC_REFRESH"); return 3; }
      rearms++;
    }
  }
  printf("RESULT cfg=0x%" PRIx64 " period=%" PRIu64 " budget=%d worst_interval_samples=%" PRIu64
         " rearms=%" PRIu64 " %s sum=%" PRIu64 "\n",
         cfg, period, budget, worst, rearms, worst <= (uint64_t)budget ? "BUDGET_HELD" : "BUDGET_EXCEEDED", sum);
  return worst <= (uint64_t)budget ? 0 : 1;
}
