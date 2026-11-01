// Owned-process PEBS DLA diagnostic. Samples only its own thread.
// usage: pebsdiag <cpu> <mode> <config_hex> <config1> <period> <precise> <u|uk|k> [secs]
#define _GNU_SOURCE
#include <errno.h>
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

#define PAGES 512
#define MAXU 64
static uint64_t ns(void) {
  struct timespec t;
  clock_gettime(CLOCK_MONOTONIC, &t);
  return (uint64_t)t.tv_sec * 1000000000ULL + t.tv_nsec;
}
struct ctr { uint64_t key, n; };
static void bump(struct ctr *c, int *nc, uint64_t k) {
  for (int i = 0; i < *nc; i++) if (c[i].key == k) { c[i].n++; return; }
  if (*nc < MAXU) { c[*nc].key = k; c[*nc].n = 1; (*nc)++; }
  else c[MAXU - 1].n++; // overflow bucket
}
static int cmp(const void *a, const void *b) {
  const struct ctr *x = a, *y = b; return x->n < y->n ? 1 : x->n > y->n ? -1 : 0;
}
static struct ctr addrs[MAXU], ips[MAXU], srcs[MAXU], pages[MAXU];
static int na, ni, ns_, np;
static uint64_t total, inrange, zero, lost, throttle, wsum, other_tid;
static uintptr_t lo[4], hi[4]; static int nr;
static struct perf_event_mmap_page *mp; static char *data; static size_t dsz;
static pid_t mytid;

static void drain(void) {
  uint64_t head = __atomic_load_n(&mp->data_head, __ATOMIC_ACQUIRE), tail = mp->data_tail;
  while (tail < head) {
    struct perf_event_header h; size_t off = tail % dsz;
    char buf[256];
    for (size_t i = 0; i < sizeof h; i++) ((char *)&h)[i] = data[(off + i) % dsz];
    if (h.size > sizeof buf || h.size < sizeof h) { fprintf(stderr, "bad rec %u\n", h.size); exit(3); }
    for (size_t i = 0; i < h.size; i++) buf[i] = data[(off + i) % dsz];
    if (h.type == PERF_RECORD_SAMPLE) {
      uint64_t *p = (uint64_t *)(buf + sizeof h);
      uint64_t ip = p[0]; uint32_t tid = (uint32_t)(p[1] >> 32); uint64_t addr = p[2];
      uint64_t w = p[4], src = p[5];
      total++; if ((pid_t)tid != mytid) other_tid++;
      if (!addr) zero++;
      int in = 0; for (int r = 0; r < nr; r++) if (addr >= lo[r] && addr < hi[r]) in = 1;
      inrange += in; wsum += w;
      bump(addrs, &na, addr); bump(ips, &ni, ip); bump(srcs, &ns_, src); bump(pages, &np, addr >> 12);
    } else if (h.type == PERF_RECORD_LOST) lost += ((uint64_t *)(buf + sizeof h))[1];
    else if (h.type == PERF_RECORD_THROTTLE) throttle++;
    tail += h.size;
  }
  __atomic_store_n(&mp->data_tail, tail, __ATOMIC_RELEASE);
}

static volatile uint64_t bss_arr[2048];
static uint64_t sink;

int main(int argc, char **argv) {
  if (argc < 8) { fprintf(stderr, "usage\n"); return 2; }
  int cpu = atoi(argv[1]); const char *mode = argv[2];
  uint64_t cfg = strtoull(argv[3], 0, 16), cfg1 = strtoull(argv[4], 0, 0), period = strtoull(argv[5], 0, 0);
  int prec = atoi(argv[6]); const char *priv = argv[7]; double secs = argc > 8 ? atof(argv[8]) : 1.0;
  cpu_set_t s; CPU_ZERO(&s); CPU_SET(cpu, &s);
  if (sched_setaffinity(0, sizeof s, &s)) { perror("affinity"); return 2; }
  mytid = (pid_t)syscall(SYS_gettid);

  // workload buffers, all initialised before attach
  size_t small = 16384, big = getenv("CHASE_BYTES") ? strtoull(getenv("CHASE_BYTES"),0,0) : (256UL << 20);
  volatile uint64_t *p = mmap(0, small, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
  for (size_t i = 0; i < small / 8; i++) p[i] = i + 1;
  for (size_t i = 0; i < 2048; i++) bss_arr[i] = i;
  uint64_t *chase = 0; size_t nchase = big / 64;
  char *splitbuf = 0;
  if (!strcmp(mode, "chase") || !strcmp(mode, "mixed")) {
    chase = mmap(0, big, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    // random single-cycle permutation over cache lines (Sattolo)
    uint32_t *perm = malloc(nchase * 4); for (size_t i = 0; i < nchase; i++) perm[i] = i;
    uint64_t x = 88172645463325252ULL;
    for (size_t i = nchase - 1; i > 0; i--) { x ^= x << 13; x ^= x >> 7; x ^= x << 17; size_t j = x % i; uint32_t t = perm[i]; perm[i] = perm[j]; perm[j] = t; }
    for (size_t i = 0; i < nchase; i++) chase[(size_t)perm[i] * 8] = (uint64_t)&chase[(size_t)perm[(i + 1) % nchase] * 8];
    free(perm);
    lo[nr] = (uintptr_t)chase; hi[nr++] = (uintptr_t)chase + big;
    if (!strcmp(mode, "mixed")) { lo[nr] = (uintptr_t)p; hi[nr++] = (uintptr_t)p + small; }
  } else if (!strcmp(mode, "split")) {
    splitbuf = mmap(0, small, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    memset(splitbuf, 1, small);
    lo[nr] = (uintptr_t)splitbuf; hi[nr++] = (uintptr_t)splitbuf + small;
  } else if (!strcmp(mode, "bss")) {
    lo[nr] = (uintptr_t)bss_arr; hi[nr++] = (uintptr_t)(bss_arr + 2048);
  } else {
    lo[nr] = (uintptr_t)p; hi[nr++] = (uintptr_t)p + small;
  }
  volatile uint64_t stackarr[512]; for (int i = 0; i < 512; i++) stackarr[i] = i;
  if (!strcmp(mode, "stack")) { nr = 0; lo[nr] = (uintptr_t)stackarr; hi[nr++] = (uintptr_t)(stackarr + 512); }

  struct perf_event_attr a; memset(&a, 0, sizeof a);
  a.type = PERF_TYPE_RAW; a.size = sizeof a; a.config = cfg; a.config1 = cfg1; a.sample_period = period;
  a.sample_type = PERF_SAMPLE_IP | PERF_SAMPLE_TID | PERF_SAMPLE_ADDR | PERF_SAMPLE_CPU | PERF_SAMPLE_WEIGHT | PERF_SAMPLE_DATA_SRC;
  a.disabled = 1; a.exclude_hv = 1; a.precise_ip = prec; a.wakeup_events = getenv("WAKE0") ? 0 : 1;
  a.exclude_kernel = strchr(priv, 'k') == 0; a.exclude_user = strchr(priv, 'u') == 0;
  int fd = syscall(SYS_perf_event_open, &a, 0, -1, -1, 0);
  if (fd < 0) { printf("RESULT open_failed errno=%d %s\n", errno, strerror(errno)); return 4; }
  long pg = sysconf(_SC_PAGESIZE); dsz = PAGES * pg;
  void *m = mmap(0, (PAGES + 1) * pg, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
  if (m == MAP_FAILED) { perror("mmap"); return 4; }
  mp = m; data = (char *)m + pg;

  uint64_t sum = 0, rounds = 0, t0 = ns(), end = t0 + (uint64_t)(secs * 1e9);
  uint64_t *cur = chase;
  ioctl(fd, PERF_EVENT_IOC_ENABLE, 0);
  do {
    if (!strcmp(mode, "seq")) {
      for (int r = 0; r < 128; r++) for (size_t i = 0; i < small / 8; i++) sum += p[i];
    } else if (!strcmp(mode, "bss")) {
      for (int r = 0; r < 128; r++) for (size_t i = 0; i < 2048; i++) sum += bss_arr[i];
    } else if (!strcmp(mode, "stack")) {
      for (int r = 0; r < 512; r++) for (size_t i = 0; i < 512; i++) sum += stackarr[i];
    } else if (!strcmp(mode, "chase")) {
      for (int r = 0; r < 200000; r++) cur = (uint64_t *)*(volatile uint64_t *)cur;
    } else if (!strcmp(mode, "split")) {
      for (int r = 0; r < 128; r++) for (size_t i = 60; i + 8 < small; i += 64) { uint64_t v; __asm__ volatile("movq (%1), %0" : "=r"(v) : "r"(splitbuf + i) : "memory"); sum += v; }
    } else if (!strcmp(mode, "lock")) {
      for (int r = 0; r < 100000; r++) __atomic_fetch_add((uint64_t *)&p[r & 2047], 1, __ATOMIC_SEQ_CST);
    } else if (!strcmp(mode, "store")) {
      for (int r = 0; r < 128; r++) for (size_t i = 0; i < small / 8; i++) p[i] = sum++;
    } else if (!strcmp(mode, "mixed")) {
      // L1 + L2 + L3 + DRAM-resident loads in one loop (chase over CHASE_BYTES)
      for (int r = 0; r < 2000; r++) { for (size_t i = 0; i < 64; i++) sum += p[i]; cur = (uint64_t *)*(volatile uint64_t *)cur; }
    } else if (!strcmp(mode, "syscall")) {
      for (int r = 0; r < 128; r++) { for (size_t i = 0; i < small / 8; i++) sum += p[i]; syscall(SYS_getppid); }
    } else { fprintf(stderr, "mode?\n"); return 2; }
    rounds++; drain();
  } while (ns() < end);
  ioctl(fd, PERF_EVENT_IOC_DISABLE, 0);
  uint64_t count = 0; if (read(fd, &count, 8) != 8) count = ~0ULL; drain();
  sink = sum + (uint64_t)cur;
  qsort(addrs, na, sizeof *addrs, cmp); qsort(ips, ni, sizeof *ips, cmp); qsort(srcs, ns_, sizeof *srcs, cmp); qsort(pages, np, sizeof *pages, cmp);
  printf("RESULT cpu=%d mode=%s cfg=0x%" PRIx64 " cfg1=%" PRIu64 " period=%" PRIu64 " prec=%d priv=%s count=%" PRIu64
         " rounds=%" PRIu64 " samples=%" PRIu64 " inrange=%" PRIu64 " zero=%" PRIu64 " lost=%" PRIu64 " throttle=%" PRIu64 " othertid=%" PRIu64
         " distinct_addr=%d%s distinct_page=%d mean_weight=%.1f range=0x%" PRIxPTR "-0x%" PRIxPTR "\n",
         cpu, mode, cfg, cfg1, period, prec, priv, count, rounds, total, inrange, zero, lost, throttle, other_tid,
         na, na >= MAXU ? "+" : "", np, total ? (double)wsum / total : 0, lo[0], hi[0]);
  printf("  top_addr:"); for (int i = 0; i < na && i < 6; i++) printf(" 0x%" PRIx64 "x%" PRIu64, addrs[i].key, addrs[i].n); printf("\n");
  printf("  top_ip:  "); for (int i = 0; i < ni && i < 4; i++) printf(" 0x%" PRIx64 "x%" PRIu64, ips[i].key, ips[i].n); printf("\n");
  printf("  top_src: "); for (int i = 0; i < ns_ && i < 4; i++) printf(" 0x%" PRIx64 "x%" PRIu64, srcs[i].key, srcs[i].n); printf("\n");
  return 0;
}
