#ifndef LIBCRAM_HPP
#define LIBCRAM_HPP
#include "libcram.h"
#include <cstdlib>
#include <cstdint>
#include <exception>
#include <memory_resource>
#include <new>

namespace cram {
// The resource must outlive its pmr containers. Identity is object identity;
// allocation hints do not establish an ownership relationship between resources.
class memory_resource final : public std::pmr::memory_resource {
    std::uint32_t hint_;
    void* do_allocate(std::size_t bytes, std::size_t alignment) override {
        if (void* ptr = cram_alloc(bytes == 0 ? 1 : bytes, alignment, hint_)) return ptr;
        throw std::bad_alloc();
    }
    void do_deallocate(void* ptr, std::size_t, std::size_t) override {
        // pmr deallocation cannot report an ownership error to its caller.
        if (cram_free(ptr) != CRAM_OK) std::terminate();
    }
    bool do_is_equal(const std::pmr::memory_resource& other) const noexcept override {
        return this == &other;
    }
public:
    explicit memory_resource(std::uint32_t hint = CRAM_AUTO) : hint_(hint) {
        if (hint > CRAM_WRITE_HEAVY || cram_init() != CRAM_OK) throw std::bad_alloc();
    }
    memory_resource(const memory_resource&) = delete;
    memory_resource& operator=(const memory_resource&) = delete;
};
}
#endif
