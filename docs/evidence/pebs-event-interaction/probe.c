#define _GNU_SOURCE
#include <errno.h>
#include <inttypes.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/mman.h>
#include <time.h>
#include <unistd.h>

static uint64_t ns(void) {
    struct timespec t;
    if (clock_gettime(CLOCK_MONOTONIC, &t)) exit(2);
    return (uint64_t)t.tv_sec * 1000000000ULL + (uint64_t)t.tv_nsec;
}
int main(void) {
    const size_t bytes=16384;
    volatile uint64_t *p=mmap(NULL,bytes,PROT_READ|PROT_WRITE,MAP_PRIVATE|MAP_ANONYMOUS,-1,0);
    if (p==MAP_FAILED) return 2;
    for (size_t i=0;i<bytes/8;i++) p[i]=i+1;
    printf("OWNED_READ_RANGE 0x%" PRIxPTR " 0x%" PRIxPTR " PID %ld\n",(uintptr_t)p,(uintptr_t)p+bytes,(long)getpid());
    fflush(stdout);
    uint64_t sum=0, rounds=0, end=ns()+2000000000ULL;
    do {
        for (int repeat=0;repeat<128;repeat++) {
            for (size_t i=0;i<bytes/8;i++) sum+=p[i];
        }
        rounds++;
    } while(ns()<end);
    printf("OWNED_DONE sum=%" PRIu64 " rounds=%" PRIu64 "\n",sum,rounds);
    if(munmap((void*)p,bytes)) return 2;
    return 0;
}
