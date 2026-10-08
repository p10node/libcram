#include "libcram.hpp"
#include <cassert>
#include <cstdint>
#include <memory_resource>
#include <vector>
struct alignas(256) Record { int value; };
int main() {
    cram::memory_resource a, b;
    assert(a.is_equal(a)); assert(!a.is_equal(b));
    std::pmr::vector<Record> values(&a);
    values.resize(32);
    assert(reinterpret_cast<std::uintptr_t>(values.data()) % 256 == 0);
    values[31].value = 77; assert(values[31].value == 77);
    void* zero = a.allocate(0, 64); assert(zero); a.deallocate(zero,0,64);
    bool rejected=false;
    try { cram::memory_resource invalid(99); } catch (const std::bad_alloc&) {rejected=true;}
    assert(rejected);
}
