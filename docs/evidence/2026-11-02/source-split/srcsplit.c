// PEBS load-attribution bias probe for the read/write migration policy.
// Owned process only; samples this thread (user-only, precise 2, PERF_SAMPLE_ADDR)
// exactly like libcram, and reads the kernel's perf rings directly (no BPF).
//
// Four regions with known accesses per step:
//   A: 16 KiB, L1-resident   8 loads, 0 stores   (read-mostly, cache-friendly)
//   B: 64 MiB pointer chase  1 load,  0 stores   (read-only, DRAM)
//   C: 16 KiB, L1-resident   1 load,  2 stores   (write-heavy, cache-friendly)
//   D: 64 MiB pointer chase  1 load,  1 store    (write-heavy, DRAM; the store
//                                                  hits the line just loaded)
// Policy truth for `reads / 4 > writes`: A yes, B yes, C no, D no.
//
// usage: srcsplit <cpu> <steps> <cfg_hex:period> ... (0x82d0 is the store event)
// Each sample is weighted by its event's period. The per-region estimate is
// compared with the exact access count.
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
#define MAXEV 6
#define CHASE (64ul << 20)
enum { RA, RB, RC, RD, ROTHER, NREG };
static const char *names[NREG] = {"A", "B", "C", "D", "other"};
static uint64_t *A, *B, *C, *D;

struct ev {
  uint64_t cfg, period;
  int store, fd;
  struct perf_event_mmap_page *mp;
  char *data;
  size_t dsz;
  uint64_t samples, lost, throttle, region[NREG];
};

static int region_of(uint64_t v) {
  if (v >= (uintptr_t)A && v < (uintptr_t)(A + 2048)) return RA;
  if (v >= (uintptr_t)B && v < (uintptr_t)B + CHASE) return RB;
  if (v >= (uintptr_t)C && v < (uintptr_t)(C + 2048)) return RC;
  if (v >= (uintptr_t)D && v < (uintptr_t)D + CHASE) return RD;
  return ROTHER;
}

static void drain(struct ev *e) {
  uint64_t head = __atomic_load_n(&e->mp->data_head, __ATOMIC_ACQUIRE), tail = e->mp->data_tail;
  while (tail < head) {
    struct perf_event_header h;
    char buf[64];
    size_t off = tail % e->dsz;
    for (size_t i = 0; i < sizeof h; i++) ((char *)&h)[i] = e->data[(off + i) % e->dsz];
    for (size_t i = 0; i < h.size && i < sizeof buf; i++) buf[i] = e->data[(off + i) % e->dsz];
    uint64_t v;
    memcpy(&v, buf + sizeof h, 8);
    if (h.type == PERF_RECORD_SAMPLE) {
      e->samples++;
      e->region[region_of(v)]++;
    } else if (h.type == PERF_RECORD_LOST_SAMPLES) {
      e->lost += v;
    } else if (h.type == PERF_RECORD_THROTTLE) {
      e->throttle++;
    }
    tail += h.size;
  }
  __atomic_store_n(&e->mp->data_tail, tail, __ATOMIC_RELEASE);
}

static uint64_t *chase_cycle(uint64_t seed) {
  uint64_t *m = mmap(0, CHASE, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
  if (m == MAP_FAILED) { perror("mmap"); exit(3); }
  size_t lines = CHASE / 64;
  uint32_t *perm = malloc(lines * sizeof *perm);
  for (size_t i = 0; i < lines; i++) perm[i] = i;
  uint64_t x = seed;
  for (size_t i = lines - 1; i > 0; i--) {  // Sattolo: one cycle over all lines
    x ^= x << 13; x ^= x >> 7; x ^= x << 17;
    size_t j = x % i;
    uint32_t t = perm[i]; perm[i] = perm[j]; perm[j] = t;
  }
  for (size_t i = 0; i < lines; i++) {
    m[perm[i] * 8ul] = (uintptr_t)(m + perm[(i + 1) % lines] * 8ul);
    m[perm[i] * 8ul + 1] = 0;
  }
  free(perm);
  return m;
}

// Every memory access is volatile; locals stay in registers.
static __attribute__((noinline)) uint64_t run(uint64_t steps, uint64_t **pb, uint64_t **pd, uint64_t sum,
                                              uint64_t base) {
  volatile uint64_t *a = A, *c = C;
  uint64_t *b = *pb, *d = *pd;
  for (uint64_t j = base; j < base + steps; j++) {
    for (int k = 0; k < 8; k++) sum += a[(j * 8 + k) & 2047];
    b = (uint64_t *)*(volatile uint64_t *)b;
    d = (uint64_t *)*(volatile uint64_t *)d;
    ((volatile uint64_t *)d)[1] = sum;
    sum += c[j & 2047];
    c[(j + 1) & 2047] = sum;
    c[(j + 2) & 2047] = sum;
  }
  *pb = b;
  *pd = d;
  return sum;
}

int main(int argc, char **argv) {
  if (argc < 4 || argc - 3 > MAXEV) {
    fprintf(stderr, "usage: srcsplit <cpu> <steps> <cfg_hex:period>...\n");
    return 2;
  }
  int cpu = atoi(argv[1]);
  uint64_t steps = strtoull(argv[2], 0, 0);
  int n = argc - 3;
  cpu_set_t s;
  CPU_ZERO(&s);
  CPU_SET(cpu, &s);
  if (sched_setaffinity(0, sizeof s, &s)) { perror("affinity"); return 3; }
  A = aligned_alloc(4096, 16384);
  C = aligned_alloc(4096, 16384);
  memset(A, 0x5a, 16384);
  memset(C, 0x5a, 16384);
  B = chase_cycle(88172645463325252ull);
  D = chase_cycle(0x9e3779b97f4a7c15ull);
  struct ev e[MAXEV];
  memset(e, 0, sizeof e);
  long pg = sysconf(_SC_PAGESIZE);
  for (int i = 0; i < n; i++) {
    char *colon = strchr(argv[3 + i], ':');
    if (!colon) return 2;
    e[i].cfg = strtoull(argv[3 + i], 0, 16);
    e[i].period = strtoull(colon + 1, 0, 0);
    e[i].store = e[i].cfg == 0x82d0;
    struct perf_event_attr at;
    memset(&at, 0, sizeof at);
    at.type = PERF_TYPE_RAW;
    at.size = sizeof at;
    at.config = e[i].cfg;
    at.sample_period = e[i].period;
    at.sample_type = PERF_SAMPLE_ADDR;
    at.read_format = PERF_FORMAT_TOTAL_TIME_ENABLED | PERF_FORMAT_TOTAL_TIME_RUNNING;
    at.disabled = 1;
    at.exclude_kernel = 1;
    at.exclude_hv = 1;
    at.precise_ip = 2;
    at.wakeup_events = 1;
    e[i].fd = syscall(SYS_perf_event_open, &at, 0, -1, -1, 0);
    if (e[i].fd < 0) { perror("perf_event_open"); return 3; }
    void *m = mmap(0, (PAGES + 1) * pg, PROT_READ | PROT_WRITE, MAP_SHARED, e[i].fd, 0);
    if (m == MAP_FAILED) { perror("mmap ring"); return 3; }
    e[i].mp = m;
    e[i].data = (char *)m + pg;
    e[i].dsz = PAGES * pg;
  }
  uint64_t sum = 0, chunk = 1 << 20, done = 0;
  uint64_t *b = B, *d = D;
  for (int i = 0; i < n; i++) ioctl(e[i].fd, PERF_EVENT_IOC_ENABLE, 0);
  while (done < steps) {
    uint64_t now = steps - done < chunk ? steps - done : chunk;
    sum = run(now, &b, &d, sum, done);
    done += now;
    for (int i = 0; i < n; i++) drain(&e[i]);
  }
  for (int i = 0; i < n; i++) ioctl(e[i].fd, PERF_EVENT_IOC_DISABLE, 0);
  double reads[NREG] = {0}, writes[NREG] = {0};
  uint64_t load_count = 0, store_count = 0, multiplexed = 0;
  for (int i = 0; i < n; i++) {
    uint64_t v[3] = {0};
    if (read(e[i].fd, v, sizeof v) != sizeof v) { perror("read"); return 3; }
    drain(&e[i]);
    multiplexed |= v[1] != v[2];
    if (e[i].store) store_count += v[0]; else load_count += v[0];
    printf("EVENT cfg=0x%" PRIx64 " period=%" PRIu64 " count=%" PRIu64 " enabled=%" PRIu64 " running=%" PRIu64
           " samples=%" PRIu64 " expected=%" PRIu64 " lost=%" PRIu64 " throttle=%" PRIu64,
           e[i].cfg, e[i].period, v[0], v[1], v[2], e[i].samples, v[0] / e[i].period, e[i].lost, e[i].throttle);
    for (int r = 0; r < NREG; r++) {
      printf(" %s=%" PRIu64, names[r], e[i].region[r]);
      (e[i].store ? writes : reads)[r] += (double)e[i].region[r] * e[i].period;
    }
    printf("\n");
  }
  const double tr[NREG] = {8, 1, 1, 1, 0}, tw[NREG] = {0, 0, 2, 1, 0};
  printf("TOTAL steps=%" PRIu64 " loads=%" PRIu64 " (%.4f of 11/step) stores=%" PRIu64
         " (%.4f of 3/step) multiplexed=%" PRIu64 " sum=%" PRIu64 "\n",
         steps, load_count, load_count / (11.0 * steps), store_count, store_count / (3.0 * steps), multiplexed, sum);
  for (int r = 0; r < NREG; r++) {
    double er = reads[r] / steps, ew = writes[r] / steps;
    int truth = r < ROTHER && tr[r] / 4 > tw[r], verdict = er / 4 > ew;
    printf("REGION %s reads/step est=%.3f true=%.0f writes/step est=%.3f true=%.0f read_dominant est=%s true=%s%s\n",
           names[r], er, tr[r], ew, tw[r], verdict ? "yes" : "no", r < ROTHER ? (truth ? "yes" : "no") : "-",
           r < ROTHER && verdict != truth ? "  MISCLASSIFIED" : "");
  }
  return 0;
}
