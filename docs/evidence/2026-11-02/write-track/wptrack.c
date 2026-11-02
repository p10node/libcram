// Sampled write tracking with userfaultfd asynchronous write-protect.
// Owned process only, no PMU. Each interval write-protects a random subset of
// pages (UFFDIO_WRITEPROTECT) and, at the end of the interval, reads the
// uffd-wp bit (bit 57) from /proc/self/pagemap: a page that lost it was written.
// With UFFD_FEATURE_WP_ASYNC the kernel resolves the write fault itself; the
// process sees no signal and no userfault message.
//
// Regions (same access pattern per step as ../source-split/srcsplit.c, without
// PEBS): A 16 KiB 8 loads; B 64 MiB chase 1 load; C 16 KiB 1 load + 2 stores;
// D 64 MiB chase 1 load + 1 store. Truth: A, B never written; every page of C
// written every interval; D pages written at the chase rate.
//
// usage: wptrack <cpu> <intervals> <interval_ms> <sampled_pages_per_interval>
// A sample size of 0 runs the workload untracked (throughput baseline).
#define _GNU_SOURCE
#include <fcntl.h>
#include <inttypes.h>
#include <linux/userfaultfd.h>
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

#define PAGE 4096ul
#define SMALL 16384ul
#define CHASE (64ul << 20)
enum { RA, RB, RC, RD, NREG };
static const char *names[NREG] = {"A", "B", "C", "D"};
static uint64_t *base[NREG];
static const size_t sizes[NREG] = {SMALL, CHASE, SMALL, CHASE};
static int uffd, pagemap;

static uint64_t now_ns(void) {
  struct timespec t;
  clock_gettime(CLOCK_MONOTONIC, &t);
  return t.tv_sec * 1000000000ull + t.tv_nsec;
}
static uint64_t rng = 0x9e3779b97f4a7c15ull;
static uint64_t next(void) { rng ^= rng << 13; rng ^= rng >> 7; rng ^= rng << 17; return rng; }

static uint64_t *region(size_t size) {
  uint64_t *m = mmap(0, size, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
  if (m == MAP_FAILED) { perror("mmap"); exit(3); }
  memset(m, 0x5a, size);
  return m;
}
static void chase_cycle(uint64_t *m, uint64_t seed) {
  size_t lines = CHASE / 64;
  uint32_t *perm = malloc(lines * sizeof *perm);
  for (size_t i = 0; i < lines; i++) perm[i] = i;
  uint64_t x = seed;
  for (size_t i = lines - 1; i > 0; i--) {
    x ^= x << 13; x ^= x >> 7; x ^= x << 17;
    size_t j = x % i;
    uint32_t t = perm[i]; perm[i] = perm[j]; perm[j] = t;
  }
  for (size_t i = 0; i < lines; i++) m[perm[i] * 8ul] = (uintptr_t)(m + perm[(i + 1) % lines] * 8ul);
  free(perm);
}
static void protect(void *page, int on) {
  struct uffdio_writeprotect wp = {
      .range = {.start = (uintptr_t)page, .len = PAGE},
      .mode = on ? UFFDIO_WRITEPROTECT_MODE_WP : 0};
  if (ioctl(uffd, UFFDIO_WRITEPROTECT, &wp)) { perror("UFFDIO_WRITEPROTECT"); exit(3); }
}
static int still_protected(void *page) {
  uint64_t entry;
  if (pread(pagemap, &entry, 8, (uintptr_t)page / PAGE * 8) != 8) { perror("pagemap"); exit(3); }
  return (entry >> 57) & 1;
}

static __attribute__((noinline)) uint64_t run(uint64_t steps, uint64_t **pb, uint64_t **pd, uint64_t sum,
                                              uint64_t j0) {
  volatile uint64_t *a = base[RA], *c = base[RC];
  uint64_t *b = *pb, *d = *pd;
  for (uint64_t j = j0; j < j0 + steps; j++) {
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
  if (argc != 5) return 2;
  int cpu = atoi(argv[1]), intervals = atoi(argv[2]);
  uint64_t interval_ns = strtoull(argv[3], 0, 0) * 1000000ull;
  int k = atoi(argv[4]);
  cpu_set_t s;
  CPU_ZERO(&s);
  CPU_SET(cpu, &s);
  if (sched_setaffinity(0, sizeof s, &s)) return 3;
  for (int r = 0; r < NREG; r++) base[r] = region(sizes[r]);
  chase_cycle(base[RB], 88172645463325252ull);
  chase_cycle(base[RD], 0x9e3779b97f4a7c15ull);
  size_t pages[NREG];
  for (int r = 0; r < NREG; r++) pages[r] = sizes[r] / PAGE;
  if (k) {
    uffd = syscall(SYS_userfaultfd, O_CLOEXEC | O_NONBLOCK | UFFD_USER_MODE_ONLY);
    if (uffd < 0) { perror("userfaultfd"); return 3; }
    struct uffdio_api api = {.api = UFFD_API, .features = UFFD_FEATURE_WP_ASYNC | UFFD_FEATURE_WP_UNPOPULATED};
    if (ioctl(uffd, UFFDIO_API, &api)) { perror("UFFDIO_API"); return 3; }
    for (int r = 0; r < NREG; r++) {
      struct uffdio_register reg = {.range = {.start = (uintptr_t)base[r], .len = sizes[r]},
                                    .mode = UFFDIO_REGISTER_MODE_WP};
      if (ioctl(uffd, UFFDIO_REGISTER, &reg)) { perror("UFFDIO_REGISTER"); return 3; }
    }
    pagemap = open("/proc/self/pagemap", O_RDONLY);
    if (pagemap < 0) { perror("pagemap"); return 3; }
  }
  char **sample = calloc(k ? k : 1, sizeof *sample);
  int *owner = calloc(k ? k : 1, sizeof *owner);
  uint64_t sampled[NREG] = {0}, written[NREG] = {0}, steps = 0, sum = 0, tracking_ns = 0;
  uint64_t *b = base[RB], *d = base[RD];
  uint64_t start_all = now_ns();
  for (int it = 0; it < intervals; it++) {
    uint64_t t0 = now_ns();
    int n = 0;  // stratified, without replacement: equal share per region
    for (int i = 0; i < k; i++) {
      int r = i % NREG;
      if ((size_t)(i / NREG) >= pages[r]) continue;  // region smaller than its share
      char *page;
      int fresh;
      do {
        page = (char *)base[r] + next() % pages[r] * PAGE;
        fresh = 1;
        for (int j = 0; j < n; j++) fresh &= sample[j] != page;
      } while (!fresh);
      owner[n] = r;
      sample[n++] = page;
      protect(page, 1);
    }
    uint64_t t1 = now_ns();
    while (now_ns() - t1 < interval_ns) {
      sum = run(1 << 14, &b, &d, sum, steps);
      steps += 1 << 14;
    }
    uint64_t t2 = now_ns();
    for (int i = 0; i < n; i++) {
      int r = owner[i];
      sampled[r]++;
      if (!still_protected(sample[i])) written[r]++;
      else protect(sample[i], 0);
    }
    tracking_ns += (t1 - t0) + (now_ns() - t2);
  }
  double wall = (now_ns() - start_all) / 1e9;
  printf("RESULT k=%d intervals=%d interval_ms=%" PRIu64 " steps_per_s=%.0f tracking_cpu=%.4f%% sum=%" PRIu64 "\n",
         k, intervals, interval_ns / 1000000, steps / wall, 100.0 * tracking_ns / 1e9 / wall, sum);
  for (int r = 0; r < NREG && k; r++)
    printf("REGION %s pages=%zu sampled=%" PRIu64 " written=%" PRIu64 " written_fraction=%.4f\n", names[r], pages[r],
           sampled[r], written[r], sampled[r] ? (double)written[r] / sampled[r] : 0.0);
  return 0;
}
