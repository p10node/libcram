#ifndef _GNU_SOURCE
#define _GNU_SOURCE
#endif
#include <assert.h>
#include <dlfcn.h>
#include <limits.h>
#include <stdlib.h>
#include <string.h>
static void assert_sdk_library(void) {
    const char *expected = getenv("LIBCRAM_EXPECTED_LIBRARY");
    assert(expected != NULL);
    Dl_info info;
    assert(dladdr((void *)cram_init, &info) != 0);
    char actual[PATH_MAX], wanted[PATH_MAX];
    assert(realpath(info.dli_fname, actual) != NULL);
    assert(realpath(expected, wanted) != NULL);
    assert(strcmp(actual, wanted) == 0);
}
