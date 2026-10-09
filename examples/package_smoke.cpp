#include <cram_resource.hpp>
#include <cassert>
#include <cstdint>
#include <cstdio>
#include <vector>
#include "package_probe.h"
int main() {
    assert(cram_init() == CRAM_OK);
    assert_sdk_library();
    {
        cram::pmr::memory_resource resource;
        std::pmr::vector<std::uint32_t> a{&resource};
        std::pmr::vector<std::uint32_t> b{&resource};
        for (std::uint32_t i=0; i<1024; ++i) { a.push_back(i*31+7); b.push_back(i*19+11); }
        for (std::uint32_t i=0; i<1024; ++i) { assert(a[i]==i*31+7); assert(b[i]==i*19+11); }
    }
    cram_telemetry t{};
    assert(cram_get_telemetry(&t) == CRAM_OK && t.allocation_count == 0);
    std::puts("SDK_CPP_SMOKE_PASS");
}
