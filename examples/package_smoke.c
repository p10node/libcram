#include <libcram.h>
#include <assert.h>
#include <stdint.h>
#include <stdio.h>
#include "package_probe.h"
int main(void) {
    assert(cram_init() == CRAM_OK);
    assert_sdk_library();
    unsigned char *a = cram_alloc(4096, 64, CRAM_WRITE_HEAVY);
    unsigned char *b = cram_alloc(8192, 128, CRAM_AUTO);
    assert(a && b && (uintptr_t)a % 64 == 0 && (uintptr_t)b % 128 == 0);
    for (size_t i=0; i<4096; ++i) a[i]=(unsigned char)(i*17+3);
    for (size_t i=0; i<8192; ++i) b[i]=(unsigned char)(i*29+11);
    for (size_t i=0; i<4096; ++i) assert(a[i] == (unsigned char)(i*17+3));
    for (size_t i=0; i<8192; ++i) assert(b[i] == (unsigned char)(i*29+11));
    cram_telemetry t;
    assert(cram_get_telemetry(&t) == CRAM_OK && t.allocation_count == 2);
    assert(cram_free(a) == CRAM_OK && cram_free(b) == CRAM_OK);
    assert(cram_unregister_pressure_callback_and_wait() == CRAM_OK);
    assert(cram_get_telemetry(&t) == CRAM_OK && t.allocation_count == 0);
    puts("SDK_C_SMOKE_PASS");
    return 0;
}
